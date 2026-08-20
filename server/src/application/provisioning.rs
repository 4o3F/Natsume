use snafu::Snafu;
use uuid::Uuid;

use crate::{
    audit::{AuditEvent, AuditEventId, CorrelationId, ProvisioningWindowAuditResult},
    db::{self, Database, DatabaseError},
};

/// Redacted persistence boundary shared by Provisioning-owned adapters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Snafu)]
#[snafu(module)]
pub(crate) enum ProvisioningPersistenceError {
    #[snafu(display("persisted provisioning facts are invalid"))]
    InvalidPersistedFacts,
    #[snafu(display("provisioning persistence failed"))]
    PersistenceFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecoveryOutcome {
    AlreadyClosed {
        revision: i64,
    },
    Closed {
        previous_revision: i64,
        new_revision: i64,
        audit_event_id: AuditEventId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProvisioningWindowState {
    Closed,
    Open,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProvisioningWindowAction {
    Open,
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProvisioningWindow {
    pub(crate) state: ProvisioningWindowState,
    pub(crate) revision: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RevisionOverflow;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProvisioningWindowDecision {
    next: ProvisioningWindow,
    applies: bool,
}

/// Computes the close-once recovery transition from current facts.
pub(crate) fn recovered_provisioning_window(
    current: ProvisioningWindow,
) -> Result<Option<ProvisioningWindow>, RevisionOverflow> {
    if current.state == ProvisioningWindowState::Closed {
        return Ok(None);
    }
    let revision = current.revision.checked_add(1).ok_or(RevisionOverflow)?;
    Ok(Some(ProvisioningWindow {
        state: ProvisioningWindowState::Closed,
        revision,
    }))
}

fn decide_operator_window(
    action: ProvisioningWindowAction,
    current: ProvisioningWindow,
) -> Result<ProvisioningWindowDecision, RevisionOverflow> {
    let target = match action {
        ProvisioningWindowAction::Open => ProvisioningWindowState::Open,
        ProvisioningWindowAction::Close => ProvisioningWindowState::Closed,
    };
    if current.state == target {
        return Ok(ProvisioningWindowDecision {
            next: current,
            applies: false,
        });
    }
    let revision = current.revision.checked_add(1).ok_or(RevisionOverflow)?;
    Ok(ProvisioningWindowDecision {
        next: ProvisioningWindow {
            state: target,
            revision,
        },
        applies: true,
    })
}

/// Runs startup recovery. An already-closed window completes after only a
/// deferred read; an observed open window is re-read under the write lock.
pub(crate) async fn recover_on_startup(
    database: &Database,
) -> Result<RecoveryOutcome, ProvisioningError> {
    let snapshot = read_window(database).await?;
    let Some(_) = recovered_provisioning_window(snapshot)
        .map_err(|RevisionOverflow| ProvisioningError::RevisionOverflow)?
    else {
        return Ok(RecoveryOutcome::AlreadyClosed {
            revision: snapshot.revision,
        });
    };

    let audit_event_id = AuditEventId::from_uuid(Uuid::now_v7());
    let correlation_id = CorrelationId::from_uuid(Uuid::now_v7());
    database
        .write(move |transaction| {
            let current = db::provisioning::read_window(transaction)
                .map_err(ProvisioningError::from_provisioning_persistence)?;
            let Some(next) = recovered_provisioning_window(current)
                .map_err(|RevisionOverflow| ProvisioningError::RevisionOverflow)?
            else {
                return Ok(RecoveryOutcome::AlreadyClosed {
                    revision: current.revision,
                });
            };

            let event = AuditEvent::recovery_close(
                audit_event_id,
                correlation_id,
                current.revision,
                next.revision,
            );
            db::audit::insert(transaction, &event)
                .map_err(ProvisioningError::from_audit_persistence)?;
            db::provisioning::compare_and_swap_window(transaction, current, next, audit_event_id)
                .map_err(ProvisioningError::from_provisioning_persistence)?;
            Ok(RecoveryOutcome::Closed {
                previous_revision: current.revision,
                new_revision: next.revision,
                audit_event_id,
            })
        })
        .await
}

pub(crate) async fn read_window(
    database: &Database,
) -> Result<ProvisioningWindow, ProvisioningError> {
    database
        .read(db::provisioning::read_window)
        .await
        .map_err(ProvisioningError::from_provisioning_persistence)
}

pub(crate) async fn open_window(
    database: &Database,
    correlation_id: CorrelationId,
) -> Result<ProvisioningWindow, ProvisioningError> {
    open_window_with_ids(
        database,
        correlation_id,
        AuditEventId::from_uuid(Uuid::now_v7()),
    )
    .await
}

pub(crate) async fn open_window_with_ids(
    database: &Database,
    correlation_id: CorrelationId,
    audit_event_id: AuditEventId,
) -> Result<ProvisioningWindow, ProvisioningError> {
    mutate_window_with_ids(
        database,
        ProvisioningWindowAction::Open,
        correlation_id,
        audit_event_id,
    )
    .await
}

pub(crate) async fn close_window(
    database: &Database,
    correlation_id: CorrelationId,
) -> Result<ProvisioningWindow, ProvisioningError> {
    close_window_with_ids(
        database,
        correlation_id,
        AuditEventId::from_uuid(Uuid::now_v7()),
    )
    .await
}

pub(crate) async fn close_window_with_ids(
    database: &Database,
    correlation_id: CorrelationId,
    audit_event_id: AuditEventId,
) -> Result<ProvisioningWindow, ProvisioningError> {
    mutate_window_with_ids(
        database,
        ProvisioningWindowAction::Close,
        correlation_id,
        audit_event_id,
    )
    .await
}

async fn mutate_window_with_ids(
    database: &Database,
    action: ProvisioningWindowAction,
    correlation_id: CorrelationId,
    audit_event_id: AuditEventId,
) -> Result<ProvisioningWindow, ProvisioningError> {
    database
        .write(move |transaction| {
            let current = db::provisioning::read_window(transaction)
                .map_err(ProvisioningError::from_provisioning_persistence)?;
            let decision = decide_operator_window(action, current)
                .map_err(|RevisionOverflow| ProvisioningError::RevisionOverflow)?;
            let audit_result = if decision.applies {
                ProvisioningWindowAuditResult::Succeeded
            } else {
                ProvisioningWindowAuditResult::Noop
            };
            let event = AuditEvent::operator_provisioning_window(
                audit_event_id,
                correlation_id,
                action,
                audit_result,
                current.revision,
                decision.next.revision,
            );
            db::audit::insert(transaction, &event)
                .map_err(ProvisioningError::from_audit_persistence)?;

            if !decision.applies {
                return Ok(decision.next);
            }
            db::provisioning::compare_and_swap_window(
                transaction,
                current,
                decision.next,
                audit_event_id,
            )
            .map_err(ProvisioningError::from_provisioning_persistence)?;
            Ok(decision.next)
        })
        .await
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Snafu)]
pub(crate) enum ProvisioningError {
    #[snafu(display("the provisioning window revision cannot be incremented"))]
    RevisionOverflow,
    #[snafu(display("provisioning persistence failed"))]
    PersistenceFailed,
}

impl ProvisioningError {
    pub(crate) const fn from_provisioning_persistence(error: ProvisioningPersistenceError) -> Self {
        match error {
            ProvisioningPersistenceError::InvalidPersistedFacts
            | ProvisioningPersistenceError::PersistenceFailed => Self::PersistenceFailed,
        }
    }

    pub(crate) const fn from_audit_persistence(error: crate::audit::AuditPersistenceError) -> Self {
        match error {
            crate::audit::AuditPersistenceError::PersistenceFailed => Self::PersistenceFailed,
        }
    }
}

impl From<DatabaseError> for ProvisioningError {
    fn from(error: DatabaseError) -> Self {
        match error {
            DatabaseError::InvalidConfiguration
            | DatabaseError::ConnectionFailed
            | DatabaseError::MigrationFailed
            | DatabaseError::TransactionFailed => Self::PersistenceFailed,
        }
    }
}

#[cfg(test)]
mod mapping_tests {
    use crate::audit::AuditPersistenceError;

    use super::{ProvisioningError, ProvisioningPersistenceError};

    #[test]
    fn persistence_mappings_cover_every_neutral_variant() {
        for error in [
            ProvisioningPersistenceError::InvalidPersistedFacts,
            ProvisioningPersistenceError::PersistenceFailed,
        ] {
            assert_eq!(
                ProvisioningError::from_provisioning_persistence(error),
                ProvisioningError::PersistenceFailed
            );
        }
        assert_eq!(
            ProvisioningError::from_audit_persistence(AuditPersistenceError::PersistenceFailed),
            ProvisioningError::PersistenceFailed
        );
    }
}
