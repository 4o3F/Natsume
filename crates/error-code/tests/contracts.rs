use std::{collections::BTreeSet, fmt::Debug};

use natsume_error_code::{
    ErrorCode, common::CommonErrorCode, control::ControlErrorCode, device::DeviceErrorCode,
    enrollment::EnrollmentErrorCode, home::HomeErrorCode, operator::OperatorErrorCode,
    session::SessionErrorCode,
};
use serde::{Serialize, de::DeserializeOwned};

const COMMON_CODES: [CommonErrorCode; 4] = [
    CommonErrorCode::InternalError,
    CommonErrorCode::InvalidRequest,
    CommonErrorCode::AuthenticationFailed,
    CommonErrorCode::AuthorizationDenied,
];

const OPERATOR_CODES: [OperatorErrorCode; 4] = [
    OperatorErrorCode::ImportCandidateInvalid,
    OperatorErrorCode::ImportCandidatePending,
    OperatorErrorCode::ImportCandidateUnavailable,
    OperatorErrorCode::ImportPreviewStale,
];

const ENROLLMENT_CODES: [EnrollmentErrorCode; 3] = [
    EnrollmentErrorCode::ProvisioningWindowClosed,
    EnrollmentErrorCode::EnrollmentRequestInvalid,
    EnrollmentErrorCode::DeviceIdentityConflict,
];

const CONTROL_CODES: [ControlErrorCode; 7] = [
    ControlErrorCode::CommandIdInvalid,
    ControlErrorCode::CommandRequestConflict,
    ControlErrorCode::ProtocolVersionUnsupported,
    ControlErrorCode::ProtocolInvalidEnvelope,
    ControlErrorCode::CommandPayloadConflict,
    ControlErrorCode::CommandPayloadInvalid,
    ControlErrorCode::CommandStale,
];

const DEVICE_CODES: [DeviceErrorCode; 8] = [
    DeviceErrorCode::DeviceIdentityUnavailable,
    DeviceErrorCode::DeviceIdentityMismatch,
    DeviceErrorCode::DeviceCredentialsUnreadable,
    DeviceErrorCode::GatewayCredentialInvalid,
    DeviceErrorCode::GatewayCredentialInstallFailed,
    DeviceErrorCode::GatewayActivationFailed,
    DeviceErrorCode::GatewayUpstreamTlsRequired,
    DeviceErrorCode::SecretInstallFailed,
];

const SESSION_CODES: [SessionErrorCode; 4] = [
    SessionErrorCode::SessionContextStale,
    SessionErrorCode::SessionUnavailable,
    SessionErrorCode::SessionActionUnsupported,
    SessionErrorCode::SessionStateConflict,
];

const HOME_CODES: [HomeErrorCode; 2] = [
    HomeErrorCode::HomeEpochStale,
    HomeErrorCode::HomeOperationFailed,
];

