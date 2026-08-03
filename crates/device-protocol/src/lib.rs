#![forbid(unsafe_code)]
//! Generated control protocol and semantic validation.

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

impl std::fmt::Debug for generated::SecretBytes {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SecretBytes([REDACTED])")
    }
}

pub use validation::{ProtocolValidationError, validate_envelope};

/// Returns the exact descriptor generated from `proto/device_control.proto`.
#[must_use]
pub const fn file_descriptor_set() -> &'static [u8] {
    include_bytes!(concat!(env!("OUT_DIR"), "/device_control.pb"))
}

#[cfg(test)]
mod tests {
    use super::generated::{EnrollDeviceResponse, SecretBytes, SyncSecret};

    #[test]
    fn secret_bytes_and_containing_messages_have_redacted_debug() {
        let value = b"device-token-must-never-appear".to_vec();
        let raw_debug = format!("{value:?}");
        let secret = SecretBytes {
            value: value.clone(),
        };
        let secret_debug = format!("{secret:?}");
        assert_eq!(secret_debug, "SecretBytes([REDACTED])");
        assert!(!secret_debug.contains(&raw_debug));

        let sync = SyncSecret {
            seat_id: "seat-1".to_owned(),
            binding_revision: 1,
            account_id: "account-1".to_owned(),
            credential_revision: 1,
            password: Some(secret.clone()),
        };
        let response = EnrollDeviceResponse {
            device_id: "device-1".to_owned(),
            device_token: Some(secret),
            gateway_leaf_der: Vec::new(),
            gateway_chain_der: Vec::new(),
            state: 0,
            stable_error_code: String::new(),
        };
        for containing_debug in [format!("{sync:?}"), format!("{response:?}")] {
            assert!(containing_debug.contains("SecretBytes([REDACTED])"));
            assert!(!containing_debug.contains(&raw_debug));
        }
    }
}
