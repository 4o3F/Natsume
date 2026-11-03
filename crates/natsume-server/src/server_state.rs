use std::{path::PathBuf, sync::Arc};

use snafu::ResultExt as _;

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
        runtime::RuntimeConfigComponent,
        session::SessionControlComponent,
        target_submission::TargetSubmissionComponent,
    },
    config::ServerConfig,
    db::Database,
    device_control::{DeviceControl, DeviceControlDependencies},
    feature::{
        Infra, PrintStorageSnafu, StartupError, journal::JournalFeature, operator::OperatorFeature,
        print::PrintFeature, provisioning::ProvisioningFeature,
    },
    http::AccessRoutes,
    journal::{Journal, JournalComponent},
    lifecycle::ServerLifecycle,
    server_runtime::ServerRuntime,
    vault,
};

/// Process-wide business composition shared by every transport.
///
/// Startup assembles concrete components and mutation/read coordinators once.
/// HTTP receives their shared handles; Device Control and actors never depend on
/// this composition object.
pub(crate) struct ServerState {
    /// Transitional HTTP authentication handle; removed in Step 11.
    /// Business handlers obtain Operator directly from their feature state.
    operator: Arc<OperatorComponent>,
    /// Transitional handle for HTTP authentication and background startup; removed in Step 11.
    /// Business handlers obtain Print directly from their feature state.
    print: Arc<PrintComponent>,
    photo: PhotoComponent,
    contest: ContestComponent,
    import: ImportComponent,
    device: Arc<DeviceComponent>,
    /// Shared Binding owner used directly by single-component entrypoints.
    binding: Arc<BindingComponent>,
    session: Arc<SessionControlComponent>,
    home: Arc<HomeComponent>,
    target_submission: TargetSubmissionComponent,
    device_control: Arc<DeviceControl>,
    /// Shared emit handle for request-level journal events.
    journal: Journal,
    /// Shared admission and producer ownership, closed once by the composition root.
    lifecycle: ServerLifecycle,
}

