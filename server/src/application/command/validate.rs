use serde::Deserialize;
use serde_json::{Value, value::RawValue};
use uuid::Uuid;

use crate::application::device::DeviceId;

use super::types::{
    CommandError, CommandKind, CommandRequestFingerprint, CommandRequestInput,
    REQUEST_FINGERPRINT_VERSION, ValidatedCommandRequest, parse_canonical_uuid_v7,
};

mod fingerprint;

pub(super) use self::fingerprint::fingerprint_v1;

pub(super) const PAYLOAD_VERSION: i32 = 1;
const MAX_JCS_SAFE_U64: u64 = 9_007_199_254_740_991;
const MAX_JCS_SAFE_I64: i64 = 9_007_199_254_740_991;

pub(super) fn validate_request(
    input: CommandRequestInput,
) -> Result<ValidatedCommandRequest, CommandError> {
    if input.payload_version != PAYLOAD_VERSION {
        return Err(CommandError::PayloadInvalid);
    }
    let device_id = DeviceId::parse(&input.device_id).ok_or(CommandError::DeviceIdInvalid)?;
    let kind = CommandKind::parse_request(&input.kind)?;
    if input
        .reason_code
        .as_deref()
        .is_some_and(|value| !is_printable_ascii(value, 64))
    {
        return Err(CommandError::ReasonCodeInvalid);
    }
    if input
        .group_correlation_id
        .as_deref()
        .is_some_and(|value| parse_canonical_uuid(value).is_err())
    {
        return Err(CommandError::GroupCorrelationIdInvalid);
    }
    let payload = validate_payload(kind, &input.payload)?;
    let frozen_payload_json = serde_json_canonicalizer::to_string(&payload)
        .map_err(|_| CommandError::CanonicalizationFailed)?;
    let request_fingerprint_sha256 = fingerprint_v1(
        &device_id.as_text(),
        kind,
        input.payload_version,
        &payload,
        input.reason_code.as_deref(),
        input.group_correlation_id.as_deref(),
    )?;
    Ok(ValidatedCommandRequest {
        device_id,
        kind,
        payload_version: input.payload_version,
        frozen_payload_json,
        fingerprint: CommandRequestFingerprint {
            version: REQUEST_FINGERPRINT_VERSION,
            sha256: request_fingerprint_sha256,
        },
        group_correlation_id: input.group_correlation_id,
    })
}

pub(super) fn validate_payload(kind: CommandKind, raw: &RawValue) -> Result<Value, CommandError> {
    match kind {
        CommandKind::SyncState => deserialize_and_validate::<SyncStatePayload>(raw)?,
        CommandKind::SyncSecret => deserialize_and_validate::<SyncSecretPayload>(raw)?,
        CommandKind::OpenBindingPrompt => {
            deserialize_and_validate::<OpenBindingPromptPayload>(raw)?;
        }
        CommandKind::LockSession => deserialize_and_validate::<LockSessionPayload>(raw)?,
        CommandKind::UnlockSession => deserialize_and_validate::<UnlockSessionPayload>(raw)?,
        CommandKind::TerminateSession => {
            deserialize_and_validate::<TerminateSessionPayload>(raw)?;
        }
        CommandKind::ResetHome => deserialize_and_validate::<ResetHomePayload>(raw)?,
    }
    serde_json::from_str(raw.get()).map_err(|_| CommandError::PayloadInvalid)
}

pub(super) trait ValidatePayload {
    fn validate(&self) -> bool;
}

