#![forbid(unsafe_code)]
//! Generated control protocol and semantic validation.

pub mod validation;

/// Exact WebSocket subprotocol selected by both control peers.
pub const CONTROL_SUBPROTOCOL: &str = "natsume.v1";
/// Current Device control wire version.
pub const CONTROL_WIRE_VERSION: u32 = 1;
/// Maximum encoded control-envelope frame accepted by either peer.
pub const CONTROL_MAX_FRAME_BYTES: usize = 65_536;
/// Deadline shared by the TCP, TLS, WebSocket, and hello exchanges.
pub const CONTROL_HELLO_TIMEOUT_SECONDS: u64 = 10;
/// Canonical unpadded base64url length of one 32-byte Device Token.
pub const DEVICE_TOKEN_ENCODED_LENGTH: usize = 43;

/// Returns whether bytes have the canonical encoded shape of one 32-byte Device Token.
#[must_use]
pub fn is_valid_device_token(token: &[u8]) -> bool {
    token.len() == DEVICE_TOKEN_ENCODED_LENGTH
        && token
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        && token
            .last()
            .is_some_and(|byte| b"AEIMQUYcgkosw048".contains(byte))
}

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

pub use validation::{ProtocolValidationError, is_canonical_command_id, validate_envelope};

/// Returns the exact descriptor generated from `proto/device_control.proto`.
#[must_use]
pub const fn file_descriptor_set() -> &'static [u8] {
    include_bytes!(concat!(env!("OUT_DIR"), "/device_control.pb"))
}

#[cfg(test)]
mod tests {
    use super::{
        DEVICE_TOKEN_ENCODED_LENGTH,
        generated::{EnrollDeviceResponse, SecretBytes, SyncSecret},
        is_valid_device_token,
    };

    const VALID_TOKEN: &[u8] = b"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

    #[test]
    fn device_token_shape_pins_the_32_byte_base64url_tail() {
        assert_eq!(VALID_TOKEN.len(), DEVICE_TOKEN_ENCODED_LENGTH);
        assert!(is_valid_device_token(VALID_TOKEN));

        let mut invalid_tail = VALID_TOKEN.to_vec();
        invalid_tail[DEVICE_TOKEN_ENCODED_LENGTH - 1] = b'B';
        assert!(!is_valid_device_token(&invalid_tail));
        assert!(!is_valid_device_token(&VALID_TOKEN[..42]));
        assert!(!is_valid_device_token(
            b"!AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
        ));
    }

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
