#![forbid(unsafe_code)]
//! Generated Device Control schema and shared wire primitives.

mod transcript;

pub use transcript::{
    ProofVerificationError, client_proof_signing_digest, sign_client_proof, verify_client_proof,
};

/// Exact WebSocket subprotocol selected by both control peers.
pub const CONTROL_SUBPROTOCOL: &str = "natsume.control";
/// Exact HTTP route carrying Device control WebSocket upgrades.
pub const CONTROL_ROUTE: &str = "/api/v2/device/control";
/// Maximum encoded length of one Device Control `ErrorCode` token.
pub const ERROR_CODE_MAX_BYTES: usize = 64;

/// Returns whether `value` is a well-formed open `ErrorCode` token.
///
/// Receiver behavior must come from the accompanying typed state, never from
/// this diagnostic token.
#[must_use]
pub fn is_valid_error_code_token(value: &str) -> bool {
    let bytes = value.as_bytes();
    let Some((first, rest)) = bytes.split_first() else {
        return false;
    };

    bytes.len() <= ERROR_CODE_MAX_BYTES
        && first.is_ascii_uppercase()
        && rest
            .iter()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || *byte == b'_')
}

// Prost owns this generated surface. First-party source remains subject to the
// workspace Clippy policy; these exceptions cover generator-emitted shapes only.
#[allow(
    clippy::doc_markdown,
    clippy::large_enum_variant,
    clippy::must_use_candidate
)]
pub mod generated {
    include!(concat!(env!("OUT_DIR"), "/natsume.device.control.rs"));
}

impl std::fmt::Debug for generated::SecretBytes {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SecretBytes([REDACTED])")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_code_tokens_preserve_the_open_wire_grammar() {
        for valid in ["A", "PROTOCOL_INVALID_ENVELOPE", "FUTURE_PEER_CODE_7"] {
            assert!(is_valid_error_code_token(valid), "rejected {valid:?}");
        }
        assert!(is_valid_error_code_token(&"A".repeat(ERROR_CODE_MAX_BYTES)));

        for invalid in [
            "",
            "7_STARTS_WITH_DIGIT",
            "_STARTS_WITH_UNDERSCORE",
            "lowercase",
            "HAS-HYPHEN",
            "HAS SPACE",
            "非ASCII",
        ] {
            assert!(!is_valid_error_code_token(invalid), "accepted {invalid:?}");
        }
        assert!(!is_valid_error_code_token(
            &"A".repeat(ERROR_CODE_MAX_BYTES + 1)
        ));
    }

    #[test]
    fn secret_bytes_and_containing_messages_have_redacted_debug() {
        let value = b"binding-password-must-never-appear".to_vec();
        let raw_debug = format!("{value:?}");
        let secret = generated::SecretBytes {
            value: value.clone(),
        };
        let secret_debug = format!("{secret:?}");
        assert_eq!(secret_debug, "SecretBytes([REDACTED])");
        assert!(!secret_debug.contains(&raw_debug));

        let target = generated::BoundTarget {
            context: None,
            password: Some(secret),
        };
        let containing_debug = format!("{target:?}");
        assert!(containing_debug.contains("SecretBytes([REDACTED])"));
        assert!(!containing_debug.contains(&raw_debug));
    }

    #[test]
    fn generated_descriptor_matches_the_checked_in_golden() {
        let generated = include_bytes!(concat!(env!("OUT_DIR"), "/device_control.pb"));
        let golden = include_bytes!("../testdata/device_control.pb");
        assert_eq!(generated.as_slice(), golden.as_slice());
    }
}
