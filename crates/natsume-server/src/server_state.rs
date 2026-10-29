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
        photo::PhotoComponent,
        power::PowerControlComponent,
        print::PrintComponent,
        provisioning::ProvisioningComponent,
        runtime::RuntimeConfigComponent,
        session::SessionControlComponent,
        target_submission::TargetSubmissionComponent,
    },
    config::ServerConfig,
    db::Database,
    device_control::DeviceControl,
    journal::JournalComponent,
    journal_query::JournalQuery,
    lifecycle::ServerLifecycle,
    vault::{self, VaultSession},
};

/// Process-wide business composition shared by every transport.
///
/// Startup assembles concrete components and mutation/read coordinators once.
/// HTTP receives their shared handles; Device Control and actors never depend on
/// this composition object.
pub(crate) struct ServerState {
    /// Shared authentication owner also serving bounded display-name reads.
    operator: Arc<OperatorComponent>,
    /// Read-only journal coordinator with bounded storage, names and narrow lease dependencies.
    journal_query: JournalQuery,
    print: PrintComponent,
    photo: PhotoComponent,
    contest: ContestComponent,
    import: ImportComponent,
    provisioning: Arc<ProvisioningComponent>,
    device: Arc<DeviceComponent>,
    /// Shared Binding owner used directly by single-component entrypoints.
    binding: Arc<BindingComponent>,
    session: Arc<SessionControlComponent>,
    home: Arc<HomeComponent>,
    target_submission: TargetSubmissionComponent,
    device_control: Arc<DeviceControl>,
    /// Process-owned journal writer and producer handle; joined after producers stop.
    journal: JournalComponent,
    /// Shared admission and producer ownership, closed once by the composition root.
    lifecycle: ServerLifecycle,
}

impl ServerState {
    /// Loads process-wide resources once and constructs every concrete component and
    /// the Device actor registry from them.
    ///
    /// Any configuration, Vault, site, or Origin CA failure aborts startup before a
    /// partially constructed state becomes visible to transports.
    pub(crate) async fn load(
        database: Database,
        config: &ServerConfig,
        utc_offset: time::UtcOffset,
    ) -> Result<Self, ServerStateError> {
        let vault = vault::load(config.vault_master_key_path())
            .map_err(|source| ServerStateError::Vault { source })?;
        let ca_certificate_path = config
            .origin_ca_certificate_path()
            .map_err(|source| ServerStateError::Configuration { source })?;
        let ca_private_key_path = config
            .origin_ca_private_key_path()
            .map_err(|source| ServerStateError::Configuration { source })?;
        let gateway = GatewayComponent::load(
            database.clone(),
            &ca_certificate_path,
            &ca_private_key_path,
            config.local_origin_root_path(),
            config.site(),
        )
        .map_err(map_gateway_load_error)?;

        let print = PrintComponent::load(
            database.clone(),
            config.print_documents_path().to_path_buf(),
        )
        .await
        .map_err(|source| ServerStateError::PrintStorage { source })?;
        let journal =
            JournalComponent::start(config.journal_path().to_owned(), config.journal().clone());
        Ok(Self::from_parts(
            database,
            print,
            vault,
            gateway,
            (
                config.organization_logos_path().to_path_buf(),
                config.photo_initializations_path().to_path_buf(),
            ),
            journal,
            utc_offset,
        ))
    }

    fn from_parts(
        database: Database,
        print: PrintComponent,
        vault: VaultSession,
        gateway: GatewayComponent,
        (logos, photo_directory): (std::path::PathBuf, std::path::PathBuf),
        journal: JournalComponent,
        utc_offset: time::UtcOffset,
    ) -> Self {
        let lifecycle = ServerLifecycle::new();
        let vault = Arc::new(vault);
        let provisioning = Arc::new(ProvisioningComponent::new());
        let device = Arc::new(DeviceComponent::new(database.clone(), journal.emitter()));
        let binding = Arc::new(BindingComponent::new(
            database.clone(),
            Arc::clone(&vault),
            journal.emitter(),
        ));
        let session = Arc::new(SessionControlComponent::new(database.clone()));
        let home = Arc::new(HomeComponent::new(database.clone()));
        let power = Arc::new(PowerControlComponent::new(database.clone()));
        let device_control = Arc::new(DeviceControl::new(
            Arc::clone(&provisioning),
            Arc::clone(&device),
            gateway,
            Arc::clone(&binding),
            RuntimeConfigComponent::new(database.clone()),
            Arc::clone(&session),
            Arc::clone(&home),
            Arc::clone(&power),
            journal.emitter(),
            lifecycle.clone(),
        ));
        let operator = Arc::new(OperatorComponent::new(database.clone(), journal.emitter()));
        let journal_query = JournalQuery::new(
            journal.reader(),
            Arc::clone(&operator),
            device_control.live_lease_view(),
            utc_offset,
        );
        Self {
            operator,
            journal_query,
            print,
            photo: PhotoComponent::new(database.clone(), photo_directory),
            contest: ContestComponent::new(database.clone(), Arc::clone(&vault), logos),
            target_submission: TargetSubmissionComponent::new(database.clone(), journal.emitter()),
            import: ImportComponent::new(database, vault),
            provisioning,
            device,
            binding,
            session,
            home,
            device_control,
            journal,
            lifecycle,
        }
    }

