//! Device use cases; components retain their own mutation boundaries.

use std::sync::Arc;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::component::device::{
    ControlAuthority, DeviceError, DeviceId, EnrollmentApprovalError, EnrollmentReviewId,
    LifecycleOutcome, MachineHardwareId,
};

use super::{
    DeviceControl,
    actor::{DeviceConnectionState, DeviceHandle},
    convergence::{DeviceConvergenceError, DeviceStatus, DeviceTargets, build_convergence},
};

impl DeviceControl {
    /// Reads one durable Device and calculates its current convergence view.
    pub(crate) async fn read_device_status(
        &self,
        device_id: DeviceId,
    ) -> Result<Option<DeviceStatus>, DeviceConvergenceError> {
        let Some(device) = self
            .device
            .find_device(device_id)
            .await
            .map_err(DeviceConvergenceError::Device)?
        else {
            return Ok(None);
        };
        let gateway = self
            .gateway
            .read_current(device_id)
            .await
            .map_err(DeviceConvergenceError::Gateway)?;
        let binding = self
            .binding
            .read_current(device_id)
            .await
            .map_err(DeviceConvergenceError::Binding)?;
        let runtime_config = self
            .runtime
            .read_current()
            .await
            .map_err(DeviceConvergenceError::Runtime)?;
        let session_control = self
            .session
            .read_current(device_id)
            .await
            .map_err(DeviceConvergenceError::Session)?;
        let home = self
            .home
            .read_current(device_id)
            .await
            .map_err(DeviceConvergenceError::Home)?;
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

    /// Reads every durable Device and calculates all convergence views with fixed-count queries.
    pub(crate) async fn read_all_device_statuses(
        &self,
    ) -> Result<Vec<DeviceStatus>, DeviceConvergenceError> {
        let devices = self
            .device
            .list_devices()
            .await
            .map_err(DeviceConvergenceError::Device)?;
        let device_ids = devices
            .iter()
            .map(|device| device.device_id())
            .collect::<Vec<_>>();
        let mut gateways = self
            .gateway
            .read_all_current()
            .await
            .map_err(DeviceConvergenceError::Gateway)?;
        let mut bindings = self
            .binding
            .read_all_current()
            .await
            .map_err(DeviceConvergenceError::Binding)?;
        let runtime_config = self
            .runtime
            .read_current()
            .await
            .map_err(DeviceConvergenceError::Runtime)?;
        let mut sessions = self
            .session
            .read_all_current()
            .await
            .map_err(DeviceConvergenceError::Session)?;
        let mut homes = self
            .home
            .read_all_current()
            .await
            .map_err(DeviceConvergenceError::Home)?;
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
            .unwrap_or(Err(DeviceError::PersistenceFailed))
    }

    async fn disable_device_inner(
        &self,
        device_id: DeviceId,
    ) -> Result<LifecycleOutcome, DeviceError> {
        let handle = if let Some(handle) = self.registry.get(device_id).await {
            handle
        } else {
            if self.device.find_device(device_id).await?.is_none() {
                return Err(DeviceError::DeviceNotFound);
            }
            self.registry.get_or_spawn(device_id).await
        };
        let authority_fence = Arc::clone(&handle.authority_fence);
        let mut fenced = authority_fence.lock().await;
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
            .unwrap_or(Err(DeviceError::PersistenceFailed))
    }

    async fn revoke_device_inner(
        &self,
        device_id: DeviceId,
    ) -> Result<LifecycleOutcome, DeviceError> {
        let handle = if let Some(handle) = self.registry.get(device_id).await {
            handle
        } else {
            if self.device.find_device(device_id).await?.is_none() {
                return Err(DeviceError::DeviceNotFound);
            }
            self.registry.get_or_spawn(device_id).await
        };
        let authority_fence = Arc::clone(&handle.authority_fence);
        let mut fenced = authority_fence.lock().await;
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
        let control = Arc::clone(self);
        tokio::spawn(async move { control.approve_enrollment_inner(review_id).await })
            .await
            .unwrap_or(Err(EnrollmentApprovalError::Activation(
                crate::component::device::ActivationError::AuthorityPersistenceFailed,
            )))
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
            .map_err(EnrollmentApprovalError::Authority)?
            .map(ControlAuthority::device_id);
        let handle = match current_device_id {
            Some(device_id) => Some(self.registry.get_or_spawn(device_id).await),
            None => None,
        };
        let authority_fence = handle
            .as_ref()
            .map(|handle| Arc::clone(&handle.authority_fence));
        let mut fenced = match authority_fence.as_ref() {
            Some(fence) => Some(fence.lock().await),
            None => None,
        };
        let approval = self
            .device
            .approve_enrollment(&self.provisioning, review_id)
            .await?;
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

    /// Notifies an existing actor after a committed single-Device target change.
    pub(crate) async fn dirty_device(&self, device_id: DeviceId) {
        self.registry.dirty_one(device_id).await;
    }

    /// Notifies existing actors after a committed fleet-wide target change.
    pub(crate) async fn dirty_all_devices(&self) {
        self.registry.dirty_all().await;
    }

    /// Replaces the current lease and then closes the precheck-to-replacement authority race.
    pub(super) async fn attach_device_lease(
        self: &Arc<Self>,
        machine_hardware_id: MachineHardwareId,
        authority: ControlAuthority,
        outbound: mpsc::Sender<natsume_device_protocol::generated::ServerActiveEnvelope>,
    ) -> Option<(Uuid, DeviceHandle)> {
        if self
            .device
            .find_current_authority(machine_hardware_id)
            .await
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
            .ok()
            != Some(Some(authority))
        {
            handle.clear_lease_if_current(session_id).await;
            return None;
        }
        Some((session_id, handle))
    }
}
