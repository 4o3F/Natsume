//! Compile-time ownership of Privileged Helper stable error codes.
//!
//! Home transition runtime arrives in a later probe. The hardware collector blueprint's
//! `NotImplemented` error intentionally has no stable wire code.

use natsume_error_code::ErrorCode;

/// Stable codes owned by Privileged Helper production boundaries.
pub const PRIVILEGED_HELPER_ERROR_CODES: &[ErrorCode] =
    &[ErrorCode::HomeTransition, ErrorCode::PackageLayoutInvalid];

#[cfg(test)]
mod tests {
    use natsume_error_code::{to_dbus_name, to_protocol_code};

    use super::*;

    #[test]
    fn helper_codes_have_explicit_protocol_and_dbus_mappings() {
        for code in PRIVILEGED_HELPER_ERROR_CODES {
            assert_eq!(to_protocol_code(*code), code.as_str());
            assert!(to_dbus_name(*code).starts_with("org.natsume.Error."));
        }
    }
}
