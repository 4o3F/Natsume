use std::sync::Arc;

use crate::{
    component::{
        binding::BindingComponent, device::DeviceComponent, gateway::GatewayComponent,
        home::HomeComponent, power::PowerControlComponent, provisioning::ProvisioningComponent,
        runtime::RuntimeConfigComponent, session::SessionControlComponent,
        target_submission::TargetSubmissionComponent,
    },
    device_control::{DeviceControl, DeviceControlDependencies},
    http::{
        AccessRoutes, bind_routes,
        handler::{device, device_control, enrollment, target_submission},
    },
};

use super::Infra;

pub(crate) struct DeviceFeature {
    /// Shared device coordinator and the single actor registry.
    device_control: Arc<DeviceControl>,
    /// Shared durable device authority and enrollment operations.
    device: Arc<DeviceComponent>,
    /// Shared committed binding operations.
    binding: Arc<BindingComponent>,
    /// Shared device session targets.
    session: Arc<SessionControlComponent>,
    /// Shared home reset targets.
    home: Arc<HomeComponent>,
    /// Shared target submission transactions and receipts.
    target_submission: Arc<TargetSubmissionComponent>,
}

impl DeviceFeature {
    pub(crate) fn load(
        infra: &Infra,
        gateway: GatewayComponent,
        provisioning: Arc<ProvisioningComponent>,
    ) -> Self {
        let device = Arc::new(DeviceComponent::new(
            infra.database.clone(),
            infra.journal.clone(),
        ));
        let binding = Arc::new(BindingComponent::new(
            infra.database.clone(),
            Arc::clone(&infra.vault),
            infra.journal.clone(),
        ));
        let session = Arc::new(SessionControlComponent::new(infra.database.clone()));
        let home = Arc::new(HomeComponent::new(infra.database.clone()));
        let power = Arc::new(PowerControlComponent::new(infra.database.clone()));
        let device_control = Arc::new(DeviceControl::new(DeviceControlDependencies {
            provisioning,
            device: Arc::clone(&device),
            gateway,
            binding: Arc::clone(&binding),
            runtime: RuntimeConfigComponent::new(infra.database.clone()),
            session: Arc::clone(&session),
            home: Arc::clone(&home),
            power: Arc::clone(&power),
            journal: infra.journal.clone(),
            lifecycle: infra.lifecycle.clone(),
        }));
        Self {
            device_control,
            device,
            binding,
            session,
            home,
            target_submission: Arc::new(TargetSubmissionComponent::new(
                infra.database.clone(),
                infra.journal.clone(),
            )),
        }
    }

    pub(crate) const fn device_control(&self) -> &Arc<DeviceControl> {
        &self.device_control
    }

    pub(crate) fn routes(&self) -> AccessRoutes {
        let record = device::lifecycle::DeviceRecordState {
            device: Arc::clone(&self.device),
            device_control: Arc::clone(&self.device_control),
        };
        let binding = device::binding::DeviceBindingState {
            binding: Arc::clone(&self.binding),
            device_control: Arc::clone(&self.device_control),
        };
        let submission = target_submission::TargetSubmissionState {
            target_submission: Arc::clone(&self.target_submission),
            device_control: Arc::clone(&self.device_control),
        };
        let mut operator = bind_routes(device::lifecycle::operator_routes(), &record);
        operator.extend(bind_routes(device::binding::operator_routes(), &binding));
        operator.extend(bind_routes(
            device::session::operator_routes(),
            &self.session,
        ));
        operator.extend(bind_routes(device::home::operator_routes(), &self.home));
        operator.extend(bind_routes(
            device::convergence::operator_routes(),
            &self.device_control,
        ));
        operator.extend(bind_routes(enrollment::operator_routes(), &record));
        operator.extend(bind_routes(
            target_submission::operator_routes(),
            &submission,
        ));
        AccessRoutes {
            public: bind_routes(device_control::public_routes(), &self.device_control),
            operator,
            ..AccessRoutes::new()
        }
    }
}
