use clap::Subcommand;
use tracing::Instrument as _;

mod bootstrap;
mod credentials;
mod error;
mod reset_operator_password;
mod serve;

pub use crate::logging::Logging;
pub use error::CommandError;
/// Builds the mounted Server HTTP surface over an already-bootstrapped database.
///
/// # Errors
///
/// Returns a redacted [`CommandError`] when startup infrastructure cannot be loaded.
pub async fn router(
    config: ServerConfig,
    web_root: &std::path::Path,
) -> Result<axum::Router, CommandError> {
    serve::router(config, web_root)
        .await
        .map_err(CommandError::from)
}

/// Runs the production Server stack until the supplied shutdown future resolves.
///
/// # Errors
///
/// Returns a redacted [`CommandError`] when a startup or serving stage fails.
pub async fn run_until<F>(config: ServerConfig, shutdown: F) -> Result<(), CommandError>
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    serve::run_until(config, shutdown)
        .await
        .map_err(CommandError::from)
}

use crate::{config::ServerConfig, logging};

/// Server operating mode selected by the command line interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Subcommand)]
pub enum Command {
    Serve,
    Bootstrap,
    ResetOperatorPassword,
}

impl Command {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Serve => "serve",
            Self::Bootstrap => "bootstrap",
            Self::ResetOperatorPassword => "reset-operator-password",
        }
    }
}

/// Installs stderr logging before command-line parsing or configuration loading.
///
/// # Errors
///
/// Returns a typed error if a process subscriber has already been installed.
pub fn bootstrap_logging() -> Result<Logging, CommandError> {
    logging::bootstrap().map_err(|source| error::CommandFailure::Logging { source }.into())
}

/// Configures the startup subscriber and runs the selected Server command.
///
/// # Errors
///
/// Returns a redacted [`CommandError`] when logging or command execution fails.
pub async fn run(
    config: ServerConfig,
    command: Command,
    logging: Logging,
) -> Result<(), CommandError> {
    let telemetry = logging::initialize(logging, config.log_level())
        .map_err(|source| error::CommandFailure::Logging { source })?;
    let root_span = tracing::info_span!(
        "server_command",
        "otel.kind" = "internal",
        "command.name" = command.as_str(),
    );
    let result = async move {
        match command {
            Command::Serve => {
                log_mode("serve");
                serve::execute(config).await
            }
            Command::Bootstrap => {
                log_mode("bootstrap");
                bootstrap::execute(config).await
            }
            Command::ResetOperatorPassword => {
                log_mode("reset-operator-password");
                reset_operator_password::execute(config).await
            }
        }
    }
    .instrument(root_span)
    .await;
    telemetry.shutdown();
    result.map_err(CommandError::from)
}

fn log_mode(mode: &'static str) {
    tracing::info!(mode, "server mode running");
}