fn deserialize_and_validate<T>(raw: &RawValue) -> Result<(), CommandError>
where
    T: for<'de> Deserialize<'de> + ValidatePayload,
{
    let payload = serde_json::from_str::<T>(raw.get()).map_err(|_| CommandError::PayloadInvalid)?;
    if !payload.validate() {
        return Err(CommandError::PayloadInvalid);
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SyncStatePayload {
    pub(super) generation: u64,
    pub(super) canonical_hash: String,
    pub(super) snapshot: TargetStateSnapshotPayload,
}

impl ValidatePayload for SyncStatePayload {
    fn validate(&self) -> bool {
        is_safe_u64(self.generation)
            && is_lower_hex_32(&self.canonical_hash)
            && self.snapshot.validate()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TargetStateSnapshotPayload {
    pub(super) schema_version: u32,
    pub(super) assignment: TargetAssignmentPayload,
    pub(super) gateway: TargetGatewayPayload,
    pub(super) session: TargetSessionPayload,
}

impl ValidatePayload for TargetStateSnapshotPayload {
    fn validate(&self) -> bool {
        // Version fields are 1-based across this contract (payload_version and
        // request_fingerprint_version carry the same >= 1 domain).
        self.schema_version >= 1
            && self.assignment.validate()
            && self.gateway.validate()
            && self.session.validate()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TargetAssignmentPayload {
    pub(super) binding_revision: u64,
    pub(super) seat_id: String,
    pub(super) seat_code: String,
    pub(super) account_id: String,
    pub(super) domjudge_username: String,
}

impl ValidatePayload for TargetAssignmentPayload {
    fn validate(&self) -> bool {
        is_safe_u64(self.binding_revision)
            && parse_canonical_uuid(&self.seat_id).is_ok()
            && is_printable_ascii(&self.seat_code, 128)
            && parse_canonical_uuid(&self.account_id).is_ok()
            && is_printable_ascii(&self.domjudge_username, 128)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TargetGatewayPayload {
    pub(super) gateway_configuration_revision: u64,
    pub(super) local_origin_hostname: String,
    pub(super) fixed_upstream_profile_id: String,
    pub(super) exact_login_policy_id: String,
    pub(super) gateway_certificate_profile_id: String,
    pub(super) gateway_certificate_min_valid_until_unix_ms: i64,
}

impl ValidatePayload for TargetGatewayPayload {
    fn validate(&self) -> bool {
        is_safe_u64(self.gateway_configuration_revision)
            && is_printable_ascii(&self.local_origin_hostname, 128)
            && is_printable_ascii(&self.fixed_upstream_profile_id, 128)
            && is_printable_ascii(&self.exact_login_policy_id, 128)
            && is_printable_ascii(&self.gateway_certificate_profile_id, 128)
            && is_safe_unix_ms(self.gateway_certificate_min_valid_until_unix_ms)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TargetSessionPayload {
    pub(super) browser_policy_revision: String,
    pub(super) home_template_revision: String,
}

impl ValidatePayload for TargetSessionPayload {
    fn validate(&self) -> bool {
        is_printable_ascii(&self.browser_policy_revision, 128)
            && is_printable_ascii(&self.home_template_revision, 128)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SyncSecretPayload {
    pub(super) seat_id: String,
    pub(super) binding_revision: u64,
    pub(super) account_id: String,
    pub(super) credential_revision: u64,
}

impl ValidatePayload for SyncSecretPayload {
    fn validate(&self) -> bool {
        parse_canonical_uuid(&self.seat_id).is_ok()
            && is_safe_u64(self.binding_revision)
            && parse_canonical_uuid(&self.account_id).is_ok()
            && is_safe_u64(self.credential_revision)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct OpenBindingPromptPayload {
    pub(super) expires_at_unix_ms: i64,
    pub(super) prompt_message_id: String,
}

impl ValidatePayload for OpenBindingPromptPayload {
    fn validate(&self) -> bool {
        is_safe_unix_ms(self.expires_at_unix_ms) && is_printable_ascii(&self.prompt_message_id, 128)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SessionTargetPayload {
    pub(super) session_instance_id: String,
    pub(super) session_epoch: u64,
}

impl ValidatePayload for SessionTargetPayload {
    fn validate(&self) -> bool {
        is_printable_ascii(&self.session_instance_id, 128) && is_safe_u64(self.session_epoch)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LockSessionPayload {
    pub(super) target: SessionTargetPayload,
    pub(super) requested_lock_epoch: u64,
}

impl ValidatePayload for LockSessionPayload {
    fn validate(&self) -> bool {
        self.target.validate() && is_safe_u64(self.requested_lock_epoch)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct UnlockSessionPayload {
    pub(super) target: SessionTargetPayload,
    pub(super) expected_lock_epoch: u64,
    pub(super) expected_lock_command_id: String,
}

impl ValidatePayload for UnlockSessionPayload {
    fn validate(&self) -> bool {
        self.target.validate()
            && is_safe_u64(self.expected_lock_epoch)
            && parse_canonical_uuid_v7(&self.expected_lock_command_id).is_ok()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TerminateSessionPayload {
    pub(super) target: SessionTargetPayload,
}

impl ValidatePayload for TerminateSessionPayload {
    fn validate(&self) -> bool {
        self.target.validate()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ResetHomePayload {
    pub(super) home_template_revision: String,
    pub(super) home_epoch: u64,
}

impl ValidatePayload for ResetHomePayload {
    fn validate(&self) -> bool {
        is_printable_ascii(&self.home_template_revision, 128) && is_safe_u64(self.home_epoch)
    }
}

const fn is_safe_u64(value: u64) -> bool {
    value <= MAX_JCS_SAFE_U64
}

/// Millisecond timestamps share the JCS/ES6 safe-integer bound: a larger value would be
/// silently rounded during canonicalization, breaking "the validated canonical form is the
/// storage form".
const fn is_safe_unix_ms(value: i64) -> bool {
    value > 0 && value <= MAX_JCS_SAFE_I64
}

fn is_lower_hex_32(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn is_printable_ascii(value: &str, maximum_length: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum_length
        && value.bytes().all(|byte| byte.is_ascii_graphic())
}

fn parse_canonical_uuid(value: &str) -> Result<Uuid, ()> {
    let parsed = Uuid::parse_str(value).map_err(|_| ())?;
    (parsed.hyphenated().to_string() == value)
        .then_some(parsed)
        .ok_or(())
}