const ALL_ERROR_CODES: [(ErrorCode, &str); 32] = [
    (
        ErrorCode::Common(CommonErrorCode::InternalError),
        "INTERNAL_ERROR",
    ),
    (
        ErrorCode::Common(CommonErrorCode::InvalidRequest),
        "INVALID_REQUEST",
    ),
    (
        ErrorCode::Common(CommonErrorCode::AuthenticationFailed),
        "AUTHENTICATION_FAILED",
    ),
    (
        ErrorCode::Common(CommonErrorCode::AuthorizationDenied),
        "AUTHORIZATION_DENIED",
    ),
    (
        ErrorCode::Operator(OperatorErrorCode::ImportCandidateInvalid),
        "IMPORT_CANDIDATE_INVALID",
    ),
    (
        ErrorCode::Operator(OperatorErrorCode::ImportCandidatePending),
        "IMPORT_CANDIDATE_PENDING",
    ),
    (
        ErrorCode::Operator(OperatorErrorCode::ImportCandidateUnavailable),
        "IMPORT_CANDIDATE_UNAVAILABLE",
    ),
    (
        ErrorCode::Operator(OperatorErrorCode::ImportPreviewStale),
        "IMPORT_PREVIEW_STALE",
    ),
    (
        ErrorCode::Enrollment(EnrollmentErrorCode::ProvisioningWindowClosed),
        "PROVISIONING_WINDOW_CLOSED",
    ),
    (
        ErrorCode::Enrollment(EnrollmentErrorCode::EnrollmentRequestInvalid),
        "ENROLLMENT_REQUEST_INVALID",
    ),
    (
        ErrorCode::Enrollment(EnrollmentErrorCode::DeviceIdentityConflict),
        "DEVICE_IDENTITY_CONFLICT",
    ),
    (
        ErrorCode::Control(ControlErrorCode::CommandIdInvalid),
        "COMMAND_ID_INVALID",
    ),
    (
        ErrorCode::Control(ControlErrorCode::CommandRequestConflict),
        "COMMAND_REQUEST_CONFLICT",
    ),
    (
        ErrorCode::Control(ControlErrorCode::ProtocolVersionUnsupported),
        "PROTOCOL_VERSION_UNSUPPORTED",
    ),
    (
        ErrorCode::Control(ControlErrorCode::ProtocolInvalidEnvelope),
        "PROTOCOL_INVALID_ENVELOPE",
    ),
    (
        ErrorCode::Control(ControlErrorCode::CommandPayloadConflict),
        "COMMAND_PAYLOAD_CONFLICT",
    ),
    (
        ErrorCode::Control(ControlErrorCode::CommandPayloadInvalid),
        "COMMAND_PAYLOAD_INVALID",
    ),
    (
        ErrorCode::Control(ControlErrorCode::CommandStale),
        "COMMAND_STALE",
    ),
    (
        ErrorCode::Device(DeviceErrorCode::DeviceIdentityUnavailable),
        "DEVICE_IDENTITY_UNAVAILABLE",
    ),
    (
        ErrorCode::Device(DeviceErrorCode::DeviceIdentityMismatch),
        "DEVICE_IDENTITY_MISMATCH",
    ),
    (
        ErrorCode::Device(DeviceErrorCode::DeviceCredentialsUnreadable),
        "DEVICE_CREDENTIALS_UNREADABLE",
    ),
    (
        ErrorCode::Device(DeviceErrorCode::GatewayCredentialInvalid),
        "GATEWAY_CREDENTIAL_INVALID",
    ),
    (
        ErrorCode::Device(DeviceErrorCode::GatewayCredentialInstallFailed),
        "GATEWAY_CREDENTIAL_INSTALL_FAILED",
    ),
    (
        ErrorCode::Device(DeviceErrorCode::GatewayActivationFailed),
        "GATEWAY_ACTIVATION_FAILED",
    ),
    (
        ErrorCode::Device(DeviceErrorCode::GatewayUpstreamTlsRequired),
        "GATEWAY_UPSTREAM_TLS_REQUIRED",
    ),
    (
        ErrorCode::Device(DeviceErrorCode::SecretInstallFailed),
        "SECRET_INSTALL_FAILED",
    ),
    (
        ErrorCode::Session(SessionErrorCode::SessionContextStale),
        "SESSION_CONTEXT_STALE",
    ),
    (
        ErrorCode::Session(SessionErrorCode::SessionUnavailable),
        "SESSION_UNAVAILABLE",
    ),
    (
        ErrorCode::Session(SessionErrorCode::SessionActionUnsupported),
        "SESSION_ACTION_UNSUPPORTED",
    ),
    (
        ErrorCode::Session(SessionErrorCode::SessionStateConflict),
        "SESSION_STATE_CONFLICT",
    ),
    (
        ErrorCode::Home(HomeErrorCode::HomeEpochStale),
        "HOME_EPOCH_STALE",
    ),
    (
        ErrorCode::Home(HomeErrorCode::HomeOperationFailed),
        "HOME_OPERATION_FAILED",
    ),
];

