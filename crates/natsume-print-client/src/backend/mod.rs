//! Native printing boundary. All handwritten unsafe code stays in this module tree.
//! Handles, DCs and `WinRT` objects remain on the calling worker thread.
mod buffer;
mod gdi;
mod queue;
mod render;

pub use queue::{inspect_queue, list_queues, observe_job};

use std::{marker::PhantomData, rc::Rc};

use snafu::ResultExt as _;
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize};

use crate::{Document, InvalidSnafu, NativeSnafu, Result, UnavailableSnafu};

struct Apartment(PhantomData<Rc<()>>);
impl Apartment {
    fn initialize() -> Result<Self> {
        // SAFETY: initialize the current worker thread; a successful call is balanced in Drop.
        unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.context(NativeSnafu {
            operation: "initialize Windows PDF runtime",
        })?;
        Ok(Self(PhantomData))
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        // SAFETY: this non-Send guard remains on the successfully initialized thread.
        unsafe { RoUninitialize() };
    }
}

fn wide(value: &str) -> Result<Vec<u16>> {
    if value.is_empty()
        || value.chars().any(char::is_control)
        || value.encode_utf16().count() > 32767
    {
        return InvalidSnafu {
            field: "Windows queue or document name",
        }
        .fail();
    }
    let mut units: Vec<u16> = value.encode_utf16().collect();
    units.push(0);
    Ok(units)
}

/// Renders an A4 PDF and submits one monochrome copy to an idle queue.
///
/// `on_started` receives `StartDocW`'s exact job ID before the first page is submitted.
/// It must persist the ID synchronously; failure aborts the document. Neither a
/// successful return nor an abort proves physical output or its absence.
///
/// # Errors
/// Returns native/renderer failures, rejected settings, busy queues or callback errors.
/// The caller must not automatically resubmit when a result is uncertain.
pub fn submit(
    queue: &str,
    document: &Document,
    on_started: impl FnOnce(u32) -> Result<()>,
) -> Result<u32> {
    let _apartment = Apartment::initialize()?;
    let pdf = render::Pdf::load(document)?;
    // Fail rendering before starting a job when even its first page cannot be decoded.
    let first = pdf.render(0)?;
    let printer = queue::Printer::open(queue)?;
    if !printer.snapshot()?.can_submit() {
        return UnavailableSnafu.fail();
    }
    let job = gdi::PrintJob::start(&printer, queue, document.label())?;
    on_started(job.job_id)?;
    job.page(&first)?;
    drop(first);
    for index in 1..pdf.pages {
        job.page(&pdf.render(index)?)?;
    }
    job.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_untrusted_native_names_and_terminates_valid_names() -> Result<()> {
        for name in ["", "printer\0other", "printer\nother", "\u{1b}bad"] {
            assert!(wide(name).is_err());
        }
        assert_eq!(wide("中文打印机")?.last(), Some(&0));
        Ok(())
    }
}
