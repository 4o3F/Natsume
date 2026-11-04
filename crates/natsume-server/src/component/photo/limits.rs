//! Server-side Photo I/O budgets shared by upload, validation and delivery.
pub(crate) const IO_BUFFER_BYTES: usize = 64 * 1024;
pub(crate) const OPERATION_TIMEOUT: std::time::Duration = std::time::Duration::from_mins(15);
pub(super) const IMAGE_DECODE_BYTES: u64 = 320 * 1024 * 1024;
pub(super) const ACCOUNT_ID_BYTES: usize = 512;
// EOCD fixed header plus the largest u16 ZIP comment.
pub(super) const ZIP_EOCD_SCAN_BYTES: u64 = 22 + u16::MAX as u64;