#[test]
fn registry_matches_the_normative_catalog() {
    for (code, expected) in ALL_ERROR_CODES {
        assert_eq!(expected_string(code), expected);
        assert_eq!(code.as_str(), expected);
        assert_eq!(serialize(&code), format!("\"{expected}\""));
        assert_eq!(deserialize_error_code(&serialize(&code)), code);
    }
}

#[test]
fn stable_strings_are_unique_upper_snake_case() {
    let mut strings = BTreeSet::new();

    for (code, stable) in ALL_ERROR_CODES {
        assert!(strings.insert(stable), "duplicate stable string {stable}");
        assert!(
            is_upper_snake_case(stable),
            "invalid stable string {stable}"
        );
        assert_ne!(format!("{code:?}"), stable);
    }
}

#[test]
fn category_codes_convert_to_the_unified_registry() {
    for code in COMMON_CODES {
        assert_category_round_trip(code, code.as_str());
        assert_eq!(ErrorCode::from(code), ErrorCode::Common(code));
    }
    for code in OPERATOR_CODES {
        assert_category_round_trip(code, code.as_str());
        assert_eq!(ErrorCode::from(code), ErrorCode::Operator(code));
    }
    for code in ENROLLMENT_CODES {
        assert_category_round_trip(code, code.as_str());
        assert_eq!(ErrorCode::from(code), ErrorCode::Enrollment(code));
    }
    for code in CONTROL_CODES {
        assert_category_round_trip(code, code.as_str());
        assert_eq!(ErrorCode::from(code), ErrorCode::Control(code));
    }
    for code in DEVICE_CODES {
        assert_category_round_trip(code, code.as_str());
        assert_eq!(ErrorCode::from(code), ErrorCode::Device(code));
    }
    for code in SESSION_CODES {
        assert_category_round_trip(code, code.as_str());
        assert_eq!(ErrorCode::from(code), ErrorCode::Session(code));
    }
    for code in HOME_CODES {
        assert_category_round_trip(code, code.as_str());
        assert_eq!(ErrorCode::from(code), ErrorCode::Home(code));
    }
}

#[test]
fn unknown_codes_are_rejected_without_retaining_input() {
    let canary = "UNKNOWN_/var/lib/natsume_secret=hunter2";
    let encoded = serialize(canary);
    let error = match serde_json::from_str::<ErrorCode>(&encoded) {
        Ok(code) => panic!("unexpectedly deserialized unknown code: {code:?}"),
        Err(error) => error,
    };
    let display = error.to_string();
    let debug = format!("{error:?}");

    assert!(!display.contains(canary));
    assert!(!debug.contains(canary));
}

#[test]
fn command_identity_codes_remain_exact() {
    assert_eq!(
        ControlErrorCode::CommandIdInvalid.as_str(),
        "COMMAND_ID_INVALID"
    );
    assert_eq!(
        ControlErrorCode::CommandRequestConflict.as_str(),
        "COMMAND_REQUEST_CONFLICT"
    );
}

fn assert_category_round_trip<T>(code: T, expected: &str)
where
    T: Copy + Debug + DeserializeOwned + Eq + Serialize,
{
    let encoded = serialize(&code);
    assert_eq!(encoded, format!("\"{expected}\""));
    let decoded = match serde_json::from_str::<T>(&encoded) {
        Ok(decoded) => decoded,
        Err(error) => panic!("failed to deserialize {expected}: {error}"),
    };
    assert_eq!(decoded, code);
}

fn serialize<T>(value: &T) -> String
where
    T: Serialize + ?Sized,
{
    match serde_json::to_string(value) {
        Ok(encoded) => encoded,
        Err(error) => panic!("failed to serialize test value: {error}"),
    }
}

fn deserialize_error_code(encoded: &str) -> ErrorCode {
    match serde_json::from_str(encoded) {
        Ok(code) => code,
        Err(error) => panic!("failed to deserialize ErrorCode: {error}"),
    }
}

