//! Device use cases; components retain their own mutation boundaries.

use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{Instrument as _, instrument::WithSubscriber as _};
use uuid::Uuid;

use crate::component::device::{
    ControlAuthority, DeviceError, DeviceId, DeviceListFilter, EnrollmentApprovalError,
    EnrollmentReviewId, LifecycleOutcome, MachineHardwareId,
};

use super::{
    DeviceControl,
    actor::{DeviceConnectionState, DeviceHandle},
    convergence::{DeviceConvergenceError, DeviceStatus, DeviceTargets, build_convergence},
};

impl DeviceControl {
    pub(crate) async fn active_device_ids(&self) -> Vec<DeviceId> {
        self.registry.active_device_ids().await
    }

    /// Reads one durable Device and calculates its current convergence view.
    pub(crate) async fn read_device_status(
        &self,
        device_id: DeviceId,
    ) -> Result<Option<DeviceStatus>, DeviceConvergenceError> {
        let Some(device) = self
            .device
            .find_device(device_id)
            .await
            .map_err(|source| DeviceConvergenceError::Device { source })?
        else {
            return Ok(None);
        };
        let gateway = self
            .gateway
            .read_current(device_id)
            .await
            .map_err(|source| DeviceConvergenceError::Gateway { source })?;
        let binding = self
            .binding
            .read_current(device_id)
            .await
            .map_err(|source| DeviceConvergenceError::Binding { source })?;
        let runtime_config = self
            .runtime
            .read_current()
            .await
            .map_err(|source| DeviceConvergenceError::Runtime { source })?;
        let session_control = self
            .session
            .read_current(device_id)
            .await
            .map_err(|source| DeviceConvergenceError::Session { source })?;
        let home = self
            .home
            .read_current(device_id)
            .await
            .map_err(|source| DeviceConvergenceError::Home { source })?;
        let targets = DeviceTargets {
            gateway,
            binding,
            runtime_config,
            session_control,
            home,
        };
        let connection = self.registry.read_connection_state(device_id).await;
        Ok(Some(DeviceStatus {
            device,
            convergence: build_convergence(connection, targets),
        }))
    }

    /// Selects durable Devices before assembling their convergence views with fixed-count queries.
    pub(crate) async fn read_device_statuses(
        &self,
        filter: DeviceListFilter,
    ) -> Result<Vec<DeviceStatus>, DeviceConvergenceError> {
        let devices = self
            .device
            .list_devices(filter)
            .await
            .map_err(|source| DeviceConvergenceError::Device { source })?;
        if devices.is_empty() {
            return Ok(Vec::new());
        }
        let device_ids = devices
            .iter()
            .map(|device| device.device_id())
            .collect::<Vec<_>>();
        let mut gateways = self
            .gateway
            .read_all_current()
            .await
            .map_err(|source| DeviceConvergenceError::Gateway { source })?;
        let mut bindings = self
            .binding
            .read_all_current()
            .await
            .map_err(|source| DeviceConvergenceError::Binding { source })?;
        let runtime_config = self
            .runtime
            .read_current()
            .await
            .map_err(|source| DeviceConvergenceError::Runtime { source })?;
        let mut sessions = self
            .session
            .read_all_current()
            .await
            .map_err(|source| DeviceConvergenceError::Session { source })?;
        let mut homes = self
            .home
            .read_all_current()
            .await
            .map_err(|source| DeviceConvergenceError::Home { source })?;
        let mut connections = self.registry.read_connection_states(&device_ids).await;

        Ok(devices
            .into_iter()
            .map(|device| {
                let device_id = device.device_id();
                let targets = DeviceTargets {
                    gateway: gateways.remove(&device_id),
                    binding: bindings.remove(&device_id),
                    runtime_config: runtime_config.clone(),
                    session_control: sessions.remove(&device_id),
                    home: homes.remove(&device_id).flatten(),
                };
                let connection = connections
                    .remove(&device_id)
                    .unwrap_or(DeviceConnectionState::Offline);
                DeviceStatus {
                    device,
                    convergence: build_convergence(connection, targets),
                }
            })
            .collect())
    }

    /// Completes disable, fencing, and eviction independently of request cancellation.
    pub(crate) async fn disable_device(
        self: &Arc<Self>,
        device_id: DeviceId,
    ) -> Result<LifecycleOutcome, DeviceError> {
        let control = Arc::clone(self);
        tokio::spawn(async move { control.disable_device_inner(device_id).await })
            .await
            .map_err(|source| DeviceError::PersistenceFailed {
                source: crate::db::PersistenceError::Executor {
                    source: source.into(),
                },
            })?
    }

    async fn disable_device_inner(
        &self,
        device_id: DeviceId,
    ) -> Result<LifecycleOutcome, DeviceError> {
        if self.registry.get(device_id).await.is_none()
            && self.device.find_device(device_id).await?.is_none()
        {
            return Err(DeviceError::DeviceNotFound);
        }
        let (handle, mut fenced) = self.registry.lock_current_fence(device_id).await;
        let outcome = self.device.disable(device_id).await?;
        if matches!(
            outcome,
            LifecycleOutcome::Changed | LifecycleOutcome::Unchanged
        ) {
            *fenced = true;
        }
        drop(fenced);
        if matches!(
            outcome,
            LifecycleOutcome::Changed | LifecycleOutcome::Unchanged
        ) {
            handle.evict_current_lease().await;
        }
        Ok(outcome)
    }

    /// Completes revoke, fencing, and eviction independently of request cancellation.
    pub(crate) async fn revoke_device(
        self: &Arc<Self>,
        device_id: DeviceId,
    ) -> Result<LifecycleOutcome, DeviceError> {
        let control = Arc::clone(self);
        tokio::spawn(async move { control.revoke_device_inner(device_id).await })
            .await
            .map_err(|source| DeviceError::PersistenceFailed {
                source: crate::db::PersistenceError::Executor {
                    source: source.into(),
                },
            })?
    }

