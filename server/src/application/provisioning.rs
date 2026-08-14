use snafu::Snafu;

use crate::{
    audit::AuditEventId,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Snafu)]
pub enum ProvisioningError {
    #[snafu(display("the provisioning window revision cannot be incremented"))]
    RevisionOverflow,
    #[snafu(display("provisioning persistence failed"))]
    PersistenceFailed,
}