const fn expected_string(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::Common(code) => match code {
            CommonErrorCode::InternalError => "INTERNAL_ERROR",
            CommonErrorCode::InvalidRequest => "INVALID_REQUEST",
            CommonErrorCode::AuthenticationFailed => "AUTHENTICATION_FAILED",
            CommonErrorCode::AuthorizationDenied => "AUTHORIZATION_DENIED",
        },
        ErrorCode::Operator(code) => match code {
            OperatorErrorCode::ImportCandidateInvalid => "IMPORT_CANDIDATE_INVALID",
            OperatorErrorCode::ImportCandidatePending => "IMPORT_CANDIDATE_PENDING",
            OperatorErrorCode::ImportCandidateUnavailable => "IMPORT_CANDIDATE_UNAVAILABLE",
            OperatorErrorCode::ImportPreviewStale => "IMPORT_PREVIEW_STALE",
        },
        ErrorCode::Enrollment(code) => match code {
            EnrollmentErrorCode::ProvisioningWindowClosed => "PROVISIONING_WINDOW_CLOSED",
            EnrollmentErrorCode::EnrollmentRequestInvalid => "ENROLLMENT_REQUEST_INVALID",
            EnrollmentErrorCode::DeviceIdentityConflict => "DEVICE_IDENTITY_CONFLICT",
        },
        ErrorCode::Control(code) => match code {
            ControlErrorCode::CommandIdInvalid => "COMMAND_ID_INVALID",
            ControlErrorCode::CommandRequestConflict => "COMMAND_REQUEST_CONFLICT",
            ControlErrorCode::ProtocolVersionUnsupported => "PROTOCOL_VERSION_UNSUPPORTED",
            ControlErrorCode::ProtocolInvalidEnvelope => "PROTOCOL_INVALID_ENVELOPE",
            ControlErrorCode::CommandPayloadConflict => "COMMAND_PAYLOAD_CONFLICT",
            ControlErrorCode::CommandPayloadInvalid => "COMMAND_PAYLOAD_INVALID",
            ControlErrorCode::CommandStale => "COMMAND_STALE",
        },
        ErrorCode::Device(code) => match code {
            DeviceErrorCode::DeviceIdentityUnavailable => "DEVICE_IDENTITY_UNAVAILABLE",
            DeviceErrorCode::DeviceIdentityMismatch => "DEVICE_IDENTITY_MISMATCH",
            DeviceErrorCode::DeviceCredentialsUnreadable => "DEVICE_CREDENTIALS_UNREADABLE",
            DeviceErrorCode::GatewayCredentialInvalid => "GATEWAY_CREDENTIAL_INVALID",
            DeviceErrorCode::GatewayCredentialInstallFailed => "GATEWAY_CREDENTIAL_INSTALL_FAILED",
            DeviceErrorCode::GatewayActivationFailed => "GATEWAY_ACTIVATION_FAILED",
            DeviceErrorCode::GatewayUpstreamTlsRequired => "GATEWAY_UPSTREAM_TLS_REQUIRED",
            DeviceErrorCode::SecretInstallFailed => "SECRET_INSTALL_FAILED",
        },
        ErrorCode::Session(code) => match code {
            SessionErrorCode::SessionContextStale => "SESSION_CONTEXT_STALE",
            SessionErrorCode::SessionUnavailable => "SESSION_UNAVAILABLE",
            SessionErrorCode::SessionActionUnsupported => "SESSION_ACTION_UNSUPPORTED",
            SessionErrorCode::SessionStateConflict => "SESSION_STATE_CONFLICT",
        },
        ErrorCode::Home(code) => match code {
            HomeErrorCode::HomeEpochStale => "HOME_EPOCH_STALE",
            HomeErrorCode::HomeOperationFailed => "HOME_OPERATION_FAILED",
        },
    }
}

fn is_upper_snake_case(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('_')
        && !value.ends_with('_')
        && !value.contains("__")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
}
