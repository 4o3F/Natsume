use clap::Subcommand;

mod bootstrap;
mod credentials;
mod error;
mod reset_operator_password;
mod serve;

pub use error::CommandError;
pub use serve::{router, run_until};

use crate::{config::ServerConfig, logging};

/// Server operating mode selected by the command line interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Subcommand)]
pub enum Command {
    Serve,
    Bootstrap,
    ResetOperatorPassword,
}

/// Initializes process logging and runs the selected Server command.
///
/// # Errors
///
/// Returns a redacted [`CommandError`] when logging or command execution fails.
pub async fn run(config: ServerConfig, command: Command) -> Result<(), CommandError> {
    logging::initialize(config.log_level()).map_err(|_| CommandError::Logging)?;
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

fn log_mode(mode: &'static str) {
    tracing::info!(mode, "server mode running");
}
