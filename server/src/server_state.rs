use std::sync::Arc;

use crate::{
    component::{
        contest::ContestComponent, import::ImportComponent, operator::OperatorComponent,
        provisioning::ProvisioningComponent,
    },
    db::Database,
    vault::VaultSession,
};

/// Process-wide business composition shared by every transport.
///
/// Raw infrastructure stays inside concrete components. HTTP, WSS, and actors
/// receive this state and call business methods instead of assembling their own
/// dependency graphs.
pub(crate) struct ServerState {
    operator: OperatorComponent,
    contest: ContestComponent,
    import: ImportComponent,
    provisioning: ProvisioningComponent,
}

impl ServerState {
    pub(crate) fn new(database: Database, vault: Arc<VaultSession>) -> Self {
        Self {
            operator: OperatorComponent::new(database.clone()),
            contest: ContestComponent::new(database.clone()),
            import: ImportComponent::new(database, vault),
            provisioning: ProvisioningComponent::new(),
        }
    }

    pub(crate) const fn operator(&self) -> &OperatorComponent {
        &self.operator
    }

    pub(crate) const fn contest(&self) -> &ContestComponent {
        &self.contest
    }

    pub(crate) const fn import(&self) -> &ImportComponent {
        &self.import
    }

    pub(crate) const fn provisioning(&self) -> &ProvisioningComponent {
        &self.provisioning
    }
}
