//! HTTP Problem Details mapping without an Axum dependency.

use serde::Serialize;
use uuid::Uuid;

use crate::ErrorCode;

/// RFC 9457-shaped Problem Details payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProblemDetails {
    #[serde(rename = "type")]
    type_uri: String,
    title: String,
    status: u16,
    code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
    correlation_id: Uuid,
}

impl ProblemDetails {
    /// Returns the problem-type URI.
    #[must_use]
    pub fn type_uri(&self) -> &str {
        &self.type_uri
    }

    /// Returns the public, non-stable title.
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Returns the HTTP status code.
    #[must_use]
    pub const fn status(&self) -> u16 {
        self.status
    }

    /// Returns the stable error-code string.
    #[must_use]
    pub fn code(&self) -> &str {
        &self.code
    }

    /// Returns reviewed public detail text, if a future boundary explicitly supplies it.
    #[must_use]
    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }

    /// Returns the request correlation ID.
    #[must_use]
    pub const fn correlation_id(&self) -> Uuid {
        self.correlation_id
    }
}

/// Returns the explicit HTTP status mapping for a stable code.
#[must_use]
pub const fn http_status(code: ErrorCode) -> u16 {
    match code {
        ErrorCode::InstallEndpointInvalidIp
        | ErrorCode::InstallEndpointInvalidPort
        | ErrorCode::ProtocolFrameTooLarge
        | ErrorCode::ProtocolInvalidEnvelope
        | ErrorCode::EnrollmentCsrInvalid
        | ErrorCode::CommandIdInvalid => 400,
        ErrorCode::ProtocolAnonymousClient => 401,
        ErrorCode::SessionIneligible => 403,
        ErrorCode::ProvisioningWindowClosed
        | ErrorCode::GatewayCertLocalKeyMismatch
        | ErrorCode::SessionChanged
        | ErrorCode::SessionAmbiguous
        | ErrorCode::SessionAgentDuplicate
        | ErrorCode::SessionAutostartShadowed
        | ErrorCode::SessionUiPresentedUnfocused
        | ErrorCode::SessionLockUnsupported
        | ErrorCode::SessionUnlockUnsupported
        | ErrorCode::StaleLockEpoch
        | ErrorCode::LockCommandMismatch
        | ErrorCode::NoActiveLock
        | ErrorCode::HomeTransition
        | ErrorCode::CommandRequestConflict => 409,
        ErrorCode::GatewayCertIssuerUnavailable
        | ErrorCode::SessionAgentMissing
        | ErrorCode::SessionDisplayUnavailable
        | ErrorCode::SessionDisplayLost => 503,
        ErrorCode::GatewayCertProfileInvalid
        | ErrorCode::GatewayCertInstallFailed
        | ErrorCode::VaultCorrupt
        | ErrorCode::PackageLayoutInvalid => 500,
    }
}

/// Builds a safe Problem Details payload with no source detail by default.
#[must_use]
pub fn to_problem_details(code: ErrorCode, correlation_id: Uuid) -> ProblemDetails {
    ProblemDetails {
        type_uri: format!("urn:natsume:error:{}", code.as_str()),
        title: code.public_title().to_owned(),
        status: http_status(code),
        code: code.as_str().to_owned(),
        detail: None,
        correlation_id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ALL_ERROR_CODES;

    #[test]
    fn command_id_errors_have_their_contractual_http_statuses() {
        assert_eq!(http_status(ErrorCode::CommandIdInvalid), 400);
        assert_eq!(http_status(ErrorCode::CommandRequestConflict), 409);
    }

    #[test]
    fn every_code_has_a_safe_explicit_problem_details_mapping() {
        for code in ALL_ERROR_CODES {
            let problem = to_problem_details(code, Uuid::nil());

            assert!((400..600).contains(&problem.status()));
            assert_eq!(problem.status(), http_status(code));
            assert_eq!(problem.code(), code.as_str());
            assert_eq!(
                problem.type_uri(),
                format!("urn:natsume:error:{}", code.as_str())
            );
            assert_eq!(problem.title(), code.public_title());
            assert!(problem.detail().is_none());
            assert_eq!(problem.correlation_id(), Uuid::nil());
        }
    }
}
