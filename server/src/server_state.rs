use snafu::Snafu;

use crate::{
    component::{
        contest::ContestComponent,
        device::DeviceComponent,
        gateway::{GatewayComponent, GatewayLoadError},
        import::ImportComponent,
        operator::OperatorComponent,
        provisioning::ProvisioningComponent,
    },
    config::{GatewaySiteConfig, ServerConfig},
    db::Database,
    vault::{self, VaultSession},
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
    // TODO(WP7): Consume Device and Gateway from the production DeviceActor.
    #[allow(dead_code)]
    device: DeviceComponent,
    #[allow(dead_code)]
    gateway: GatewayComponent,
}

impl ServerState {
    pub(crate) fn load(
        database: Database,
        config: &ServerConfig,
    ) -> Result<Self, ServerStateError> {
        let vault =
            vault::load(config.vault_master_key_path()).map_err(|_| ServerStateError::Vault)?;
        let site = GatewaySiteConfig::load_from(config.site_config_path())
            .map_err(|_| ServerStateError::SiteConfiguration)?;
        let ca_certificate_path = config
            .origin_ca_certificate_path()
            .map_err(|_| ServerStateError::Configuration)?;
        let ca_private_key_path = config
            .origin_ca_private_key_path()
            .map_err(|_| ServerStateError::Configuration)?;
        let gateway = GatewayComponent::load(
            database.clone(),
            &ca_certificate_path,
            &ca_private_key_path,
            config.local_origin_root_path(),
            &site,
        )
        .map_err(map_gateway_load_error)?;

        Ok(Self::from_parts(database, vault, gateway))
    }

    fn from_parts(database: Database, vault: VaultSession, gateway: GatewayComponent) -> Self {
        Self {
            operator: OperatorComponent::new(database.clone()),
            contest: ContestComponent::new(database.clone()),
            import: ImportComponent::new(database.clone(), vault),
            provisioning: ProvisioningComponent::new(),
            device: DeviceComponent::new(database),
            gateway,
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

    // TODO(WP7): Use these accessors from the production DeviceActor.
    #[allow(dead_code)]
    pub(crate) const fn device(&self) -> &DeviceComponent {
        &self.device
    }

    #[allow(dead_code)]
    pub(crate) const fn gateway(&self) -> &GatewayComponent {
        &self.gateway
    }
}

fn map_gateway_load_error(error: GatewayLoadError) -> ServerStateError {
    if error.is_trust_root_mismatch() {
        ServerStateError::OriginCaTrustRootMismatch
    } else {
        ServerStateError::OriginCa
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Snafu)]
pub(crate) enum ServerStateError {
    #[snafu(display("server configuration failed"))]
    Configuration,
    #[snafu(display("site configuration startup failed"))]
    SiteConfiguration,
    #[snafu(display("vault startup failed"))]
    Vault,
    #[snafu(display("Origin CA startup failed"))]
    OriginCa,
    #[snafu(display("Origin CA issuing certificate and packaged trust root differ"))]
    OriginCaTrustRootMismatch,
}

#[cfg(test)]
pub(crate) mod tests {
    use std::{fs, os::unix::fs::PermissionsExt as _};

    use uuid::Uuid;

    use super::{ServerState, ServerStateError, map_gateway_load_error};
    use crate::{component::gateway::GatewayComponent, db::Database, vault};

    pub(crate) fn for_test(database: Database) -> Result<ServerState, ServerStateError> {
        let root = std::env::temp_dir().join(format!("natsume-server-state-{}", Uuid::now_v7()));
        fs::create_dir(&root).map_err(|_| ServerStateError::Vault)?;
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
            .map_err(|_| ServerStateError::Vault)?;
        let key_path = root.join("master.key");
        vault::ensure_master_key(&key_path).map_err(|_| ServerStateError::Vault)?;
        let vault = vault::load(&key_path).map_err(|_| ServerStateError::Vault)?;
        fs::remove_dir_all(root).map_err(|_| ServerStateError::Vault)?;
        let gateway =
            GatewayComponent::for_test(database.clone()).map_err(map_gateway_load_error)?;
        Ok(ServerState::from_parts(database, vault, gateway))
    }
}
