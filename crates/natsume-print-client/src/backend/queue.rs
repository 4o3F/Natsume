use snafu::ResultExt as _;
use windows::{
    Win32::{
        Foundation::{
            ERROR_FILE_NOT_FOUND, ERROR_INSUFFICIENT_BUFFER, ERROR_INVALID_PARAMETER,
            ERROR_NOT_FOUND, GetLastError,
        },
        Graphics::Printing::{
            ClosePrinter, EnumPrintersW, GetJobW, GetPrinterW, JOB_INFO_1W,
            JOB_STATUS_BLOCKED_DEVQ, JOB_STATUS_COMPLETE, JOB_STATUS_DELETED, JOB_STATUS_DELETING,
            JOB_STATUS_ERROR, JOB_STATUS_OFFLINE, JOB_STATUS_PAPEROUT, JOB_STATUS_PAUSED,
            JOB_STATUS_PRINTED, JOB_STATUS_PRINTING, JOB_STATUS_USER_INTERVENTION, OpenPrinterW,
            PRINTER_ATTRIBUTE_WORK_OFFLINE, PRINTER_ENUM_CONNECTIONS, PRINTER_ENUM_LOCAL,
            PRINTER_HANDLE, PRINTER_INFO_2W, PRINTER_INFO_4W,
        },
    },
    core::PCWSTR,
};

use super::{buffer::NativeBuffer, wide};
use crate::{JobObservation, NativeSnafu, QueueSnapshot, Result};

pub(super) struct Printer(pub(super) PRINTER_HANDLE);

impl Printer {
    pub(super) fn open(name: &str) -> Result<Self> {
        let name = wide(name)?;
        let mut handle = PRINTER_HANDLE::default();
        // SAFETY: the name is terminated and alive; handle points to writable storage.
        unsafe { OpenPrinterW(PCWSTR(name.as_ptr()), &raw mut handle, None) }.context(
            NativeSnafu {
                operation: "open printer",
            },
        )?;
        Ok(Self(handle))
    }

    pub(super) fn snapshot(&self) -> Result<QueueSnapshot> {
        let mut needed = 0;
        // SAFETY: this size query writes only to needed; the handle remains owned here.
        let result = unsafe { GetPrinterW(self.0, 2, None, &raw mut needed) };
        if let Err(source) = result
            && source.code() != windows::core::HRESULT::from_win32(ERROR_INSUFFICIENT_BUFFER.0)
        {
            return Err(crate::PrintError::Native {
                operation: "query printer size",
                source,
            });
        }
        let mut buffer = NativeBuffer::new(needed)?;
        // SAFETY: the buffer has the requested capacity and needed is writable.
        unsafe { GetPrinterW(self.0, 2, Some(buffer.bytes()), &raw mut needed) }.context(
            NativeSnafu {
                operation: "query printer",
            },
        )?;
        // SAFETY: a successful level-2 query initialized this structure.
        let info = unsafe { buffer.read::<PRINTER_INFO_2W>(0) }?;
        Ok(QueueSnapshot {
            name: buffer.string(info.pPrinterName)?,
            port: buffer.string(info.pPortName)?,
            driver: buffer.string(info.pDriverName)?,
            status: info.Status,
            jobs: info.cJobs,
            work_offline: info.Attributes & PRINTER_ATTRIBUTE_WORK_OFFLINE != 0,
        })
    }
}

impl Drop for Printer {
    fn drop(&mut self) {
        // SAFETY: this is the single owner of a successfully opened handle.
        if unsafe { ClosePrinter(self.0) }.is_err() {
            tracing::warn!("closing the printer handle failed");
        }
    }
}

/// Enumerates Windows queues visible to the current logged-in user.
///
/// # Errors
/// Returns a native query or malformed-buffer error.
pub fn list_queues() -> Result<Vec<String>> {
    let (mut needed, mut count) = (0, 0);
    let flags = PRINTER_ENUM_LOCAL | PRINTER_ENUM_CONNECTIONS;
    // SAFETY: both output counts are valid; no data buffer is supplied for the size query.
    let result = unsafe {
        EnumPrintersW(
            flags,
            PCWSTR::null(),
            4,
            None,
            &raw mut needed,
            &raw mut count,
        )
    };
    if let Err(source) = result
        && source.code() != windows::core::HRESULT::from_win32(ERROR_INSUFFICIENT_BUFFER.0)
    {
        return Err(crate::PrintError::Native {
            operation: "enumerate printer size",
            source,
        });
    }
    if needed == 0 {
        return Ok(Vec::new());
    }
    let mut buffer = NativeBuffer::new(needed)?;
    // SAFETY: the aligned allocation has the requested size; both count outputs are valid.
    unsafe {
        EnumPrintersW(
            flags,
            PCWSTR::null(),
            4,
            Some(buffer.bytes()),
            &raw mut needed,
            &raw mut count,
        )
    }
    .context(NativeSnafu {
        operation: "enumerate printers",
    })?;
    let mut names = Vec::new();
    for index in 0..count as usize {
        // SAFETY: a successful level-4 query initialized count entries; bounds are checked.
        let info = unsafe { buffer.read::<PRINTER_INFO_4W>(index) }?;
        names.push(buffer.string(info.pPrinterName)?);
    }
    names.sort();
    names.dedup();
    Ok(names)
}

