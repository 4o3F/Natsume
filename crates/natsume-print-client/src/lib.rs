//! Native Windows printing backend. Server task claiming is not implemented here yet.
#![deny(unsafe_op_in_unsafe_fn)]

mod document;
mod layout;

// The reviewed native-interface exception is confined to this module tree.
#[cfg(windows)]
#[allow(unsafe_code)]
pub mod backend;

pub use document::Document;
pub use layout::{PageLayout, fit_page};

use snafu::Snafu;

/// Native calls may block on a printer driver; callers must use a worker thread.
#[derive(Debug, Snafu)]
pub enum PrintError {
    #[snafu(display("invalid print input: {field}"))]
    Invalid { field: &'static str },
    #[snafu(display("print filesystem operation failed: {operation}"))]
    Io {
        operation: &'static str,
        source: std::io::Error,
    },
    #[snafu(display("printer queue is busy, offline or requires attention"))]
    Unavailable,
    #[snafu(display("printer driver did not accept A4, monochrome, single-copy settings"))]
    SettingsRejected,
    #[snafu(display("Windows returned an invalid print buffer"))]
    NativeBuffer,
    #[snafu(display("printer driver operation failed: {operation} (code {code})"))]
    DriverCall { operation: &'static str, code: i32 },
    #[cfg(windows)]
    #[snafu(display("Windows print operation failed: {operation}"))]
    Native {
        operation: &'static str,
        source: windows::core::Error,
    },
}

pub type Result<T> = std::result::Result<T, PrintError>;

#[derive(Debug)]
pub struct QueueSnapshot {
    pub name: String,
    pub port: String,
    pub driver: String,
    pub status: u32,
    pub jobs: u32,
    pub work_offline: bool,
}

impl QueueSnapshot {
    /// This is a conservative queue observation, not proof of physical readiness.
    #[must_use]
    pub const fn can_submit(&self) -> bool {
        self.status == 0 && self.jobs == 0 && !self.work_offline
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum JobObservation {
    Pending,
    Printing,
    NeedsAttention,
    /// Windows reports delivery to the printer, not completion.
    SentToDevice,
    /// The spooler reports PRINTED; this is not a physical-output guarantee.
    SpoolerReportedPrinted,
    Deleted,
    /// The job is no longer observable. This is not a successful result.
    Missing,
    /// This queue now uses the numeric ID for a different document.
    IdentityMismatch,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_capacity_requires_zero_jobs_and_no_reported_fault() {
        let mut queue = QueueSnapshot {
            name: "queue".to_owned(),
            port: "port".to_owned(),
            driver: "driver".to_owned(),
            status: 0,
            jobs: 0,
            work_offline: false,
        };
        assert!(queue.can_submit());
        queue.jobs = 1;
        assert!(!queue.can_submit());
        queue.jobs = 0;
        queue.status = 1;
        assert!(!queue.can_submit());
        queue.status = 0;
        queue.work_offline = true;
        assert!(!queue.can_submit());
    }
}
