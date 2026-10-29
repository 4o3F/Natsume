//! Fixed systemd lifecycle operations for the packaged Server unit.

use crate::{account::SYSTEM_PATH, config::SERVICE_UNIT};
use snafu::{ResultExt as _, Snafu, ensure};
use std::process::{Command, ExitStatus, Stdio};

/// Lifecycle operations permitted during first installation and rollback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceAction {
    /// Makes systemd reread packaged unit files.
    DaemonReload,
    /// Installs boot-time activation links for Server.
    Enable,
    /// Undoes Enable when rolling back a failed first installation.
    Disable,
    /// Starts the newly deployed Server.
    Start,
    /// Stops Server before failed installation files are removed.
    Stop,
}

impl ServiceAction {
    /// Returns fixed systemctl arguments without shell interpretation.
    #[must_use]
    pub fn arguments(self) -> Vec<&'static str> {
        match self {
            Self::DaemonReload => vec!["daemon-reload"],
            Self::Enable => vec!["enable", SERVICE_UNIT],
            Self::Disable => vec!["disable", SERVICE_UNIT],
            Self::Start => vec!["start", SERVICE_UNIT],
            Self::Stop => vec!["stop", SERVICE_UNIT],
        }
    }
}

/// Runs systemctl, preserving its stderr diagnostics for the administrator.
///
/// # Errors
/// Returns process launch or nonzero exit status failures with the requested action.
pub fn systemctl(action: ServiceAction) -> Result<(), ServiceError> {
    let status = Command::new("/usr/bin/systemctl")
        .args(action.arguments())
        .env_clear()
        .env("PATH", SYSTEM_PATH)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .context(LaunchSnafu { action })?;
    ensure!(status.success(), StatusSnafu { action, status });
    Ok(())
}

/// Systemd failed to apply a requested installation action.
#[derive(Debug, Snafu)]
pub enum ServiceError {
    /// The OS could not launch systemctl.
    #[snafu(display("cannot launch systemctl for {action:?}"))]
    Launch {
        /// Installation operation being performed.
        action: ServiceAction,
        /// Original process creation failure.
        source: std::io::Error,
    },
    /// Systemctl reported failure or signal termination.
    #[snafu(display("systemctl {action:?} failed: {status}"))]
    Status {
        /// Installation operation being performed.
        action: ServiceAction,
        /// Original status including signal termination.
        status: ExitStatus,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_fixed_unit_actions_are_constructed() {
        assert_eq!(ServiceAction::DaemonReload.arguments(), ["daemon-reload"]);
        assert_eq!(
            ServiceAction::Enable.arguments(),
            ["enable", "natsume-server.service"]
        );
        assert_eq!(
            ServiceAction::Disable.arguments(),
            ["disable", "natsume-server.service"]
        );
        assert_eq!(
            ServiceAction::Start.arguments(),
            ["start", "natsume-server.service"]
        );
        assert_eq!(
            ServiceAction::Stop.arguments(),
            ["stop", "natsume-server.service"]
        );
    }
}