pub(crate) struct LoadedServer {
    /// Shared components and handles passed to transports.
    pub(crate) state: ServerState,
    /// Owns runtime resources and coordinates shutdown.
    pub(crate) runtime: ServerRuntime,
    /// Migrated feature routes merged with legacy routes by `http::mount`.
    pub(crate) routes: AccessRoutes,
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
    ) -> Result<LoadedServer, StartupError> {
        let vault = vault::load(config.vault_master_key_path())
            .map_err(|source| StartupError::Vault { source })?;
        let ca_certificate_path = config
            .origin_ca_certificate_path()
            .map_err(|source| StartupError::Configuration { source })?;
        let ca_private_key_path = config
            .origin_ca_private_key_path()
            .map_err(|source| StartupError::Configuration { source })?;
        let gateway = GatewayComponent::load(
            database.clone(),
            &ca_certificate_path,
            &ca_private_key_path,
            config.local_origin_root_path(),
            config.site(),
        )
        .map_err(map_gateway_load_error)?;

        let print = PrintFeature::load(
            database.clone(),
            config.print_documents_path().to_path_buf(),
        )
        .await
        .context(PrintStorageSnafu)?;
        let photo_database = database.clone();
        let photo_path = config.photo_initializations_path().to_path_buf();
        let photo =
            tokio::task::spawn_blocking(move || PhotoComponent::new(photo_database, &photo_path))
                .await
                .map_err(|source| StartupError::PhotoStorage {
                    source: crate::component::photo::InitializationError::Worker { source },
                })?
                .map_err(|source| StartupError::PhotoStorage { source })?;
        let lifecycle = ServerLifecycle::new();
        let journal =
            JournalComponent::start(config.journal_path().to_owned(), config.journal().clone());
        let infra = Infra {
            database,
            vault: Arc::new(vault),
            journal: journal.emitter(),
            lifecycle,
        };
        Ok(Self::from_parts(
            infra,
            journal,
            &print,
            photo,
            gateway,
            config.organization_logos_path().to_path_buf(),
            utc_offset,
        ))
    }

    fn from_parts(
        infra: Infra,
        journal: JournalComponent,
        print_feature: &PrintFeature,
        photo: PhotoComponent,
        gateway: GatewayComponent,
        logos: PathBuf,
        utc_offset: time::UtcOffset,
    ) -> LoadedServer {
        let Infra {
            database,
            vault,
            journal: emitter,
            lifecycle,
        } = infra;
        let operator_feature = OperatorFeature::load(database.clone(), emitter.clone());
        let provisioning_feature = ProvisioningFeature::load();
        let device = Arc::new(DeviceComponent::new(database.clone(), emitter.clone()));
        let binding = Arc::new(BindingComponent::new(
            database.clone(),
            Arc::clone(&vault),
            emitter.clone(),
        ));
        let session = Arc::new(SessionControlComponent::new(database.clone()));
        let home = Arc::new(HomeComponent::new(database.clone()));
        let power = Arc::new(PowerControlComponent::new(database.clone()));
        let device_control = Arc::new(DeviceControl::new(DeviceControlDependencies {
            provisioning: Arc::clone(provisioning_feature.component()),
            device: Arc::clone(&device),
            gateway,
            binding: Arc::clone(&binding),
            runtime: RuntimeConfigComponent::new(database.clone()),
            session: Arc::clone(&session),
            home: Arc::clone(&home),
            power: Arc::clone(&power),
            journal: emitter.clone(),
            lifecycle: lifecycle.clone(),
        }));
        let journal_feature = JournalFeature::load(
            journal.reader(),
            Arc::clone(operator_feature.component()),
            device_control.live_lease_view(),
            utc_offset,
        );
        let routes = AccessRoutes::new()
            .merge(journal_feature.routes())
            .merge(print_feature.routes())
            .merge(operator_feature.routes())
            .merge(provisioning_feature.routes());
        let print = Arc::clone(print_feature.component());
        let runtime = ServerRuntime::new(lifecycle.clone(), Arc::clone(&device_control), journal);
        let state = Self {
            operator: Arc::clone(operator_feature.component()),
            print,
            photo,
            contest: ContestComponent::new(database.clone(), Arc::clone(&vault), logos),
            target_submission: TargetSubmissionComponent::new(database.clone(), emitter.clone()),
            import: ImportComponent::new(database, vault),
            device,
            binding,
            session,
            home,
            device_control,
            journal: emitter,
            lifecycle,
        };
        LoadedServer {
            state,
            runtime,
            routes,
        }
    }

    pub(crate) const fn journal(&self) -> &Journal {
        &self.journal
    }
    pub(crate) const fn lifecycle(&self) -> &ServerLifecycle {
        &self.lifecycle
    }
    pub(crate) const fn device_control(&self) -> &Arc<DeviceControl> {
        &self.device_control
    }

    pub(crate) const fn print(&self) -> &Arc<PrintComponent> {
        &self.print
    }

    pub(crate) fn photo_initialization(
        &self,
    ) -> crate::photo_initialization::PreparePhotoInitialization<'_> {
        crate::photo_initialization::PreparePhotoInitialization {
            photo: &self.photo,
            contest: &self.contest,
        }
    }
    pub(crate) const fn photo(&self) -> &PhotoComponent {
        &self.photo
    }

    pub(crate) fn operator(&self) -> &Arc<OperatorComponent> {
        &self.operator
    }

    pub(crate) const fn contest(&self) -> &ContestComponent {
        &self.contest
    }

    pub(crate) const fn import(&self) -> &ImportComponent {
        &self.import
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

fn map_gateway_load_error(error: GatewayLoadError) -> StartupError {
    match error {
        GatewayLoadError::TrustRootMismatch => StartupError::OriginCaTrustRootMismatch,
        GatewayLoadError::OriginCa { .. } => StartupError::OriginCa { source: error },
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::{fs, os::unix::fs::PermissionsExt as _, path::PathBuf, sync::Arc};

    use snafu::ResultExt as _;
    use uuid::Uuid;

    use super::{LoadedServer, ServerState, map_gateway_load_error};
    use crate::{
        component::{gateway::GatewayComponent, photo::PhotoComponent},
        db::Database,
        feature::{Infra, PrintStorageSnafu, StartupError, print::PrintFeature},
        journal::{CAPACITY, JournalComponent},
        lifecycle::ServerLifecycle,
        vault,
    };

    pub(crate) struct ServerStateFixture {
        /// Shared business database used by the test components.
        database: Database,
        /// Root directory for test print documents.
        documents: PathBuf,
        /// Root directory for test organization logos.
        logos: PathBuf,
        /// Optional journal owner supplied by the test.
        journal: Option<JournalComponent>,
    }

    impl ServerStateFixture {
        pub(crate) fn new(database: Database, documents: PathBuf) -> Self {
            Self {
                database,
                documents,
                logos: PathBuf::from("/nonexistent/natsume-test-logos"),
                journal: None,
            }
        }

        pub(crate) fn logos(mut self, logos: PathBuf) -> Self {
            self.logos = logos;
            self
        }

        pub(crate) fn journal(mut self, journal: JournalComponent) -> Self {
            self.journal = Some(journal);
            self
        }

        pub(crate) fn build(self) -> Result<LoadedServer, StartupError> {
            let Self {
                database,
                documents,
                logos,
                journal,
            } = self;
            let journal = journal.unwrap_or_else(|| JournalComponent::channel(CAPACITY).0);
            let root =
                std::env::temp_dir().join(format!("natsume-server-state-{}", Uuid::now_v7()));
            fs::create_dir(&root).map_err(|source| StartupError::Vault {
                source: vault::VaultError::Read { source },
            })?;
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).map_err(|source| {
                StartupError::Vault {
                    source: vault::VaultError::Read { source },
                }
            })?;
            let key_path = root.join("master.key");
            vault::ensure_master_key(&key_path).map_err(|source| StartupError::Vault { source })?;
            let vault = vault::load(&key_path).map_err(|source| StartupError::Vault { source })?;
            fs::remove_dir_all(root).map_err(|source| StartupError::Vault {
                source: vault::VaultError::Read { source },
            })?;
            let gateway =
                GatewayComponent::for_test(database.clone()).map_err(map_gateway_load_error)?;
            let photo_directory = documents.with_extension("photo-initializations");
            let print =
                PrintFeature::for_test(database.clone(), documents).context(PrintStorageSnafu)?;
            let photo = PhotoComponent::new(database.clone(), &photo_directory)
                .map_err(|source| StartupError::PhotoStorage { source })?;
            let infra = Infra {
                database,
                vault: Arc::new(vault),
                journal: journal.emitter(),
                lifecycle: ServerLifecycle::new(),
            };
            Ok(ServerState::from_parts(
                infra,
                journal,
                &print,
                photo,
                gateway,
                logos,
                time::UtcOffset::UTC,
            ))
        }
    }
}
