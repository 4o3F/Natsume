//! Server composition layer; future `<Name>Feature` types belong in `feature/` submodules.

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
    #[snafu(display("Photo storage startup failed"))]
    PhotoStorage {
        source: crate::component::photo::InitializationError,
    },
    #[snafu(display("server configuration failed"))]
    Configuration { source: crate::config::ConfigError },
    #[snafu(display("vault startup failed"))]
    Vault { source: crate::vault::VaultError },
    #[snafu(display("print document storage startup failed"))]
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
