//! Server composition layer; future `<Name>Feature` types belong in `feature/` submodules.

pub(crate) mod contest;
pub(crate) mod device;
pub(crate) mod import;
pub(crate) mod journal;
pub(crate) mod operator;
pub(crate) mod photo;
pub(crate) mod print;
pub(crate) mod provisioning;

use std::sync::Arc;

use snafu::Snafu;

use crate::{db::Database, journal::Journal, lifecycle::ServerLifecycle, vault::VaultSession};

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
        source: crate::component::photo::InitializationError,
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
