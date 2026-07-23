//! Compile-time ownership of Session Agent stable error codes.
//!
//! Desktop lock runtime and D-Bus methods remain outside Step 3. Session lock never owns
//! Caddy state.

use natsume_error_code::ErrorCode;

/// Stable codes owned by the Session Agent desktop-lock surface.
pub const SESSION_AGENT_ERROR_CODES: &[ErrorCode] = &[
    ErrorCode::SessionChanged,
    ErrorCode::StaleLockEpoch,
    ErrorCode::LockCommandMismatch,
    ErrorCode::NoActiveLock,
];

#[cfg(test)]
mod tests {
    use natsume_error_code::{to_dbus_name, to_protocol_code};

    use super::*;

    #[test]
    fn session_codes_have_explicit_protocol_and_dbus_mappings() {
        for code in SESSION_AGENT_ERROR_CODES {
            assert_eq!(to_protocol_code(*code), code.as_str());
            assert!(to_dbus_name(*code).starts_with("org.natsume.Error."));
        }
    }
}
