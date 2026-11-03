use std::{path::PathBuf, sync::Arc};

use crate::{
    component::contest::ContestComponent,
    db::Database,
    http::{AccessRoutes, bind_routes, handler::contest},
    vault::VaultSession,
};

pub(crate) struct ContestFeature {
    /// The single Contest instance shared by routes and Photo initialization.
    contest: Arc<ContestComponent>,
}

impl ContestFeature {
    pub(crate) fn load(database: Database, vault: Arc<VaultSession>, logos: PathBuf) -> Self {
        Self {
            contest: Arc::new(ContestComponent::new(database, vault, logos)),
        }
    }

    pub(crate) const fn component(&self) -> &Arc<ContestComponent> {
        &self.contest
    }

    pub(crate) fn routes(&self) -> AccessRoutes {
        AccessRoutes {
            public: bind_routes(contest::public_routes(), &self.contest),
            operator: bind_routes(contest::operator_routes(), &self.contest),
            ..AccessRoutes::new()
        }
    }
}
