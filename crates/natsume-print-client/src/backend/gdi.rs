use std::mem::size_of;

use windows::{
    Win32::{
        Graphics::{
            Gdi::{
                BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateDCW, DEVMODEW, DIB_RGB_COLORS,
                DM_COLOR, DM_COPIES, DM_IN_BUFFER, DM_ORIENTATION, DM_OUT_BUFFER, DM_PAPERLENGTH,
                DM_PAPERSIZE, DM_PAPERWIDTH, DM_SCALE, DMCOLOR_MONOCHROME, DMPAPER_A4, DeleteDC,
                GetDeviceCaps, HDC, HORZRES, SRCCOPY, StretchDIBits, VERTRES,
            },
            Printing::DocumentPropertiesW,
        },
        Storage::Xps::{AbortDoc, DOCINFOW, EndDoc, EndPage, StartDocW, StartPage},
    },
    core::PCWSTR,
};

use super::{buffer::NativeBuffer, queue::Printer, render::Raster, wide};
use crate::{DriverCallSnafu, InvalidSnafu, Result, SettingsRejectedSnafu, fit_page};

struct Device(HDC);
impl Drop for Device {
    fn drop(&mut self) {
        // SAFETY: this is the only owner of the successfully created printer DC.
        if !unsafe { DeleteDC(self.0) }.as_bool() {
            tracing::warn!("deleting the printer context failed");
        }
    }
}

pub(super) struct PrintJob {
    device: Device,
    pub(super) job_id: u32,
    ended: bool,
}

fn driver_result(code: i32, operation: &'static str) -> Result<i32> {
    if code <= 0 {
        DriverCallSnafu { operation, code }.fail()
    } else {
        Ok(code)
    }
}

fn settings(printer: &Printer, name: &[u16]) -> Result<NativeBuffer> {
    let name = PCWSTR(name.as_ptr());
    // SAFETY: the owned printer and terminated name remain valid; no DEVMODE is supplied.
    let required = unsafe { DocumentPropertiesW(None, printer.0, name, None, None, 0) };
    let required = u32::try_from(driver_result(required, "query printer settings size")?)
        .map_err(|_| SettingsRejectedSnafu.build())?;
    let mut buffer = NativeBuffer::new(required)?;
    let mode_address = buffer.pointer::<DEVMODEW>()?;
    // SAFETY: the aligned, zero-initialized buffer has the driver-requested capacity.
    let code = unsafe {
        DocumentPropertiesW(
            None,
            printer.0,
            name,
            Some(mode_address),
            None,
            DM_OUT_BUFFER.0,
        )
    };
    if code != 1 {
        return DriverCallSnafu {
            operation: "read printer settings",
            code,
        }
        .fail();
    }
    // SAFETY: the successful driver call filled a printer DEVMODEW, not a display DEVMODE.
    let mode = unsafe { &mut *mode_address };
    if usize::from(mode.dmSize) < size_of::<DEVMODEW>()
        || usize::from(mode.dmSize) + usize::from(mode.dmDriverExtra) > required as usize
    {
        return SettingsRejectedSnafu.fail();
    }
    mode.dmFields = (mode.dmFields & !(DM_PAPERWIDTH | DM_PAPERLENGTH))
        | DM_ORIENTATION
        | DM_PAPERSIZE
        | DM_COPIES
        | DM_COLOR
        | DM_SCALE;
    mode.dmColor = DMCOLOR_MONOCHROME;
    // SAFETY: DocumentPropertiesW initialized the printer variant of this union.
    let paper = unsafe { &mut mode.Anonymous1.Anonymous1 };
    paper.dmPaperSize = i16::try_from(DMPAPER_A4).map_err(|_| SettingsRejectedSnafu.build())?;
    paper.dmCopies = 1;
    paper.dmOrientation = 1;
    paper.dmScale = 100;
    // SAFETY: input and output may alias per DocumentPropertiesW; capacity is unchanged.
    let code = unsafe {
        DocumentPropertiesW(
            None,
            printer.0,
            name,
            Some(mode_address),
            Some(mode_address),
            (DM_IN_BUFFER | DM_OUT_BUFFER).0,
        )
    };
    if code != 1 {
        return DriverCallSnafu {
            operation: "validate printer settings",
            code,
        }
        .fail();
    }
    // SAFETY: the successful round trip initialized the printer DEVMODEW again.
    let mode = unsafe { &*mode_address };
    // SAFETY: the printer DEVMODE union uses the printer layout.
    let paper = unsafe { mode.Anonymous1.Anonymous1 };
    let required_fields = DM_ORIENTATION | DM_PAPERSIZE | DM_COPIES | DM_COLOR | DM_SCALE;
    if mode.dmFields & required_fields != required_fields
        || mode.dmColor != DMCOLOR_MONOCHROME
        || u32::try_from(paper.dmPaperSize).ok() != Some(DMPAPER_A4)
        || paper.dmCopies != 1
        || paper.dmScale != 100
        || paper.dmOrientation != 1
    {
        return SettingsRejectedSnafu.fail();
    }
    Ok(buffer)
}