    async fn revoke_device_inner(
        &self,
        device_id: DeviceId,
    ) -> Result<LifecycleOutcome, DeviceError> {
        if self.registry.get(device_id).await.is_none()
            && self.device.find_device(device_id).await?.is_none()
        {
            return Err(DeviceError::DeviceNotFound);
        }
        let (handle, mut fenced) = self.registry.lock_current_fence(device_id).await;
        let outcome = self.device.revoke(device_id).await?;
        if matches!(
            outcome,
            LifecycleOutcome::Changed | LifecycleOutcome::Unchanged
        ) {
            *fenced = true;
        }
        drop(fenced);
        if matches!(
            outcome,
            LifecycleOutcome::Changed | LifecycleOutcome::Unchanged
        ) {
            handle.evict_current_lease().await;
        }
        Ok(outcome)
    }

    /// Completes approval, fencing, eviction, and notification independently of the request.
    pub(crate) async fn approve_enrollment(
        self: &Arc<Self>,
        review_id: EnrollmentReviewId,
    ) -> Result<ControlAuthority, EnrollmentApprovalError> {
        self.approve_enrollment_with_reporting(review_id, false)
            .await
    }

    pub(super) async fn automatically_approve_enrollment(
        self: &Arc<Self>,
        review_id: EnrollmentReviewId,
    ) -> Result<ControlAuthority, EnrollmentApprovalError> {
        self.approve_enrollment_with_reporting(review_id, true)
            .await
    }

    async fn approve_enrollment_with_reporting(
        self: &Arc<Self>,
        review_id: EnrollmentReviewId,
        automatic: bool,
    ) -> Result<ControlAuthority, EnrollmentApprovalError> {
        let control = Arc::clone(self);
        tokio::spawn(async move {
            control.approve_enrollment_inner(review_id).await.inspect_err(|error| {
                if automatic && !matches!(error, EnrollmentApprovalError::ReviewNotFound) {
                    tracing::warn!(error = %snafu::Report::from_error(error), "Device automatic enrollment approval failed");
                }
            })
        }.in_current_span().with_current_subscriber())
            .await
            .map_err(|source| EnrollmentApprovalError::Activation {
                source: Arc::new(
                    crate::component::device::ActivationError::AuthorityPersistenceFailed {
                        source: crate::db::PersistenceError::Executor {
                            source: source.into(),
                        },
                    },
                ),
            })?
    }

    async fn approve_enrollment_inner(
        &self,
        review_id: EnrollmentReviewId,
    ) -> Result<ControlAuthority, EnrollmentApprovalError> {
        let _approval = self.enrollment_approval.lock().await;
        let machine_hardware_id = self
            .device
            .enrollment_review_machine_hardware_id(review_id)
            .await?;
        let current_device_id = self
            .device
            .find_current_authority(machine_hardware_id)
            .await
            .map_err(|source| EnrollmentApprovalError::Authority {
                source: std::sync::Arc::new(source),
            })?
            .map(ControlAuthority::device_id);
        let (handle, mut fenced) = match current_device_id {
            Some(device_id) => {
                let (handle, fenced) = self.registry.lock_current_fence(device_id).await;
                (Some(handle), Some(fenced))
            }
            None => (None, None),
        };
        let approval = self.device.approve_enrollment(review_id).await?;
        let authority = approval.authority();
        if let Some(fenced) = fenced.as_mut() {
            **fenced = true;
        }
        drop(fenced);
        match handle {
            Some(handle) => handle.evict_current_lease().await,
            None => {
                self.evict_current_lease(authority.device_id()).await;
            }
        }
        approval.complete();
        Ok(authority)
    }

    pub(crate) async fn publish(&self, change: super::TargetChange) {
        match change.into_scope() {
            super::change::TargetScope::Device(device_id) => {
                self.registry.dirty_one(device_id).await;
            }
            super::change::TargetScope::Devices(devices) => {
                self.registry.dirty_many(&devices).await;
            }
            super::change::TargetScope::AllConnected => self.registry.dirty_all().await,
        }
    }

    /// Replaces the current lease and then closes the precheck-to-replacement authority race.
    pub(super) async fn attach_device_lease(
        self: &Arc<Self>,
        machine_hardware_id: MachineHardwareId,
        authority: ControlAuthority,
        outbound: mpsc::Sender<natsume_workstation_protocol::generated::ServerActiveEnvelope>,
    ) -> Option<(Uuid, DeviceHandle)> {
        if self
            .device
            .find_current_authority(machine_hardware_id)
            .await
            .inspect_err(|error| tracing::warn!(error = %snafu::Report::from_error(error), "Device lease authority lookup failed"))
            .ok()
            != Some(Some(authority))
        {
            return None;
        }
        let handle = self.registry.get_or_spawn(authority.device_id()).await;
        let session_id = handle
            .replace_current_lease(Arc::downgrade(self), outbound)
            .await?;
        if self
            .device
            .find_current_authority(machine_hardware_id)
            .await
            .inspect_err(|error| tracing::warn!(error = %snafu::Report::from_error(error), "Device lease authority lookup failed"))
            .ok()
            != Some(Some(authority))
        {
            handle.clear_lease_if_current(session_id).await;
            return None;
        }
        Some((session_id, handle))
    }
}

#[cfg(test)]
pub(super) async fn mutate_authority_for_test(
    control: &DeviceControl,
    device_id: DeviceId,
    revoke: bool,
) -> Result<LifecycleOutcome, DeviceError> {
    if revoke {
        control.revoke_device_inner(device_id).await
    } else {
        control.disable_device_inner(device_id).await
    }
}
