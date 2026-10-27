mod db;

use serde::{Deserialize, Serialize};
use snafu::Snafu;
use uuid::Uuid;

use crate::{
    db::{Database, PersistenceError, Transaction, TransactionError},
    device_control::TargetChange,
};

use super::{
    device::{DeviceComponent, DeviceId, DeviceState},
    home::{HomeComponent, HomeError},
    power::{PowerControlComponent, PowerError},
    session::{ForegroundTarget, SessionControlComponent, SessionControlError},
};

/// Owns HTTP target submission receipts and their atomic transaction with Targets.
/// Session/Home retain all validation and mutation of their own tables.
pub(crate) struct TargetSubmissionComponent {
    database: Database,
}

impl TargetSubmissionComponent {
    pub(crate) const fn new(database: Database) -> Self {
        Self { database }
    }

    pub(crate) async fn submit(
        &self,
        operator_id: Uuid,
        mut request: TargetSubmissionRequest,
    ) -> Result<(TargetSubmissionResult, TargetChange), TargetSubmissionError> {
        if let TargetScope::Devices(devices) = &mut request.scope {
            devices.sort_unstable_by_key(DeviceId::as_text);
            devices.dedup();
        }
        let canonical = request.canonical()?;
        self.database
            .write(move |transaction| {
                let operation_id = request.operation_id.hyphenated().to_string();
                let owner = operator_id.hyphenated().to_string();
                // Replay precedes scope selection and current eligibility checks.
                if let Some((stored_owner, stored_request, stored_results)) =
                    db::find(transaction, &operation_id)?
                {
                    if stored_owner != owner || stored_request != canonical {
                        return Err(TargetSubmissionError::OperationIdConflict);
                    }
                    return decode_result(request.operation_id, &stored_results);
                }
                let devices = match request.scope {
                    TargetScope::AllEnabled => {
                        DeviceComponent::enabled_target_devices(transaction)?
                    }
                    TargetScope::Devices(devices) | TargetScope::AllOnlineEnabled(devices) => {
                        devices
                    }
                };
                let mut results = Vec::with_capacity(devices.len());
                for device_id in devices {
                    let rejection = apply(transaction, &device_id, request.action)?;
                    results.push(DeviceSubmissionResult {
                        device_id,
                        rejection,
                    });
                }
                let stored: Vec<_> = results
                    .iter()
                    .map(|row| StoredResult {
                        device_id: row.device_id.as_text(),
                        rejection: row.rejection,
                    })
                    .collect();
                let stored = serde_json::to_string(&stored).map_err(|source| {
                    TargetSubmissionError::PersistenceFailed {
                        source: PersistenceError::Json {
                            field: "target receipt",
                            source,
                        },
                    }
                })?;
                // Even zero devices or all rejected results must reserve this ID.
                db::insert(transaction, &operation_id, &owner, &canonical, &stored)?;
                Ok(TargetSubmissionResult {
                    operation_id: request.operation_id,
                    results,
                })
            })
            .await
            .map_err(TransactionError::into_error)
            .map(|result| {
                let devices = result
                    .results
                    .iter()
                    .filter(|row| row.rejection.is_none())
                    .map(|row| row.device_id)
                    .collect();
                (result, TargetChange::devices(devices))
            })
    }
}

fn decode_result(
    operation_id: Uuid,
    stored: &str,
) -> Result<TargetSubmissionResult, TargetSubmissionError> {
    let rows: Vec<StoredResult> = serde_json::from_str(stored).map_err(|source| {
        TargetSubmissionError::InvalidPersistedFacts {
            source: PersistenceError::Json {
                field: "target receipt",
                source,
            },
        }
    })?;
    let results = rows
        .into_iter()
        .map(|row| {
            let device_id = DeviceId::parse(&row.device_id).ok_or(
                TargetSubmissionError::InvalidPersistedFacts {
                    source: crate::db::PersistenceError::invalid("persisted facts"),
                },
            )?;
            Ok(DeviceSubmissionResult {
                device_id,
                rejection: row.rejection,
            })
        })
        .collect::<Result<_, TargetSubmissionError>>()?;
    Ok(TargetSubmissionResult {
        operation_id,
        results,
    })
}

