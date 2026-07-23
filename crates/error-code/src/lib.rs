#![forbid(unsafe_code)]
//! Stable error contracts shared by Natsume production processes.
//!
//! Domain errors remain typed SNAFU enums in their owning modules. Public boundaries map
//! them explicitly through [`AsErrorCode`]; no caller may derive behavior from `Display`.

pub mod code;
pub mod dbus;
pub mod http;
pub mod protocol;
pub mod redact;

pub use code::{ALL_ERROR_CODES, ErrorCode};
pub use dbus::to_dbus_name;
pub use http::{ProblemDetails, to_problem_details};
pub use protocol::to_protocol_code;
pub use redact::{CodedReport, Redacted, RedactedString, redact_report};

/// Explicit mapping from a typed domain error to a stable [`ErrorCode`].
pub trait AsErrorCode {
    /// Returns the stable code for this domain error.
    #[must_use]
    fn error_code(&self) -> ErrorCode;
}
