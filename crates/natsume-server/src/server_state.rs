use std::sync::Arc;

use snafu::ResultExt as _;

use crate::{
    component::{
        gateway::{GatewayComponent, GatewayLoadError},
        operator::OperatorComponent,
        print::PrintComponent,
    },
    config::ServerConfig,
    db::Database,
    device_control::DeviceControl,
    feature::{
        Infra, PhotoStorageSnafu, PrintStorageSnafu, StartupError, contest::ContestFeature,
        device::DeviceFeature, import::ImportFeature, journal::JournalFeature,
        operator::OperatorFeature, photo::PhotoFeature, print::PrintFeature,
        provisioning::ProvisioningFeature,
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
    /// Transitional handle used only by tests; removed in Step 11.
    #[cfg_attr(not(test), allow(dead_code))]
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
        let vault = Arc::new(
            vault::load(config.vault_master_key_path())
                .map_err(|source| StartupError::Vault { source })?,
        );
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
        let contest = ContestFeature::load(
            database.clone(),
            Arc::clone(&vault),
            config.organization_logos_path().to_path_buf(),
        );
        let photo = PhotoFeature::load(
            database.clone(),
            config.photo_initializations_path().to_path_buf(),
            Arc::clone(contest.component()),
        )
        .await
        .context(PhotoStorageSnafu)?;
        let lifecycle = ServerLifecycle::new();
        let journal =
            JournalComponent::start(config.journal_path().to_owned(), config.journal().clone());
        let infra = Infra {
            database,
            vault,
            journal: journal.emitter(),
            lifecycle,
        };
        let (loaded, _) = Self::from_parts(
            infra, journal, &print, &contest, &photo, gateway, utc_offset,
        );
        Ok(loaded)
    }

    fn from_parts(
        infra: Infra,
        journal: JournalComponent,
        print_feature: &PrintFeature,
        contest: &ContestFeature,
        photo: &PhotoFeature,
        gateway: GatewayComponent,
        utc_offset: time::UtcOffset,
    ) -> (LoadedServer, ImportFeature) {
        let operator_feature = OperatorFeature::load(infra.database.clone(), infra.journal.clone());
        let provisioning_feature = ProvisioningFeature::load();
        let device_feature = DeviceFeature::load(
            &infra,
            gateway,
            Arc::clone(provisioning_feature.component()),
        );
        let Infra {
            database,
            vault,
            journal: emitter,
            lifecycle,
        } = infra;
        let import_feature = ImportFeature::load(
            database.clone(),
            Arc::clone(&vault),
            Arc::clone(contest.component()),
            Arc::clone(device_feature.device_control()),
        );
        let journal_feature = JournalFeature::load(
            journal.reader(),
            Arc::clone(operator_feature.component()),
            device_feature.device_control().live_lease_view(),
            utc_offset,
        );
        let routes = AccessRoutes::new()
            .merge(journal_feature.routes())
            .merge(print_feature.routes())
            .merge(operator_feature.routes())
            .merge(provisioning_feature.routes())
            .merge(contest.routes())
            .merge(photo.routes())
            .merge(device_feature.routes())
            .merge(import_feature.routes());
        let print = Arc::clone(print_feature.component());
        let runtime = ServerRuntime::new(
            lifecycle.clone(),
            Arc::clone(device_feature.device_control()),
            journal,
        );
        let state = Self {
            operator: Arc::clone(operator_feature.component()),
            print,
            device_control: Arc::clone(device_feature.device_control()),
            journal: emitter,
            lifecycle,
        };
        (
            LoadedServer {
                state,
                runtime,
                routes,
            },
            import_feature,
        )
    }

    pub(crate) const fn journal(&self) -> &Journal {
        &self.journal
    }
    pub(crate) const fn lifecycle(&self) -> &ServerLifecycle {
        &self.lifecycle
    }
    #[cfg(test)]
    pub(crate) const fn device_control(&self) -> &Arc<DeviceControl> {
        &self.device_control
    }

    pub(crate) const fn print(&self) -> &Arc<PrintComponent> {
        &self.print
    }

    pub(crate) fn operator(&self) -> &Arc<OperatorComponent> {
        &self.operator
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
        component::gateway::GatewayComponent,
        db::Database,
        feature::{
            Infra, PhotoStorageSnafu, PrintStorageSnafu, StartupError, contest::ContestFeature,
            import::ImportFeature, photo::PhotoFeature, print::PrintFeature,
        },
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

    pub(crate) struct ServerWithFeatures {
        /// Complete server with production feature routes.
        pub(crate) loaded: LoadedServer,
        /// Contest owner shared with Photo and Import candidate routes.
        pub(crate) contest: ContestFeature,
        /// Photo owner used by the routes and test setup.
        pub(crate) photo: PhotoFeature,
        /// Import owner used by the routes and test setup.
        pub(crate) import: ImportFeature,
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
            Ok(self.build_with_features()?.loaded)
        }

        pub(crate) fn build_with_features(self) -> Result<ServerWithFeatures, StartupError> {
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
            let infra = Infra {
                database,
                vault: Arc::new(vault),
                journal: journal.emitter(),
                lifecycle: ServerLifecycle::new(),
            };
            let contest =
                ContestFeature::load(infra.database.clone(), Arc::clone(&infra.vault), logos);
            let photo = PhotoFeature::for_test(
                infra.database.clone(),
                &photo_directory,
                Arc::clone(contest.component()),
            )
            .context(PhotoStorageSnafu)?;
            let (loaded, import) = ServerState::from_parts(
                infra,
                journal,
                &print,
                &contest,
                &photo,
                gateway,
                time::UtcOffset::UTC,
            );
            let features = ServerWithFeatures {
                loaded,
                contest,
                photo,
                import,
            };
            assert!(
                Arc::ptr_eq(features.contest.component(), features.import.contest()),
                "Import routes must share the Contest feature instance"
            );
            Ok(features)
        }
    }
}
