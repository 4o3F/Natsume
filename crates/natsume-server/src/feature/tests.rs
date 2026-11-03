use std::{fs, os::unix::fs::PermissionsExt as _, path::PathBuf, sync::Arc};

use snafu::ResultExt as _;
use uuid::Uuid;

use super::{LoadedResources, LoadedServer, assemble, map_gateway_load_error};
use crate::{
    component::gateway::GatewayComponent,
    db::Database,
    feature::{
        Infra, PhotoStorageSnafu, PrintStorageSnafu, StartupError, contest::ContestFeature,
        import::ImportFeature, journal::JournalFeature, photo::PhotoFeature, print::PrintFeature,
    },
    journal::{CAPACITY, JournalComponent},
    lifecycle::ServerLifecycle,
    vault,
};

pub(crate) struct ServerFixture {
    /// Shared business database used by the test components.
    database: Database,
    /// Root directory for test print documents.
    documents: PathBuf,
    /// Root directory for test organization logos.
    logos: PathBuf,
    /// Optional journal owner supplied by the test.
    journal: Option<JournalComponent>,
}

pub(crate) struct TestServer {
    /// Complete server with production feature routes.
    pub(crate) loaded: LoadedServer,
    /// Contest owner shared with Photo and Import candidate routes.
    pub(crate) contest: ContestFeature,
    /// Photo owner used by the routes and test setup.
    pub(crate) photo: PhotoFeature,
    /// Import owner used by the routes and test setup.
    pub(crate) import: ImportFeature,
    /// Journal query feature shared with the mounted routes.
    pub(crate) journal: JournalFeature,
}

struct CapturedAssembly {
    loaded: LoadedServer,
    import: ImportFeature,
    journal: JournalFeature,
}

impl ServerFixture {
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

    pub(crate) fn build_with_features(self) -> Result<TestServer, StartupError> {
        let Self {
            database,
            documents,
            logos,
            journal,
        } = self;
        let journal = journal.unwrap_or_else(|| JournalComponent::channel(CAPACITY).0);
        let root = std::env::temp_dir().join(format!("natsume-server-fixture-{}", Uuid::now_v7()));
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
        let photo_results_directory = documents.with_extension("photo-results");
        let print =
            PrintFeature::for_test(database.clone(), documents).context(PrintStorageSnafu)?;
        let infra = Infra {
            database,
            vault: Arc::new(vault),
            journal: journal.emitter(),
            lifecycle: ServerLifecycle::new(),
        };
        let contest = ContestFeature::load(infra.database.clone(), Arc::clone(&infra.vault), logos);
        let photo = PhotoFeature::for_test(
            infra.database.clone(),
            &photo_directory,
            &photo_results_directory,
            Arc::clone(contest.component()),
        )
        .context(PhotoStorageSnafu)?;
        let captured = assemble(
            LoadedResources {
                infra,
                journal,
                gateway,
            },
            &print,
            &contest,
            &photo,
            time::UtcOffset::UTC,
            |loaded, import, journal| CapturedAssembly {
                loaded,
                import,
                journal,
            },
        );
        let features = TestServer {
            loaded: captured.loaded,
            contest,
            photo,
            import: captured.import,
            journal: captured.journal,
        };
        assert!(
            Arc::ptr_eq(features.contest.component(), features.import.contest()),
            "Import routes must share the Contest feature instance"
        );
        Ok(features)
    }
}
