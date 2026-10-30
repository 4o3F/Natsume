//! Read-only deployment checks executed strictly under the packaged service identity.

use crate::{
    account::{SERVICE_USER, ServiceAccount},
    cli::ProbeCommand,
    config::CONFIG_PATH,
};
use natsume_server::{
    BootstrapStatus, config::ServerConfig, inspect_bootstrap, validate_deployment,
};

/// Shared child-process protocol used by setup and its hidden probe command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeExit {
    /// Deployment validation passed, or bootstrap is already complete.
    Passed,
    /// The operation failed without a usable inspection result.
    Failed,
    /// The command was invoked with invalid arguments.
    Usage,
    /// The existing database still requires credential initialization.
    NotInitialized,
    /// No database exists at the configured path.
    DatabaseMissing,
}

impl ProbeExit {
    /// Explains the status to an administrator reading a setup failure.
    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            Self::Passed => "the check passed",
            Self::Failed => "the Server reported an error; see its message above",
            Self::Usage => {
                "the Server rejected the check command; the installed Server and setup may be different versions"
            }
            Self::NotInitialized => "the database exists but has no administrator account",
            Self::DatabaseMissing => "the Server database does not exist",
        }
    }
    /// Encodes the shared protocol as a process exit status.
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::Passed => 0,
            Self::Failed => 1,
            Self::Usage => 2,
            Self::NotInitialized => 10,
            Self::DatabaseMissing => 11,
        }
    }
    /// Rejects statuses that are not part of the shared probe protocol.
    #[must_use]
    pub const fn from_code(code: i32) -> Option<Self> {
        match code {
            0 => Some(Self::Passed),
            1 => Some(Self::Failed),
            2 => Some(Self::Usage),
            10 => Some(Self::NotInitialized),
            11 => Some(Self::DatabaseMissing),
            _ => None,
        }
    }
}

impl From<BootstrapStatus> for ProbeExit {
    fn from(status: BootstrapStatus) -> Self {
        match status {
            BootstrapStatus::Initialized => Self::Passed,
            BootstrapStatus::NeedsBootstrap => Self::NotInitialized,
            BootstrapStatus::DatabaseMissing => Self::DatabaseMissing,
        }
    }
}

/// Checks caller identity before loading deployment inputs or inspecting the database.
#[must_use]
pub fn run(command: ProbeCommand) -> ProbeExit {
    let account = match ServiceAccount::lookup(SERVICE_USER) {
        Ok(account) => account,
        Err(error) => {
            tracing::error!(error = %snafu::Report::from_error(error), "probe service identity lookup failed");
            return ProbeExit::Failed;
        }
    };
    let uid = rustix::process::geteuid().as_raw();
    if uid == 0 || uid != account.uid {
        tracing::error!("probe must be invoked by setup as the natsume-server service user");
        return ProbeExit::Failed;
    }
    let config = match ServerConfig::load_from(std::path::Path::new(CONFIG_PATH)) {
        Ok(config) => config,
        Err(error) => {
            tracing::error!(error = %snafu::Report::from_error(error), "probe configuration failed");
            return ProbeExit::Failed;
        }
    };
    let result = match command {
        ProbeCommand::Deployment => validate_deployment(&config).map(|()| ProbeExit::Passed),
        ProbeCommand::Bootstrap => {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    tracing::error!(error = %snafu::Report::from_error(error), "probe runtime creation failed");
                    return ProbeExit::Failed;
                }
            };
            runtime
                .block_on(inspect_bootstrap(&config))
                .map(ProbeExit::from)
        }
    };
    match result {
        Ok(exit) => {
            tracing::info!(result = ?exit, "probe completed");
            exit
        }
        Err(error) => {
            tracing::error!(error = %snafu::Report::from_error(error), "probe failed");
            ProbeExit::Failed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bootstrap_states_and_protocol_roundtrip() {
        assert_eq!(
            ProbeExit::from(BootstrapStatus::Initialized),
            ProbeExit::Passed
        );
        assert_eq!(
            ProbeExit::from(BootstrapStatus::NeedsBootstrap),
            ProbeExit::NotInitialized
        );
        assert_eq!(
            ProbeExit::from(BootstrapStatus::DatabaseMissing),
            ProbeExit::DatabaseMissing
        );
        for exit in [
            ProbeExit::Passed,
            ProbeExit::Failed,
            ProbeExit::Usage,
            ProbeExit::NotInitialized,
            ProbeExit::DatabaseMissing,
        ] {
            assert_eq!(ProbeExit::from_code(i32::from(exit.code())), Some(exit));
        }
        for code in [-1, 3, 12, 255] {
            assert_eq!(ProbeExit::from_code(code), None);
        }
    }
}