impl PrintJob {
    pub(super) fn start(printer: &Printer, queue: &str, label: &str) -> Result<Self> {
        let name = wide(queue)?;
        let label = wide(label)?;
        let mut mode = settings(printer, &name)?;
        let mode_pointer = mode.pointer::<DEVMODEW>()?;
        // SAFETY: the name and full driver settings remain live through this call.
        let dc = unsafe {
            CreateDCW(
                windows::core::w!("WINSPOOL"),
                PCWSTR(name.as_ptr()),
                PCWSTR::null(),
                Some(mode_pointer),
            )
        };
        if dc.is_invalid() {
            return DriverCallSnafu {
                operation: "create printer context",
                code: 0,
            }
            .fail();
        }
        let device = Device(dc);
        let info = DOCINFOW {
            cbSize: i32::try_from(size_of::<DOCINFOW>()).map_err(|_| {
                InvalidSnafu {
                    field: "document info",
                }
                .build()
            })?,
            lpszDocName: PCWSTR(label.as_ptr()),
            ..Default::default()
        };
        // SAFETY: DOCINFOW and its terminated label are live; dc is an owned printer DC.
        let id = driver_result(
            unsafe { StartDocW(device.0, &raw const info) },
            "start print job",
        )?;
        let job_id = u32::try_from(id).map_err(|_| InvalidSnafu { field: "job_id" }.build())?;
        Ok(Self {
            device,
            job_id,
            ended: false,
        })
    }

    pub(super) fn page(&self, raster: &Raster) -> Result<()> {
        // SAFETY: the DC remains owned and this function runs on its creating thread.
        let width = unsafe { GetDeviceCaps(Some(self.device.0), HORZRES) };
        // SAFETY: the same live DC is used to query its printable area.
        let height = unsafe { GetDeviceCaps(Some(self.device.0), VERTRES) };
        let layout = fit_page(raster.width, raster.height, width, height)?;
        let bitmap = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: u32::try_from(size_of::<BITMAPINFOHEADER>()).map_err(|_| {
                    InvalidSnafu {
                        field: "bitmap header",
                    }
                    .build()
                })?,
                biWidth: raster.width,
                biHeight: -raster.height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        // SAFETY: the DC owns an active document; no page is already open.
        driver_result(unsafe { StartPage(self.device.0) }, "start print page")?;
        // SAFETY: the top-down BGRA buffer matches the header dimensions and is live for the call.
        let rows = unsafe {
            StretchDIBits(
                self.device.0,
                layout.x,
                layout.y,
                layout.width,
                layout.height,
                0,
                0,
                raster.width,
                raster.height,
                Some(raster.pixels.as_ptr().cast()),
                &raw const bitmap,
                DIB_RGB_COLORS,
                SRCCOPY,
            )
        };
        driver_result(rows, "draw print page")?;
        // SAFETY: the successful StartPage belongs to this active document.
        driver_result(unsafe { EndPage(self.device.0) }, "end print page")?;
        Ok(())
    }

    pub(super) fn finish(mut self) -> Result<u32> {
        // SAFETY: all opened pages have ended and the DC still owns this document.
        driver_result(unsafe { EndDoc(self.device.0) }, "submit print document")?;
        self.ended = true;
        Ok(self.job_id)
    }
}

impl Drop for PrintJob {
    fn drop(&mut self) {
        if !self.ended {
            // SAFETY: this DC still owns the unfinished document. No retry is attempted.
            if unsafe { AbortDoc(self.device.0) } <= 0 {
                tracing::warn!(
                    job_id = self.job_id,
                    "aborting the unfinished print job failed; output is uncertain"
                );
            }
        }
    }
}
