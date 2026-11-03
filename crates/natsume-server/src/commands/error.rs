use snafu::Snafu;

#[derive(Debug, Snafu)]
#[snafu(context(false), display("{source}"))]
pub struct CommandError {
    source: CommandFailure,
}

/// Redacted failure while starting or running the server.
#[derive(Debug, Snafu)]
pub(super) enum CommandFailure {
    #[snafu(visibility(pub(super)), display("{source}"))]
    Startup {
        source: crate::feature::StartupError,
    },
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
    RuntimeConfig {
        source: crate::component::runtime::RuntimeConfigError,
    },
    #[snafu(display("vault startup failed"))]
    Vault { source: crate::vault::VaultError },
    #[snafu(display("web assets startup failed"))]
    WebAssets,
    #[snafu(display("TLS startup failed"))]
    Tls { source: crate::tls::TlsError },
    #[snafu(visibility(pub(super)), display("{source}"))]
    Runtime {
        source: crate::server_runtime::RuntimeError,
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
