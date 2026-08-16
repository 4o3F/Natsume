use serde::{Deserialize, Serialize};
use serde_json::{Value, value::RawValue};
use sha2::{Digest, Sha256};
use snafu::Snafu;
use uuid::Uuid;

use crate::{
    audit::{AuditEvent, AuditEventId, CorrelationId},
    db::{
        self, Database,
        command::{InsertCommandOutcome, NewCommand},
    },
};

pub(crate) const COMMAND_REQUEST_FINGERPRINT_DOMAIN: &[u8] = b"natsume:command-request:v1";
pub(crate) const REQUEST_FINGERPRINT_VERSION: i32 = 1;

const PAYLOAD_VERSION: i32 = 1;
const MAX_JCS_SAFE_U64: u64 = 9_007_199_254_740_991;
const MAX_JCS_SAFE_I64: i64 = 9_007_199_254_740_991;

pub(crate) struct CommandId(Uuid);

impl CommandId {
    pub(crate) fn parse(value: &str) -> Result<Self, CommandError> {
        parse_canonical_uuid_v7(value)
            .map(Self)
            .map_err(|()| CommandError::CommandIdInvalid)
    }

    pub(crate) const fn value(&self) -> Uuid {
        self.0
    }

    fn as_text(&self) -> String {
        self.0.to_string()
    }
}

pub(crate) struct CommandRequestInput {
    pub(crate) device_id: String,
    pub(crate) kind: String,
    pub(crate) payload_version: i32,
    pub(crate) payload: Box<RawValue>,
    pub(crate) reason_code: Option<String>,
    pub(crate) group_correlation_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandOutcome {
    Created,
    Replayed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandKind {
    SyncState,
    SyncSecret,
    OpenBindingPrompt,
    LockSession,
    UnlockSession,
    TerminateSession,
    ResetHome,
}

impl CommandKind {
    fn parse(value: &str) -> Result<Self, CommandError> {
        match value {
            "sync_state" => Ok(Self::SyncState),
            "sync_secret" => Ok(Self::SyncSecret),
            "open_binding_prompt" => Ok(Self::OpenBindingPrompt),
            "lock_session" => Ok(Self::LockSession),
            "unlock_session" => Ok(Self::UnlockSession),
            "terminate_session" => Ok(Self::TerminateSession),
            "reset_home" => Ok(Self::ResetHome),
            _ => Err(CommandError::KindInvalid),
        }
    }

    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::SyncState => "sync_state",
            Self::SyncSecret => "sync_secret",
            Self::OpenBindingPrompt => "open_binding_prompt",
            Self::LockSession => "lock_session",
            Self::UnlockSession => "unlock_session",
            Self::TerminateSession => "terminate_session",
            Self::ResetHome => "reset_home",
        }
    }
}

struct ValidatedCommandRequest {
    device_id: String,
    kind: CommandKind,
    payload_version: i32,
    frozen_payload_json: String,
    request_fingerprint_sha256: Vec<u8>,
    group_correlation_id: Option<String>,
}