/// Observes queue metadata without treating it as physical-printer evidence.
///
/// # Errors
/// Returns native open/query failures.
pub fn inspect_queue(name: &str) -> Result<QueueSnapshot> {
    Printer::open(name)?.snapshot()
}

/// Checks both the numeric job ID and the exact Natsume document label.
///
/// # Errors
/// Returns query errors other than a job that is no longer present.
pub fn observe_job(queue: &str, job_id: u32, document_label: &str) -> Result<JobObservation> {
    if job_id == 0 {
        return crate::InvalidSnafu { field: "job_id" }.fail();
    }
    let printer = Printer::open(queue)?;
    let mut needed = 0;
    // SAFETY: the owned handle and output count are valid; this is a size query.
    let success = unsafe { GetJobW(printer.0, job_id, 1, None, &raw mut needed) };
    if !success.as_bool() {
        // SAFETY: read immediately after the failed query, before any other native call.
        let code = unsafe { GetLastError() };
        if [
            ERROR_INVALID_PARAMETER,
            ERROR_FILE_NOT_FOUND,
            ERROR_NOT_FOUND,
        ]
        .contains(&code)
        {
            return Ok(JobObservation::Missing);
        }
        if code != ERROR_INSUFFICIENT_BUFFER {
            return Err(crate::PrintError::Native {
                operation: "query print job size",
                source: windows::core::Error::from_hresult(windows::core::HRESULT::from_win32(
                    code.0,
                )),
            });
        }
    }
    let mut buffer = NativeBuffer::new(needed)?;
    // SAFETY: this level-1 query uses the sized allocation and valid output count.
    let success = unsafe { GetJobW(printer.0, job_id, 1, Some(buffer.bytes()), &raw mut needed) };
    if !success.as_bool() {
        // SAFETY: capture the query error before another native call can replace it.
        let code = unsafe { GetLastError() };
        if [
            ERROR_INVALID_PARAMETER,
            ERROR_FILE_NOT_FOUND,
            ERROR_NOT_FOUND,
        ]
        .contains(&code)
        {
            return Ok(JobObservation::Missing);
        }
        return Err(crate::PrintError::Native {
            operation: "query print job",
            source: windows::core::Error::from_hresult(windows::core::HRESULT::from_win32(code.0)),
        });
    }
    // SAFETY: the successful level-1 query initialized JOB_INFO_1W.
    let job = unsafe { buffer.read::<JOB_INFO_1W>(0) }?;
    if job.JobId != job_id || buffer.string(job.pDocument)? != document_label {
        return Ok(JobObservation::IdentityMismatch);
    }
    Ok(classify_status(job.Status))
}

fn classify_status(status: u32) -> JobObservation {
    if status & (JOB_STATUS_DELETED | JOB_STATUS_DELETING) != 0 {
        JobObservation::Deleted
    } else if status
        & (JOB_STATUS_ERROR
            | JOB_STATUS_OFFLINE
            | JOB_STATUS_PAPEROUT
            | JOB_STATUS_PAUSED
            | JOB_STATUS_BLOCKED_DEVQ
            | JOB_STATUS_USER_INTERVENTION)
        != 0
    {
        JobObservation::NeedsAttention
    } else if status & JOB_STATUS_PRINTED != 0 {
        JobObservation::SpoolerReportedPrinted
    } else if status & JOB_STATUS_COMPLETE != 0 {
        JobObservation::SentToDevice
    } else if status & JOB_STATUS_PRINTING != 0 {
        JobObservation::Printing
    } else {
        JobObservation::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_does_not_confuse_delivery_deletion_or_errors_with_printing() {
        for (flags, expected) in [
            (0, JobObservation::Pending),
            (JOB_STATUS_PRINTING, JobObservation::Printing),
            (JOB_STATUS_COMPLETE, JobObservation::SentToDevice),
            (JOB_STATUS_PRINTED, JobObservation::SpoolerReportedPrinted),
            (
                JOB_STATUS_PRINTED | JOB_STATUS_ERROR,
                JobObservation::NeedsAttention,
            ),
            (
                JOB_STATUS_PRINTED | JOB_STATUS_DELETED,
                JobObservation::Deleted,
            ),
        ] {
            assert_eq!(classify_status(flags), expected);
        }
    }
}
