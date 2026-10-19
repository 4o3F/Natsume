use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::component::operator::{InvitationSummary, OperatorRole, OperatorSummary};

/// Secret fields redact Debug and zeroize on drop, including partial JSON errors.
#[derive(Debug)]
pub(super) struct SecretValue(SecretString);

impl SecretValue {
    pub(super) fn new(value: &str) -> Self {
        Self(value.to_owned().into())
    }
    pub(super) fn into_string(self) -> String {
        self.0.expose_secret().to_owned()
    }
}

impl<'de> Deserialize<'de> for SecretValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(|value| Self(value.into()))
    }
}

impl Serialize for SecretValue {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.0.expose_secret())
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub(crate) enum OperatorRoleValue {
    Admin,
    Viewer,
}

impl From<OperatorRole> for OperatorRoleValue {
    fn from(role: OperatorRole) -> Self {
        match role {
            OperatorRole::Admin => Self::Admin,
            OperatorRole::Viewer => Self::Viewer,
        }
    }
}
impl From<OperatorRoleValue> for OperatorRole {
    fn from(role: OperatorRoleValue) -> Self {
        match role {
            OperatorRoleValue::Admin => Self::Admin,
            OperatorRoleValue::Viewer => Self::Viewer,
        }
    }
}

/// The sole mutable account field, also used to choose an invitation's fixed role.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct OperatorRoleRequest {
    #[schema(inline)]
    pub(super) role: OperatorRoleValue,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct OperatorAccountResponse {
    pub(super) operator_id: Uuid,
    pub(super) username: String,
    #[schema(inline)]
    pub(super) role: OperatorRoleValue,
}
impl From<OperatorSummary> for OperatorAccountResponse {
    fn from(account: OperatorSummary) -> Self {
        Self {
            operator_id: account.operator_id,
            username: account.username,
            role: account.role.into(),
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct OperatorInvitationResponse {
    pub(super) invitation_id: Uuid,
    #[schema(inline)]
    pub(super) role: OperatorRoleValue,
    pub(super) issuer_operator_id: Uuid,
    pub(super) created_at_unix_ms: i64,
    pub(super) expires_at_unix_ms: i64,
    pub(super) expired: bool,
}
impl OperatorInvitationResponse {
    pub(super) fn new(invitation: &InvitationSummary, now: i64) -> Self {
        Self {
            invitation_id: invitation.invitation_id,
            role: invitation.role.into(),
            issuer_operator_id: invitation.issuer_operator_id,
            created_at_unix_ms: invitation.created_at_unix_ms,
            expires_at_unix_ms: invitation.expires_at_unix_ms,
            expired: invitation.expires_at_unix_ms <= now,
        }
    }
}

/// Only creation/regeneration can return the plaintext authorization.
#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct OperatorInvitationIssuedResponse {
    pub(super) invitation: OperatorInvitationResponse,
    #[schema(value_type = String, read_only, pattern = "^invite_[0-9a-f]{64}$")]
    pub(super) token: SecretValue,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct OperatorPasswordResetIssuedResponse {
    pub(super) operator_id: Uuid,
    pub(super) reset_id: Uuid,
    pub(super) username: String,
    pub(super) expires_at_unix_ms: i64,
    #[schema(value_type = String, read_only, pattern = "^reset_[0-9a-f]{64}$")]
    pub(super) token: SecretValue,
}

/// Token stays in the JSON body, never in an API path or query.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct OperatorTokenRequest {
    #[schema(value_type = String, write_only)]
    pub(super) token: SecretValue,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct OperatorRegistrationInspectionResponse {
    #[schema(inline)]
    pub(super) role: OperatorRoleValue,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct OperatorPasswordResetInspectionResponse {
    pub(super) username: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct OperatorRegistrationRequest {
    #[schema(value_type = String, write_only, pattern = "^invite_[0-9a-f]{64}$")]
    pub(super) token: SecretValue,
    /// 1–128 UTF-8 bytes; case-sensitive, unique, without surrounding whitespace.
    #[schema(min_length = 1, max_length = 128)]
    pub(super) username: String,
    /// 16–1024 ASCII characters: letters, digits, !@#$%^&*()-_=+[]{};:,.?/~.
    /// Requires a digit and a special character. Whitespace is forbidden.
    #[schema(value_type = String, write_only, format = Password, min_length = 16, max_length = 1024, pattern = r"^(?=.*[0-9])(?=.*[!@#$%^&*()_=+\[\]{};:,.?/~\-])[A-Za-z0-9!@#$%^&*()_=+\[\]{};:,.?/~\-]{16,1024}$")]
    pub(super) password: SecretValue,
    /// Must equal password exactly; no normalization.
    #[schema(value_type = String, write_only, format = Password, min_length = 16, max_length = 1024, pattern = r"^(?=.*[0-9])(?=.*[!@#$%^&*()_=+\[\]{};:,.?/~\-])[A-Za-z0-9!@#$%^&*()_=+\[\]{};:,.?/~\-]{16,1024}$")]
    pub(super) password_confirmation: SecretValue,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct OperatorPasswordResetRequest {
    #[schema(value_type = String, write_only, pattern = "^reset_[0-9a-f]{64}$")]
    pub(super) token: SecretValue,
    /// 16–1024 ASCII characters, using the same policy as registration.
    #[schema(value_type = String, write_only, format = Password, min_length = 16, max_length = 1024, pattern = r"^(?=.*[0-9])(?=.*[!@#$%^&*()_=+\[\]{};:,.?/~\-])[A-Za-z0-9!@#$%^&*()_=+\[\]{};:,.?/~\-]{16,1024}$")]
    pub(super) password: SecretValue,
    #[schema(value_type = String, write_only, format = Password, min_length = 16, max_length = 1024, pattern = r"^(?=.*[0-9])(?=.*[!@#$%^&*()_=+\[\]{};:,.?/~\-])[A-Za-z0-9!@#$%^&*()_=+\[\]{};:,.?/~\-]{16,1024}$")]
    pub(super) password_confirmation: SecretValue,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct OperatorPasswordChangeRequest {
    /// Existing passwords retain their legacy format; at most 1024 UTF-8 bytes.
    #[schema(value_type = String, write_only, format = Password, max_length = 1024)]
    pub(super) current_password: SecretValue,
    /// 16–1024 ASCII characters, using the same policy as registration.
    #[schema(value_type = String, write_only, format = Password, min_length = 16, max_length = 1024, pattern = r"^(?=.*[0-9])(?=.*[!@#$%^&*()_=+\[\]{};:,.?/~\-])[A-Za-z0-9!@#$%^&*()_=+\[\]{};:,.?/~\-]{16,1024}$")]
    pub(super) password: SecretValue,
    #[schema(value_type = String, write_only, format = Password, min_length = 16, max_length = 1024, pattern = r"^(?=.*[0-9])(?=.*[!@#$%^&*()_=+\[\]{};:,.?/~\-])[A-Za-z0-9!@#$%^&*()_=+\[\]{};:,.?/~\-]{16,1024}$")]
    pub(super) password_confirmation: SecretValue,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
pub(crate) struct OperatorAccountPath {
    /// Canonical lowercase hyphenated `UUIDv7`.
    pub(super) operator_id: String,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
pub(crate) struct OperatorInvitationPath {
    /// Canonical lowercase hyphenated `UUIDv7`.
    pub(super) invite_id: String,
}