pub(crate) async fn put_command(
    database: &Database,
    command_id: &CommandId,
    input: CommandRequestInput,
    correlation_id: CorrelationId,
) -> Result<CommandOutcome, CommandError> {
    let request = validate_request(input)?;
    let Some(device_pk) = db::command::find_device_pk(database, &request.device_id).await? else {
        return Err(CommandError::DeviceNotFound);
    };
    let command_id_text = command_id.as_text();
    if let Some(existing) = db::command::find_command(database, &command_id_text).await? {
        return classify_existing(database, command_id, &request, &existing, correlation_id).await;
    }

    let event = AuditEvent::command_created(
        AuditEventId::from_uuid(Uuid::now_v7()),
        correlation_id,
        command_id.value(),
        request.group_correlation_id.clone(),
        request.kind.as_str(),
        request.payload_version,
        REQUEST_FINGERPRINT_VERSION,
    );
    let outcome = db::command::insert_command_with_created_audit(
        database,
        NewCommand {
            command_id: command_id_text.clone(),
            device_pk,
            kind: request.kind.as_str(),
            request_fingerprint_version: REQUEST_FINGERPRINT_VERSION,
            request_fingerprint_sha256: request.request_fingerprint_sha256.clone(),
            group_correlation_id: request.group_correlation_id.clone(),
            payload_version: request.payload_version,
            frozen_payload_json: request.frozen_payload_json.clone(),
        },
        event,
    )
    .await?;
    match outcome {
        InsertCommandOutcome::Inserted => Ok(CommandOutcome::Created),
        // A concurrent PUT with the same new command_id won the race; a legitimate
        // idempotent retry must still resolve to replay (or conflict), never 500.
        InsertCommandOutcome::CommandIdExists => {
            let Some(existing) = db::command::find_command(database, &command_id_text).await?
            else {
                return Err(CommandError::PersistenceFailed);
            };
            classify_existing(database, command_id, &request, &existing, correlation_id).await
        }
    }
}

async fn classify_existing(
    database: &Database,
    command_id: &CommandId,
    request: &ValidatedCommandRequest,
    existing: &db::command::PersistedCommandRequest,
    correlation_id: CorrelationId,
) -> Result<CommandOutcome, CommandError> {
    if existing.request_fingerprint_version == REQUEST_FINGERPRINT_VERSION
        && existing.request_fingerprint_sha256 == request.request_fingerprint_sha256
    {
        return Ok(CommandOutcome::Replayed);
    }
    let event = AuditEvent::command_request_conflict(
        AuditEventId::from_uuid(Uuid::now_v7()),
        correlation_id,
        command_id.value(),
        request.group_correlation_id.clone(),
        REQUEST_FINGERPRINT_VERSION,
    );
    db::command::insert_command_conflict_audit(database, event).await?;
    Err(CommandError::RequestConflict)
}

fn validate_request(input: CommandRequestInput) -> Result<ValidatedCommandRequest, CommandError> {
    if input.payload_version != PAYLOAD_VERSION {
        return Err(CommandError::PayloadInvalid);
    }
    parse_canonical_uuid_v7(&input.device_id).map_err(|()| CommandError::DeviceIdInvalid)?;
    let kind = CommandKind::parse(&input.kind)?;
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
        &input.device_id,
        kind,
        input.payload_version,
        &payload,
        input.reason_code.as_deref(),
        input.group_correlation_id.as_deref(),
    )?;
    Ok(ValidatedCommandRequest {
        device_id: input.device_id,
        kind,
        payload_version: input.payload_version,
        frozen_payload_json,
        request_fingerprint_sha256,
        group_correlation_id: input.group_correlation_id,
    })
}

#[derive(Serialize)]
struct FingerprintInput<'a> {
    device_id: &'a str,
    kind: &'static str,
    payload_version: i32,
    payload: &'a Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason_code: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    group_correlation_id: Option<&'a str>,
}

fn fingerprint_v1(
    device_id: &str,
    kind: CommandKind,
    payload_version: i32,
    payload: &Value,
    reason_code: Option<&str>,
    group_correlation_id: Option<&str>,
) -> Result<Vec<u8>, CommandError> {
    let canonical = serde_json_canonicalizer::to_vec(&FingerprintInput {
        device_id,
        kind: kind.as_str(),
        payload_version,
        payload,
        reason_code,
        group_correlation_id,
    })
    .map_err(|_| CommandError::CanonicalizationFailed)?;
    let mut hasher = Sha256::new();
    hasher.update(COMMAND_REQUEST_FINGERPRINT_DOMAIN);
    hasher.update([0]);
    hasher.update(canonical);
    Ok(hasher.finalize().to_vec())
}

