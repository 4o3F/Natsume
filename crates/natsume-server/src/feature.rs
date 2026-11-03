//! Server resource loading and explicit feature composition.

pub(crate) mod contest;
pub(crate) mod device;
pub(crate) mod import;
pub(crate) mod journal;
pub(crate) mod operator;
pub(crate) mod photo;
pub(crate) mod print;
pub(crate) mod provisioning;

use std::sync::Arc;

use snafu::{ResultExt as _, Snafu};

use self::{
    contest::ContestFeature, device::DeviceFeature, import::ImportFeature, journal::JournalFeature,
    operator::OperatorFeature, photo::PhotoFeature, print::PrintFeature,
    provisioning::ProvisioningFeature,
};
use crate::{
    component::{
        gateway::{GatewayComponent, GatewayLoadError},
        operator::OperatorComponent,
        print::PrintComponent,
    },
    config::ServerConfig,
    db::Database,
    http::AccessRoutes,
    journal::{Journal, JournalComponent},
    lifecycle::ServerLifecycle,
    server_runtime::ServerRuntime,
    vault::{self, VaultSession},
};

#[derive(Clone)]
pub(crate) struct Infra {
    /// Shared business database handle.
    pub(crate) database: Database,
    /// Shared unlocked vault session.
    pub(crate) vault: Arc<VaultSession>,
    /// Emit handle from the process journal writer.
    pub(crate) journal: Journal,
    /// Shared admission tracking and shutdown notification.
    pub(crate) lifecycle: ServerLifecycle,
}

#[derive(Debug, Snafu)]
pub(crate) enum StartupError {
    /// Storage startup failure owned by `PhotoFeature`.
    #[snafu(visibility(pub(crate)), display("Photo storage startup failed"))]
    PhotoStorage {
        source: crate::component::photo::PhotoStorageError,
    },
    #[snafu(display("server configuration failed"))]
    Configuration { source: crate::config::ConfigError },
    #[snafu(display("vault startup failed"))]
    Vault { source: crate::vault::VaultError },
    /// Storage startup failure owned by `PrintFeature`.
    #[snafu(
        visibility(pub(crate)),
        display("print document storage startup failed")
    )]
    PrintStorage {
        source: crate::component::print::PrintError,
    },
    #[snafu(display("Origin CA startup failed"))]
    OriginCa {
        source: crate::component::gateway::GatewayLoadError,
    },
    #[snafu(display("Origin CA issuing certificate and packaged trust root differ"))]
    OriginCaTrustRootMismatch,
}

pub(crate) struct LoadedServer {
    /// Owns runtime resources and coordinates shutdown.
    pub(crate) runtime: ServerRuntime,
    /// Feature routes bound to their shared narrow states.
    pub(crate) routes: AccessRoutes,
    /// Shared Operator instance used by HTTP authentication.
    pub(crate) operator: Arc<OperatorComponent>,
    /// Shared Print instance used by authentication and background tasks.
    pub(crate) print: Arc<PrintComponent>,
    /// Emit handle for HTTP audit and completion events.
    pub(crate) journal: Journal,
    /// Shared HTTP mutation admission and producer tracking.
    pub(crate) lifecycle: ServerLifecycle,
}

struct LoadedResources {
    /// Shared database, unlocked vault, journal emitter and lifecycle.
    infra: Infra,
    /// Unique journal writer owner transferred to the runtime.
    journal: JournalComponent,
    /// Gateway with its loaded Origin CA.
    gateway: GatewayComponent,
}

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
        config.photo_results_path().to_path_buf(),
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
    Ok(assemble(
        LoadedResources {
            infra,
            journal,
            gateway,
        },
        &print,
        &contest,
        &photo,
        utc_offset,
        |loaded, _import, _journal| loaded,
    ))
}

fn assemble<R>(
    resources: LoadedResources,
    print_feature: &PrintFeature,
    contest: &ContestFeature,
    photo: &PhotoFeature,
    utc_offset: time::UtcOffset,
    finish: impl FnOnce(LoadedServer, ImportFeature, JournalFeature) -> R,
) -> R {
    let LoadedResources {
        infra,
        journal,
        gateway,
    } = resources;
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

    finish(
        LoadedServer {
            runtime,
            routes,
            operator: Arc::clone(operator_feature.component()),
            print,
            journal: emitter,
            lifecycle,
        },
        import_feature,
        journal_feature,
    )
}

impl LoadedServer {
    #[cfg(test)]
    pub(crate) fn http_router(&self, web_root: &std::path::Path) -> axum::Router {
        crate::http::mount(
            self.routes.clone(),
            Arc::clone(&self.operator),
            Arc::clone(&self.print),
            self.journal.clone(),
            self.lifecycle.clone(),
            web_root,
        )
    }
}

fn map_gateway_load_error(error: GatewayLoadError) -> StartupError {
    match error {
        GatewayLoadError::TrustRootMismatch => StartupError::OriginCaTrustRootMismatch,
        GatewayLoadError::OriginCa { .. } => StartupError::OriginCa { source: error },
    }
}

#[cfg(test)]
pub(crate) mod tests;