fn apply(
    transaction: &mut Transaction<'_>,
    device_id: &DeviceId,
    action: TargetAction,
) -> Result<Option<TargetRejection>, TargetSubmissionError> {
    match DeviceComponent::target_device_state(transaction, device_id) {
        Ok(Some(DeviceState::Enabled)) => {}
        Ok(Some(_)) => return Ok(Some(TargetRejection::DeviceNotEnabled)),
        Ok(None) => return Ok(Some(TargetRejection::DeviceNotFound)),
        Err(PersistenceError::InvalidPersistedData { .. }) => {
            return Ok(Some(TargetRejection::InvalidDeviceState));
        }
        Err(error) => return Err(error.into()),
    }
    match action {
        TargetAction::SetForeground(target) => session_result(
            SessionControlComponent::set_foreground_in_transaction(transaction, device_id, target),
        ),
        TargetAction::TerminateSession => session_result(
            SessionControlComponent::terminate_in_transaction(transaction, device_id),
        ),
        TargetAction::ResetHome => {
            match HomeComponent::reset_in_transaction(transaction, device_id) {
                Ok(_) => Ok(None),
                Err(HomeError::DeviceNotFound) => Ok(Some(TargetRejection::DeviceNotFound)),
                Err(HomeError::InvalidPersistedFacts { .. }) => {
                    Ok(Some(TargetRejection::InvalidTarget))
                }
                Err(HomeError::EpochExhausted) => Ok(Some(TargetRejection::EpochExhausted)),
                Err(HomeError::PersistenceFailed { source }) => {
                    Err(TargetSubmissionError::PersistenceFailed { source })
                }
            }
        }
        TargetAction::PowerOff { expires_at_unix_ms } => {
            match PowerControlComponent::request_shutdown_in_transaction(
                transaction,
                device_id,
                expires_at_unix_ms,
            ) {
                Ok(_) => Ok(None),
                Err(PowerError::DeviceNotFound) => Ok(Some(TargetRejection::DeviceNotFound)),
                Err(PowerError::EpochExhausted) => Ok(Some(TargetRejection::EpochExhausted)),
                Err(PowerError::InvalidDeadline | PowerError::InvalidPersistedFacts { .. }) => {
                    Ok(Some(TargetRejection::InvalidTarget))
                }
                Err(PowerError::PersistenceFailed { source }) => {
                    Err(TargetSubmissionError::PersistenceFailed { source })
                }
            }
        }
    }
}

fn session_result(
    result: Result<super::session::SessionControlTarget, SessionControlError>,
) -> Result<Option<TargetRejection>, TargetSubmissionError> {
    match result {
        Ok(_) => Ok(None),
        Err(SessionControlError::DeviceNotFound) => Ok(Some(TargetRejection::DeviceNotFound)),
        Err(SessionControlError::InvalidPersistedFacts { .. }) => {
            Ok(Some(TargetRejection::InvalidTarget))
        }
        Err(SessionControlError::TerminateEpochOverflow) => {
            Ok(Some(TargetRejection::EpochExhausted))
        }
        Err(SessionControlError::PersistenceFailed { source }) => {
            Err(TargetSubmissionError::PersistenceFailed { source })
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct TargetSubmissionRequest {
    pub(crate) operation_id: Uuid,
    pub(crate) scope: TargetScope,
    pub(crate) action: TargetAction,
}

impl TargetSubmissionRequest {
    fn canonical(&self) -> Result<String, TargetSubmissionError> {
        let action = match self.action {
            TargetAction::SetForeground(ForegroundTarget::Waiting) => "set_foreground:waiting",
            TargetAction::SetForeground(ForegroundTarget::Contest) => "set_foreground:contest",
            TargetAction::TerminateSession => "terminate_session",
            TargetAction::ResetHome => "reset_home",
            TargetAction::PowerOff { .. } => "power_off",
        };
        let devices = match &self.scope {
            TargetScope::AllEnabled => None,
            TargetScope::Devices(devices) => {
                Some(devices.iter().map(DeviceId::as_text).collect::<Vec<_>>())
            }
            TargetScope::AllOnlineEnabled(_) => Some(vec!["all_online_enabled".to_owned()]),
        };
        serde_json::to_string(&(action, devices)).map_err(|source| {
            TargetSubmissionError::PersistenceFailed {
                source: PersistenceError::Json {
                    field: "target receipt",
                    source,
                },
            }
        })
    }
}

#[derive(Debug, Clone)]
pub(crate) enum TargetScope {
    AllEnabled,
    Devices(Vec<DeviceId>),
    AllOnlineEnabled(Vec<DeviceId>),
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum TargetAction {
    SetForeground(ForegroundTarget),
    TerminateSession,
    ResetHome,
    PowerOff { expires_at_unix_ms: i64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TargetSubmissionResult {
    pub(crate) operation_id: Uuid,
    pub(crate) results: Vec<DeviceSubmissionResult>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeviceSubmissionResult {
    pub(crate) device_id: DeviceId,
    pub(crate) rejection: Option<TargetRejection>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TargetRejection {
    DeviceNotFound,
    DeviceNotEnabled,
    InvalidDeviceState,
    InvalidTarget,
    EpochExhausted,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredResult {
    device_id: String,
    rejection: Option<TargetRejection>,
}

#[derive(Debug, Snafu)]
pub(crate) enum TargetSubmissionError {
    #[snafu(display("the operation ID belongs to a different request"))]
    OperationIdConflict,
    #[snafu(display("persisted target submission facts are invalid"))]
    InvalidPersistedFacts { source: PersistenceError },
    #[snafu(display("target submission persistence failed"))]
    PersistenceFailed { source: PersistenceError },
}

impl From<PersistenceError> for TargetSubmissionError {
    fn from(source: PersistenceError) -> Self {
        match source {
            source if source.is_invalid_data() => Self::InvalidPersistedFacts { source },
            source => Self::PersistenceFailed { source },
        }
    }
}

#[cfg(test)]
mod tests;
