use serde::Serialize;
use snafu::Snafu;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{
    audit::CorrelationId,
    db::{self, Database},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub(crate) enum DeviceState {
    Enrolled,
    Revoked,
    Disabled,
}

impl DeviceState {
    pub(crate) fn from_persisted(value: &str) -> Result<Self, ContestError> {
        match value {
            "enrolled" => Ok(Self::Enrolled),
            "revoked" => Ok(Self::Revoked),
            "disabled" => Ok(Self::Disabled),
            _ => Err(ContestError::InvalidPersistedFacts),
        }
    }

    pub(crate) const fn as_persisted(self) -> &'static str {
        match self {
            Self::Enrolled => "enrolled",
            Self::Revoked => "revoked",
            Self::Disabled => "disabled",
        }
    }
}

// The closed `devices.hardware_identity_quality` vocabulary frozen by the
// migration's CHECK constraint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub(crate) enum HardwareIdentityQuality {
    Strong,
    Medium,
    Weak,
}

impl HardwareIdentityQuality {
    pub(crate) fn from_persisted(value: &str) -> Result<Self, ContestError> {
        match value {
            "strong" => Ok(Self::Strong),
            "medium" => Ok(Self::Medium),
            "weak" => Ok(Self::Weak),
            _ => Err(ContestError::InvalidPersistedFacts),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeviceLifecycleAction {
    Revoke,
    Disable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DeviceLifecycleFacts {
    pub(crate) state: DeviceState,
    pub(crate) token_count: i64,
    pub(crate) non_revoked_certificate_count: i64,
}

/// The whole lifecycle transition: whether it writes at all, the state the
/// Device ends in, and the exact row counts the mutation must observe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DeviceLifecycleOutcome {
    pub(crate) applies: bool,
    pub(crate) resulting_state: DeviceState,
    pub(crate) removed_token_count: i64,
    pub(crate) revoked_certificate_count: i64,
}

/// Decides a Device lifecycle transition from current Server facts only.
#[must_use]
pub(crate) const fn decide_device_lifecycle(
    action: DeviceLifecycleAction,
    current: DeviceLifecycleFacts,
) -> DeviceLifecycleOutcome {
    match action {
        DeviceLifecycleAction::Revoke => {
            if matches!(current.state, DeviceState::Revoked)
                && current.token_count == 0
                && current.non_revoked_certificate_count == 0
            {
                DeviceLifecycleOutcome {
                    applies: false,
                    resulting_state: DeviceState::Revoked,
                    removed_token_count: 0,
                    revoked_certificate_count: 0,
                }
            } else {
                DeviceLifecycleOutcome {
                    applies: true,
                    resulting_state: DeviceState::Revoked,
                    removed_token_count: current.token_count,
                    revoked_certificate_count: current.non_revoked_certificate_count,
                }
            }
        }
        DeviceLifecycleAction::Disable => match current.state {
            DeviceState::Enrolled => DeviceLifecycleOutcome {
                applies: true,
                resulting_state: DeviceState::Disabled,
                removed_token_count: 0,
                revoked_certificate_count: 0,
            },
            DeviceState::Disabled | DeviceState::Revoked => DeviceLifecycleOutcome {
                applies: false,
                resulting_state: current.state,
                removed_token_count: 0,
                revoked_certificate_count: 0,
            },
        },
    }
}

pub(crate) struct DeviceId(Uuid);

impl DeviceId {
    pub(crate) fn parse(value: &str) -> Result<Self, ContestError> {
        let parsed = Uuid::parse_str(value).map_err(|_| ContestError::InvalidDeviceId)?;
        if parsed.get_version_num() != 7 || parsed.hyphenated().to_string() != value {
            return Err(ContestError::InvalidDeviceId);
        }
        Ok(Self(parsed))
    }

    pub(crate) fn as_text(&self) -> String {
        self.0.hyphenated().to_string()
    }
}

// The four read value objects below have transport shapes identical to their
// application shapes, so the schema derives live here and the handler
// serializes them directly. The published component names stay `*Response`.
#[derive(Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = SeatResponse)]
pub(crate) struct SeatFacts {
    seat_id: String,
    seat_code: String,
}

impl SeatFacts {
    pub(crate) fn new(seat_id: String, seat_code: String) -> Self {
        Self { seat_id, seat_code }
    }
}

#[derive(Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = AccountResponse)]
pub(crate) struct AccountFacts {
    account_id: String,
    domjudge_username: String,
    credential_revision: i64,
}

impl AccountFacts {
    pub(crate) fn new(
        account_id: String,
        domjudge_username: String,
        credential_revision: i64,
    ) -> Self {
        Self {
            account_id,
            domjudge_username,
            credential_revision,
        }
    }
}

#[derive(Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = DeviceResponse)]
pub(crate) struct DeviceFacts {
    device_id: String,
    #[schema(inline)]
    state: DeviceState,
    #[schema(inline)]
    hardware_identity_quality: HardwareIdentityQuality,
}

impl DeviceFacts {
    pub(crate) fn new(
        device_id: String,
        state: DeviceState,
        hardware_identity_quality: HardwareIdentityQuality,
    ) -> Self {
        Self {
            device_id,
            state,
            hardware_identity_quality,
        }
    }
}

#[derive(Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = BindingResponse)]
pub(crate) struct BindingFacts {
    seat_id: String,
    device_id: String,
    binding_revision: i64,
}

impl BindingFacts {
    pub(crate) fn new(seat_id: String, device_id: String, binding_revision: i64) -> Self {
        Self {
            seat_id,
            device_id,
            binding_revision,
        }
    }
}

/// Reads the current Seat set in deterministic natural-key order.
///
/// # Errors
///
/// Returns a redacted [`ContestError`] when persistence fails.
pub(crate) async fn list_seats(database: &Database) -> Result<Vec<SeatFacts>, ContestError> {
    db::contest::list_seats(database).await
}

/// Reads the current Account set without secret-storage pointers.
///
/// # Errors
///
/// Returns a redacted [`ContestError`] when persistence fails.
pub(crate) async fn list_accounts(database: &Database) -> Result<Vec<AccountFacts>, ContestError> {
    db::contest::list_accounts(database).await
}

/// Reads the current Device set without Machine Hardware IDs.
///
/// # Errors
///
/// Returns a redacted [`ContestError`] when persistence fails or a persisted
/// vocabulary value is outside its frozen set.
pub(crate) async fn list_devices(database: &Database) -> Result<Vec<DeviceFacts>, ContestError> {
    db::contest::list_devices(database).await
}

/// Reads the current Seat-to-Device Binding set in Seat-key order.
///
/// # Errors
///
/// Returns a redacted [`ContestError`] when persistence fails.
pub(crate) async fn list_bindings(database: &Database) -> Result<Vec<BindingFacts>, ContestError> {
    db::contest::list_bindings(database).await
}

pub(crate) async fn revoke_device(
    database: &Database,
    device_id: &DeviceId,
    correlation_id: CorrelationId,
) -> Result<(), ContestError> {
    db::contest::apply_device_lifecycle(
        database,
        device_id,
        DeviceLifecycleAction::Revoke,
        correlation_id,
    )
    .await
}

pub(crate) async fn disable_device(
    database: &Database,
    device_id: &DeviceId,
    correlation_id: CorrelationId,
) -> Result<(), ContestError> {
    db::contest::apply_device_lifecycle(
        database,
        device_id,
        DeviceLifecycleAction::Disable,
        correlation_id,
    )
    .await
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Snafu)]
pub(crate) enum ContestError {
    #[snafu(display("the Device ID is invalid"))]
    InvalidDeviceId,
    #[snafu(display("the Device does not exist"))]
    DeviceNotFound,
    #[snafu(display("persisted Device facts are invalid"))]
    InvalidPersistedFacts,
    #[snafu(display("contest current facts could not be read"))]
    PersistenceFailed,
}
