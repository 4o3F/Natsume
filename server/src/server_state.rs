use std::sync::Arc;

use snafu::Snafu;

use crate::{
    component::{
        binding::BindingComponent,
        contest::ContestComponent,
        device::DeviceComponent,
        gateway::{GatewayComponent, GatewayLoadError},
        home::HomeComponent,
        import::ImportComponent,
        operator::OperatorComponent,
        provisioning::ProvisioningComponent,
        runtime::RuntimeConfigComponent,
        session::SessionControlComponent,
    },
    config::{GatewaySiteConfig, ServerConfig},
    db::Database,
    device_control::DeviceControl,
    vault::{self, VaultSession},
};

/// Process-wide business composition shared by every transport.
///
/// Startup assembles concrete components and the Device Control coordinator once.
/// HTTP receives their shared handles; Device Control and actors never depend on
/// this composition object.
pub(crate) struct ServerState {
    operator: OperatorComponent,
    contest: ContestComponent,
    import: ImportComponent,
    provisioning: Arc<ProvisioningComponent>,
    device: Arc<DeviceComponent>,
    binding: Arc<BindingComponent>,
    session: Arc<SessionControlComponent>,
    home: Arc<HomeComponent>,
    device_control: Arc<DeviceControl>,
}

impl ServerState {
    /// Loads process-wide resources once and constructs every concrete component and
    /// the Device actor registry from them.
    ///
    /// Any configuration, Vault, site, or Origin CA failure aborts startup before a
    /// partially constructed state becomes visible to transports.
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
        let vault = Arc::new(vault);
        let provisioning = Arc::new(ProvisioningComponent::new());
        let device = Arc::new(DeviceComponent::new(database.clone()));
        let binding = Arc::new(BindingComponent::new(database.clone(), Arc::clone(&vault)));
        let session = Arc::new(SessionControlComponent::new(database.clone()));
        let home = Arc::new(HomeComponent::new(database.clone()));
        let device_control = Arc::new(DeviceControl::new(
            Arc::clone(&provisioning),
            Arc::clone(&device),
            gateway,
            Arc::clone(&binding),
            RuntimeConfigComponent::new(database.clone()),
            Arc::clone(&session),
            Arc::clone(&home),
        ));
        Self {
            operator: OperatorComponent::new(database.clone()),
            contest: ContestComponent::new(database.clone()),
            import: ImportComponent::new(database, vault),
            provisioning,
            device,
            binding,
            session,
            home,
            device_control,
        }
    }

    pub(crate) const fn device_control(&self) -> &Arc<DeviceControl> {
        &self.device_control
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

    pub(crate) fn provisioning(&self) -> &ProvisioningComponent {
        &self.provisioning
    }

    pub(crate) fn device(&self) -> &DeviceComponent {
        &self.device
    }

    pub(crate) fn binding(&self) -> &BindingComponent {
        &self.binding
    }

    pub(crate) fn session(&self) -> &SessionControlComponent {
        &self.session
    }

    pub(crate) fn home(&self) -> &HomeComponent {
        &self.home
    }
}

fn map_gateway_load_error(error: GatewayLoadError) -> ServerStateError {
    match error {
        GatewayLoadError::TrustRootMismatch => ServerStateError::OriginCaTrustRootMismatch,
        GatewayLoadError::OriginCa => ServerStateError::OriginCa,
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