    pub(crate) const fn journal(&self) -> &JournalComponent {
        &self.journal
    }
    pub(crate) const fn lifecycle(&self) -> &ServerLifecycle {
        &self.lifecycle
    }
    pub(crate) const fn device_control(&self) -> &Arc<DeviceControl> {
        &self.device_control
    }

    pub(crate) const fn print(&self) -> &PrintComponent {
        &self.print
    }

    pub(crate) const fn photo(&self) -> &PhotoComponent {
        &self.photo
    }

    pub(crate) const fn journal_query(&self) -> &JournalQuery {
        &self.journal_query
    }

    pub(crate) fn operator(&self) -> &OperatorComponent {
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

    pub(crate) const fn target_submission(&self) -> &TargetSubmissionComponent {
        &self.target_submission
    }
}

fn map_gateway_load_error(error: GatewayLoadError) -> ServerStateError {
    match error {
        GatewayLoadError::TrustRootMismatch => ServerStateError::OriginCaTrustRootMismatch,
        GatewayLoadError::OriginCa { .. } => ServerStateError::OriginCa { source: error },
    }
}

#[derive(Debug, Snafu)]
pub(crate) enum ServerStateError {
    #[snafu(display("server configuration failed"))]
    Configuration { source: crate::config::ConfigError },
    #[snafu(display("vault startup failed"))]
    Vault { source: crate::vault::VaultError },
    #[snafu(display("print document storage initialization failed"))]
    PrintStorage {
        source: crate::component::print::PrintError,
    },
    #[snafu(display("Origin CA startup failed"))]
    OriginCa { source: GatewayLoadError },
    #[snafu(display("Origin CA issuing certificate and packaged trust root differ"))]
    OriginCaTrustRootMismatch,
}

#[cfg(test)]
pub(crate) mod tests {
    use std::{fs, os::unix::fs::PermissionsExt as _};

    use uuid::Uuid;

    use super::{ServerState, ServerStateError, map_gateway_load_error};
    use crate::{component::gateway::GatewayComponent, db::Database, vault};

    pub(crate) fn for_test(
        database: Database,
        documents: std::path::PathBuf,
    ) -> Result<ServerState, ServerStateError> {
        for_test_with_logos(
            database,
            documents,
            std::path::PathBuf::from("/nonexistent/natsume-test-logos"),
        )
    }

    pub(crate) fn for_test_with_logos(
        database: Database,
        documents: std::path::PathBuf,
        logos: std::path::PathBuf,
    ) -> Result<ServerState, ServerStateError> {
        for_test_with_journal(
            database,
            documents,
            logos,
            crate::journal::JournalComponent::channel(crate::journal::CAPACITY).0,
        )
    }

    pub(crate) fn for_test_with_journal(
        database: Database,
        documents: std::path::PathBuf,
        logos: std::path::PathBuf,
        journal: crate::journal::JournalComponent,
    ) -> Result<ServerState, ServerStateError> {
        let root = std::env::temp_dir().join(format!("natsume-server-state-{}", Uuid::now_v7()));
        fs::create_dir(&root).map_err(|source| ServerStateError::Vault {
            source: vault::VaultError::Read { source },
        })?;
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).map_err(|source| {
            ServerStateError::Vault {
                source: vault::VaultError::Read { source },
            }
        })?;
        let key_path = root.join("master.key");
        vault::ensure_master_key(&key_path).map_err(|source| ServerStateError::Vault { source })?;
        let vault = vault::load(&key_path).map_err(|source| ServerStateError::Vault { source })?;
        fs::remove_dir_all(root).map_err(|source| ServerStateError::Vault {
            source: vault::VaultError::Read { source },
        })?;
        let gateway =
            GatewayComponent::for_test(database.clone()).map_err(map_gateway_load_error)?;
        let photo_directory = documents.with_extension("photo-initializations");
        let print = crate::component::print::PrintComponent::for_test(database.clone(), documents)
            .map_err(|source| ServerStateError::PrintStorage { source })?;
        Ok(ServerState::from_parts(
            database,
            print,
            vault,
            gateway,
            (logos, photo_directory),
            journal,
            time::UtcOffset::UTC,
        ))
    }
}
