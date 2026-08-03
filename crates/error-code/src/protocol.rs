//! Protocol and `CommandStatus` mapping without a Prost dependency.

use crate::ErrorCode;

/// Returns the stable string written to protocol `stable_error_code` fields.
#[must_use]
pub const fn to_protocol_code(code: ErrorCode) -> &'static str {
    code.as_str()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ALL_ERROR_CODES;

    #[test]
    fn protocol_mapping_uses_only_registry_strings() {
        assert_eq!(
            to_protocol_code(ErrorCode::ProvisioningWindowClosed),
            "PROVISIONING_WINDOW_CLOSED"
        );
        assert_eq!(
            to_protocol_code(ErrorCode::CommandIdInvalid),
            "COMMAND_ID_INVALID"
        );
        assert_eq!(
            to_protocol_code(ErrorCode::CommandRequestConflict),
            "COMMAND_REQUEST_CONFLICT"
        );
        for code in ALL_ERROR_CODES {
            assert_eq!(to_protocol_code(code), code.as_str());
        }
    }
}
