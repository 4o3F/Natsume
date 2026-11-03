use std::sync::Arc;

use crate::{
    component::{contest::ContestComponent, import::ImportComponent},
    db::Database,
    device_control::DeviceControl,
    http::{
        AccessRoutes, bind_routes,
        handler::{contest::organization, import},
    },
    vault::VaultSession,
};

pub(crate) struct ImportFeature {
    /// The single Import instance shared by upload and candidate routes.
    import: Arc<ImportComponent>,
    /// Shared Contest instance used to read candidate organization logos.
    contest: Arc<ContestComponent>,
    /// Shared device coordinator notified after an import commit.
    device_control: Arc<DeviceControl>,
}

impl ImportFeature {
    pub(crate) fn load(
        database: Database,
        vault: Arc<VaultSession>,
        contest: Arc<ContestComponent>,
        device_control: Arc<DeviceControl>,
    ) -> Self {
        Self {
            import: Arc::new(ImportComponent::new(database, vault)),
            contest,
            device_control,
        }
    }

    #[cfg(test)]
    pub(crate) const fn component(&self) -> &Arc<ImportComponent> {
        &self.import
    }

    #[cfg(test)]
    pub(crate) const fn contest(&self) -> &Arc<ContestComponent> {
        &self.contest
    }

    pub(crate) fn routes(&self) -> AccessRoutes {
        let import = import::ImportState {
            import: Arc::clone(&self.import),
            device_control: Arc::clone(&self.device_control),
        };
        let candidate = organization::ImportCandidateState {
            import: Arc::clone(&self.import),
            contest: Arc::clone(&self.contest),
        };
        let mut operator = bind_routes(import::operator_routes(), &import);
        operator.extend(bind_routes(
            organization::import_candidate_routes(),
            &candidate,
        ));
        AccessRoutes {
            operator,
            ..AccessRoutes::new()
        }
    }
}
