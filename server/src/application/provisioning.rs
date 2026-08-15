use snafu::Snafu;

use crate::{
    audit::{AuditEventId, CorrelationId},
    db::{self, Database},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryOutcome {
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
pub enum ProvisioningWindowState {
    Closed,
    Open,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProvisioningWindowAction {
    Open,
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProvisioningWindow {
    pub state: ProvisioningWindowState,
    pub revision: i64,
}

/// The current revision cannot be incremented; overflow is the only failure
/// the pure recovery transition can produce, and callers map it into their
/// own error vocabulary immediately.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RevisionOverflow;

/// Computes the close-once recovery transition from current facts.
///
/// # Errors
///
/// Returns [`RevisionOverflow`] when the current revision cannot be
/// incremented.
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

/// Runs the idempotent startup recovery against the connected database.
///
/// # Errors
///
/// Returns [`ProvisioningError::RevisionOverflow`] when the open window's
/// revision cannot advance, or [`ProvisioningError::PersistenceFailed`] for a
/// persistence failure.
pub async fn recover_on_startup(database: &Database) -> Result<RecoveryOutcome, ProvisioningError> {
    db::provisioning::recover_provisioning_window(database).await
}

/// Reads the current provisioning-window fact.
///
/// # Errors
///
/// Returns [`ProvisioningError::PersistenceFailed`] when the singleton cannot
/// be read or contains invalid facts.
pub(crate) async fn read_window(
    database: &Database,
) -> Result<ProvisioningWindow, ProvisioningError> {
    db::provisioning::read_window(database).await
}

/// Applies the repeat-safe operator open action and its audit atomically.
///
/// # Errors
///
/// Returns [`ProvisioningError::RevisionOverflow`] when an effective transition
/// cannot advance the revision, or [`ProvisioningError::PersistenceFailed`] for
/// any persistence failure.
pub(crate) async fn open_window(
    database: &Database,
    correlation_id: CorrelationId,
) -> Result<ProvisioningWindow, ProvisioningError> {
    db::provisioning::open_window(database, correlation_id).await
}

/// Applies the repeat-safe operator close action and its audit atomically.
///
/// # Errors
///
/// Returns [`ProvisioningError::RevisionOverflow`] when an effective transition
/// cannot advance the revision, or [`ProvisioningError::PersistenceFailed`] for
/// any persistence failure.
pub(crate) async fn close_window(
    database: &Database,
    correlation_id: CorrelationId,
) -> Result<ProvisioningWindow, ProvisioningError> {
    db::provisioning::close_window(database, correlation_id).await
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Snafu)]
pub enum ProvisioningError {
    #[snafu(display("the provisioning window revision cannot be incremented"))]
    RevisionOverflow,
    #[snafu(display("provisioning persistence failed"))]
    PersistenceFailed,
}
