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
        for code in ALL_ERROR_CODES {
            assert_eq!(to_protocol_code(code), code.as_str());
        }
    }
}