fn validate_payload(kind: CommandKind, raw: &RawValue) -> Result<Value, CommandError> {
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

trait ValidatePayload {
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
struct SyncStatePayload {
    generation: u64,
    canonical_hash: String,
    snapshot: TargetStateSnapshotPayload,
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
struct TargetStateSnapshotPayload {
    schema_version: u32,
    assignment: TargetAssignmentPayload,
    gateway: TargetGatewayPayload,
    session: TargetSessionPayload,
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
struct TargetAssignmentPayload {
    binding_revision: u64,
    seat_id: String,
    seat_code: String,
    account_id: String,
    domjudge_username: String,
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
struct TargetGatewayPayload {
    gateway_configuration_revision: u64,
    local_origin_hostname: String,
    fixed_upstream_profile_id: String,
    exact_login_policy_id: String,
    gateway_certificate_profile_id: String,
    gateway_certificate_min_valid_until_unix_ms: i64,
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
struct TargetSessionPayload {
    browser_policy_revision: String,
    home_template_revision: String,
}

impl ValidatePayload for TargetSessionPayload {
    fn validate(&self) -> bool {
        is_printable_ascii(&self.browser_policy_revision, 128)
            && is_printable_ascii(&self.home_template_revision, 128)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SyncSecretPayload {
    seat_id: String,
    binding_revision: u64,
    account_id: String,
    credential_revision: u64,
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
struct OpenBindingPromptPayload {
    expires_at_unix_ms: i64,
    prompt_message_id: String,
}

impl ValidatePayload for OpenBindingPromptPayload {
    fn validate(&self) -> bool {
        is_safe_unix_ms(self.expires_at_unix_ms) && is_printable_ascii(&self.prompt_message_id, 128)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionTargetPayload {
    session_instance_id: String,
    session_epoch: u64,
}

impl ValidatePayload for SessionTargetPayload {
    fn validate(&self) -> bool {
        is_printable_ascii(&self.session_instance_id, 128) && is_safe_u64(self.session_epoch)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LockSessionPayload {
    target: SessionTargetPayload,
    requested_lock_epoch: u64,
}

impl ValidatePayload for LockSessionPayload {
    fn validate(&self) -> bool {
        self.target.validate() && is_safe_u64(self.requested_lock_epoch)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UnlockSessionPayload {
    target: SessionTargetPayload,
    expected_lock_epoch: u64,
    expected_lock_command_id: String,
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
struct TerminateSessionPayload {
    target: SessionTargetPayload,
}

impl ValidatePayload for TerminateSessionPayload {
    fn validate(&self) -> bool {
        self.target.validate()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResetHomePayload {
    home_template_revision: String,
    home_epoch: u64,
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

fn parse_canonical_uuid_v7(value: &str) -> Result<Uuid, ()> {
    let parsed = parse_canonical_uuid(value)?;
    (parsed.get_version_num() == 7).then_some(parsed).ok_or(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Snafu)]
pub(crate) enum CommandError {
    #[snafu(display("the Command ID is invalid"))]
    CommandIdInvalid,
    #[snafu(display("the Command request body is invalid"))]
    RequestInvalid,
    #[snafu(display("the Device ID is invalid"))]
    DeviceIdInvalid,
    #[snafu(display("the Command kind is invalid"))]
    KindInvalid,
    #[snafu(display("the Command payload is invalid"))]
    PayloadInvalid,
    #[snafu(display("the Command reason code is invalid"))]
    ReasonCodeInvalid,
    #[snafu(display("the Command group correlation ID is invalid"))]
    GroupCorrelationIdInvalid,
    #[snafu(display("the Device does not exist"))]
    DeviceNotFound,
    #[snafu(display("the Command request conflicts with persisted facts"))]
    RequestConflict,
    #[snafu(display("the Command request could not be canonicalized"))]
    CanonicalizationFailed,
    #[snafu(display("Command persistence failed"))]
    PersistenceFailed,
}

impl From<db::command::CommandStoreError> for CommandError {
    fn from(_source: db::command::CommandStoreError) -> Self {
        Self::PersistenceFailed
    }
}

#[cfg(test)]
mod tests;
