use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
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

#[derive(Debug, Clone, Copy, Deserialize, Serialize, specta::Type)]
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
#[derive(Debug, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(crate) struct OperatorRoleRequest {
    pub(super) role: OperatorRoleValue,
}

#[derive(Debug, Serialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(crate) struct OperatorAccountResponse {
    pub(super) operator_id: Uuid,
    pub(super) username: String,
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

#[derive(Debug, Serialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(crate) struct OperatorInvitationResponse {
    pub(super) invitation_id: Uuid,
    pub(super) role: OperatorRoleValue,
    pub(super) issuer_operator_id: Uuid,
    #[specta(type = specta_typescript::Number)]
    pub(super) created_at_unix_ms: i64,
    #[specta(type = specta_typescript::Number)]
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
#[derive(Debug, Serialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(crate) struct OperatorInvitationIssuedResponse {
    pub(super) invitation: OperatorInvitationResponse,
    #[specta(type = String)]
    pub(super) token: SecretValue,
}

#[derive(Debug, Serialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(crate) struct OperatorPasswordResetIssuedResponse {
    pub(super) operator_id: Uuid,
    pub(super) reset_id: Uuid,
    pub(super) username: String,
    #[specta(type = specta_typescript::Number)]
    pub(super) expires_at_unix_ms: i64,
    #[specta(type = String)]
    pub(super) token: SecretValue,
}

/// Token stays in the JSON body, never in an API path or query.
#[derive(Debug, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(crate) struct OperatorTokenRequest {
    #[specta(type = String)]
    pub(super) token: SecretValue,
}

#[derive(Debug, Serialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(crate) struct OperatorRegistrationInspectionResponse {
    pub(super) role: OperatorRoleValue,
}

#[derive(Debug, Serialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(crate) struct OperatorPasswordResetInspectionResponse {
    pub(super) username: String,
}

#[derive(Debug, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(crate) struct OperatorRegistrationRequest {
    #[specta(type = String)]
    pub(super) token: SecretValue,
    /// 1–128 UTF-8 bytes; case-sensitive, unique, without surrounding whitespace.
    pub(super) username: String,
    /// 16–1024 ASCII characters: letters, digits, !@#$%^&*()-_=+[]{};:,.?/~.
    /// Requires a digit and a special character. Whitespace is forbidden.
    #[specta(type = String)]
    pub(super) password: SecretValue,
    /// Must equal password exactly; no normalization.
    #[specta(type = String)]
    pub(super) password_confirmation: SecretValue,
}

#[derive(Debug, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(crate) struct OperatorPasswordResetRequest {
    #[specta(type = String)]
    pub(super) token: SecretValue,
    /// 16–1024 ASCII characters, using the same policy as registration.
    #[specta(type = String)]
    pub(super) password: SecretValue,
    #[specta(type = String)]
    pub(super) password_confirmation: SecretValue,
}

#[derive(Debug, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(crate) struct OperatorPasswordChangeRequest {
    /// Existing passwords retain their legacy format; at most 1024 UTF-8 bytes.
    #[specta(type = String)]
    pub(super) current_password: SecretValue,
    /// 16–1024 ASCII characters, using the same policy as registration.
    #[specta(type = String)]
    pub(super) password: SecretValue,
    #[specta(type = String)]
    pub(super) password_confirmation: SecretValue,
}

#[derive(Deserialize)]
pub(crate) struct OperatorAccountPath {
    /// Canonical lowercase hyphenated `UUIDv7`.
    pub(super) operator_id: String,
}

#[derive(Deserialize)]
pub(crate) struct OperatorInvitationPath {
    /// Canonical lowercase hyphenated `UUIDv7`.
    pub(super) invite_id: String,
}
