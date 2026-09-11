//! Applies negotiated inputs and static resource configuration in dependency order.

use natsume_device_protocol::generated::{ClientInputState, RuntimeConfigActualState};
use tokio_util::sync::CancellationToken;

use super::{
    ReconcileOutcome, SnapshotError, SnapshotReconciler, ValidatedSnapshot, check_cancellation,
    gateway::GatewayMaterialState,
};

/// Configuration sampled before any target effects are applied.
pub(super) struct ConfigurationObservation {
    pub(super) gateway_before: GatewayMaterialState,
    pub(super) runtime: RuntimeConfigActualState,
    pub(super) origin_applied: bool,
    pub(super) binding_is_applied: bool,
}

pub(super) struct AppliedConfiguration {
    pub(super) input: ClientInputState,
    pub(super) gateway_material: ReconcileOutcome<GatewayMaterialState>,
    pub(super) runtime_actual: ReconcileOutcome<RuntimeConfigActualState>,
}

impl SnapshotReconciler {
    /// Persists inputs, then applies Gateway, Runtime and Binding in that order.
    pub(super) fn apply_configuration(
        &self,
        snapshot: &ValidatedSnapshot,
        before: ConfigurationObservation,
        cancellation: &CancellationToken,
    ) -> Result<AppliedConfiguration, SnapshotError> {
        let ConfigurationObservation {
            gateway_before,
            runtime,
            origin_applied,
            binding_is_applied,
        } = before;
        check_cancellation(cancellation)?;
        let gateway_input = Some(self.gateway.current_input(&snapshot.gateway_target)?);
        let binding_input = if let Some(binding_intent) = snapshot.binding_intent.clone() {
            self.binding_input
                .current_input(cancellation, binding_intent)?
        } else {
            self.binding_input.clear_intent(cancellation)?;
            None
        };
        check_cancellation(cancellation)?;
        let gateway_material = match gateway_before {
            GatewayMaterialState::RecoveryRequired => self
                .gateway
                .reconcile(&snapshot.gateway_target, cancellation)?,
            current => ReconcileOutcome::idle(current),
        };
        check_cancellation(cancellation)?;
        let runtime_actual = if origin_applied {
            ReconcileOutcome::idle(runtime)
        } else {
            self.runtime
                .reconcile(&snapshot.runtime_origin, cancellation)?
        };
        check_cancellation(cancellation)?;
        if !binding_is_applied {
            self.binding
                .reconcile(&snapshot.binding_target, cancellation)?;
        }
        Ok(AppliedConfiguration {
            input: ClientInputState {
                gateway_credential: gateway_input,
                binding: binding_input,
            },
            gateway_material,
            runtime_actual,
        })
    }
}
