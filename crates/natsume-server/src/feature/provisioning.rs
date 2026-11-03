use std::sync::Arc;

use crate::{
    component::provisioning::ProvisioningComponent,
    http::{AccessRoutes, bind_routes, handler::provisioning},
};

pub(crate) struct ProvisioningFeature {
    /// The single Provisioning instance shared by routes and Device Control.
    provisioning: Arc<ProvisioningComponent>,
}

impl ProvisioningFeature {
    pub(crate) fn load() -> Self {
        Self {
            provisioning: Arc::new(ProvisioningComponent::new()),
        }
    }

    pub(crate) const fn component(&self) -> &Arc<ProvisioningComponent> {
        &self.provisioning
    }

    pub(crate) fn routes(&self) -> AccessRoutes {
        AccessRoutes {
            operator: bind_routes(provisioning::operator_routes(), &self.provisioning),
            ..AccessRoutes::new()
        }
    }
}
