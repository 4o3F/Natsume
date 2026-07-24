#![forbid(unsafe_code)]
//! Generated control protocol, length-delimited framing and semantic validation.

pub mod framing;
pub mod validation;

// Prost owns this generated surface. First-party source remains subject to the
// workspace Clippy policy; these exceptions cover generator-emitted shapes only.
#[allow(
    clippy::doc_markdown,
    clippy::large_enum_variant,
    clippy::must_use_candidate
)]
pub mod generated {
    include!(concat!(env!("OUT_DIR"), "/natsume.device.v2.rs"));
}

pub use framing::{DEFAULT_MAX_FRAME_BYTES, ProtocolFrameError, decode_frame, encode_frame};
pub use validation::{ProtocolValidationError, validate_envelope};

/// Returns the exact descriptor generated from `proto/device_control.proto`.
#[must_use]
pub const fn file_descriptor_set() -> &'static [u8] {
    include_bytes!(concat!(env!("OUT_DIR"), "/device_control.pb"))
}
