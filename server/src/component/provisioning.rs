use snafu::Snafu;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::{
    audit::{AuditEvent, AuditEventId, CorrelationId, ProvisioningWindowAuditResult},
    db::{Database, DatabaseError},
};

mod audit;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProvisioningWindowState {
    Closed,
    Open,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProvisioningWindowAction {
    Open,
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProvisioningWindow {
    state: ProvisioningWindowState,
}

impl ProvisioningWindow {
    pub(crate) const fn is_open(self) -> bool {
        matches!(self.state, ProvisioningWindowState::Open)
    }
}

/// Enrollment admission authority. Every Server start creates its process-local
/// gate closed while audit persistence remains component-owned.
pub(crate) struct ProvisioningComponent {
    database: Database,
    state: Mutex<ProvisioningWindowState>,
}

impl ProvisioningComponent {
    pub(crate) fn new(database: Database) -> Self {
        Self {
            database,
            state: Mutex::new(ProvisioningWindowState::Closed),
        }
    }

    pub(crate) async fn read_window(&self) -> ProvisioningWindow {
        ProvisioningWindow {
            state: *self.state.lock().await,
        }
    }

    pub(crate) async fn open_window(
        &self,
        correlation_id: CorrelationId,
    ) -> Result<ProvisioningWindow, ProvisioningError> {
        self.mutate_window(
            ProvisioningWindowAction::Open,
            correlation_id,
            AuditEventId::from_uuid(Uuid::now_v7()),
        )
        .await
    }

    pub(crate) async fn close_window(
        &self,
        correlation_id: CorrelationId,
    ) -> Result<ProvisioningWindow, ProvisioningError> {
        self.mutate_window(
            ProvisioningWindowAction::Close,
            correlation_id,
            AuditEventId::from_uuid(Uuid::now_v7()),
        )
        .await
    }

    async fn mutate_window(
        &self,
        action: ProvisioningWindowAction,
        correlation_id: CorrelationId,
        audit_event_id: AuditEventId,
    ) -> Result<ProvisioningWindow, ProvisioningError> {
        let mut state = self.state.lock().await;
        let target = match action {
            ProvisioningWindowAction::Open => ProvisioningWindowState::Open,
            ProvisioningWindowAction::Close => ProvisioningWindowState::Closed,
        };
        let result = if *state == target {
            ProvisioningWindowAuditResult::Noop
        } else {
            ProvisioningWindowAuditResult::Succeeded
        };
        let event = AuditEvent::operator_provisioning_window(
            audit_event_id,
            correlation_id,
            action,
            result,
        );
        self.database
            .write(move |transaction| {
                crate::audit::insert(transaction, &event)
                    .map_err(ProvisioningError::from_audit_persistence)
            })
            .await?;
        *state = target;
        Ok(ProvisioningWindow { state: target })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Snafu)]
pub(crate) enum ProvisioningError {
    #[snafu(display("provisioning audit persistence failed"))]
    PersistenceFailed,
}

impl ProvisioningError {
    const fn from_audit_persistence(error: crate::audit::AuditPersistenceError) -> Self {
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
