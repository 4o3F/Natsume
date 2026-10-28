use snafu::Snafu;

#[derive(Debug, Snafu)]
#[snafu(context(false), display("{source}"))]
pub struct CommandError {
    source: CommandFailure,
}

use crate::server_state::ServerStateError;

/// Redacted failure while starting or running the server.
#[derive(Debug, Snafu)]
pub(super) enum CommandFailure {
    #[snafu(display("server configuration failed"))]
    Configuration { source: ServerStateError },
    #[snafu(display("structured logging startup failed"))]
    Logging {
        source: crate::logging::LoggingError,
    },
    #[snafu(display("database startup failed"))]
    Database { source: crate::db::DatabaseError },
    #[snafu(display("database startup failed"))]
    Persistence { source: crate::db::PersistenceError },
    #[snafu(display("database startup failed"))]
    Operator {
        source: crate::component::operator::OperatorError,
    },
    #[snafu(display("server configuration failed"))]
    Runtime {
        source: crate::component::runtime::RuntimeConfigError,
    },
    #[snafu(display("vault startup failed"))]
    Vault { source: crate::vault::VaultError },
    #[snafu(display("print document storage startup failed"))]
    PrintStorage { source: ServerStateError },
    #[snafu(display("web assets startup failed"))]
    WebAssets,
    #[snafu(display("TLS startup failed"))]
    Tls { source: crate::tls::TlsError },
    #[snafu(display("Origin CA startup failed"))]
    OriginCa { source: ServerStateError },
    #[snafu(display("Origin CA issuing certificate and packaged trust root differ"))]
    OriginCaTrustRootMismatch,
    #[snafu(display("HTTP serving failed"))]
    Http { source: std::io::Error },
    #[snafu(display("HTTP serving task failed"))]
    ServingTask {
        #[snafu(source(from(tokio::task::JoinError, crate::task::TaskFailure::from)))]
        source: crate::task::TaskFailure,
    },
    #[snafu(display("server background task shutdown failed"))]
    Background {
        source: crate::background::BackgroundError,
    },
    #[snafu(display("shutdown signal setup failed"))]
    Signal { source: std::io::Error },
    #[snafu(display("server bootstrap failed"))]
    Bootstrap { source: WorkError },
    #[snafu(display("server database has not been bootstrapped"))]
    NotBootstrapped,
    #[snafu(display("operator password reset failed"))]
    PasswordReset { source: WorkError },
}

impl From<ServerStateError> for CommandFailure {
    fn from(source: ServerStateError) -> Self {
        match source {
            ServerStateError::Configuration { .. } => Self::Configuration { source },
            ServerStateError::Vault { source } => Self::Vault { source },
            ServerStateError::PrintStorage { .. } => Self::PrintStorage { source },
            ServerStateError::OriginCa { .. } => Self::OriginCa { source },
            ServerStateError::OriginCaTrustRootMismatch => Self::OriginCaTrustRootMismatch,
        }
    }
}

impl From<crate::db::PersistenceError> for CommandFailure {
    fn from(source: crate::db::PersistenceError) -> Self {
        Self::Persistence { source }
    }
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub(super) enum WorkError {
    #[snafu(display("operator operation failed"))]
    Operator {
        source: crate::component::operator::OperatorError,
    },
    #[snafu(display("runtime configuration failed"))]
    Runtime {
        source: crate::component::runtime::RuntimeConfigError,
    },
    #[snafu(display("database operation failed"))]
    Persistence { source: crate::db::PersistenceError },
    #[snafu(display("terminal input failed"))]
    Io { source: std::io::Error },
}
impl From<crate::db::PersistenceError> for WorkError {
    fn from(source: crate::db::PersistenceError) -> Self {
        Self::Persistence { source }
    }
}
