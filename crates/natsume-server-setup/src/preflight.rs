//! Host fact collection separated from deterministic first-install eligibility checks.

use crate::{
    account::{AccountError, SYSTEM_PATH, ServiceAccount},
    config::SERVER_BINARY,
};
use snafu::{ResultExt as _, Snafu};
use std::{io, path::Path, process::Command};

/// Observations gathered without modifying service or deployment state.
#[derive(Debug)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent host observations are the specified injectable fact boundary"
)]
pub struct SystemFacts {
    /// Current effective Unix identity.
    pub effective_uid: u32,
    /// Rust target architecture of the running installer.
    pub architecture: String,
    /// Whether systemd's runtime directory exists.
    pub systemd_running: bool,
    /// Whether interactive input is attached to a terminal.
    pub stdin_is_terminal: bool,
    /// Whether interactive output is attached to a terminal.
    pub stdout_is_terminal: bool,
    /// Whether prompt rendering and answer context remain on a terminal.
    pub stderr_is_terminal: bool,
    /// Whether the packaged Server executable exists.
    pub server_binary_present: bool,
    /// Account lookup result retained with its original failure.
    pub service_account: Result<ServiceAccount, AccountError>,
    /// NTP synchronization state, absent when timedatectl could not establish it.
    pub clock_synchronized: Option<bool>,
}

impl SystemFacts {
    /// Collects local system observations without requiring an interactive prompt.
    #[must_use]
    pub fn collect(account_name: &str) -> Self {
        let clock_synchronized = Command::new("/usr/bin/timedatectl")
            .args(["show", "-p", "NTPSynchronized", "--value"])
            .env_clear()
            .env("PATH", SYSTEM_PATH)
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| match output.stdout.trim_ascii() {
                b"yes" => Some(true),
                b"no" => Some(false),
                _ => None,
            });
        Self {
            effective_uid: rustix::process::geteuid().as_raw(),
            architecture: std::env::consts::ARCH.to_owned(),
            systemd_running: Path::new("/run/systemd/system").is_dir(),
            stdin_is_terminal: rustix::termios::isatty(io::stdin()),
            stdout_is_terminal: rustix::termios::isatty(io::stdout()),
            stderr_is_terminal: rustix::termios::isatty(io::stderr()),
            server_binary_present: Path::new(SERVER_BINARY).is_file(),
            service_account: ServiceAccount::lookup(account_name),
            clock_synchronized,
        }
    }
}

/// Nonfatal host conditions requiring administrator attention.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreflightWarning {
    /// Incorrect time can invalidate generated and imported certificates.
    ClockNotSynchronized,
    /// The system did not report a reliable synchronization state.
    ClockSynchronizationUnknown,
}

/// Eligible host context and warnings to present before collecting installation input.
#[derive(Debug)]
pub struct PreflightReport {
    /// Conditions that do not prevent installation.
    pub warnings: Vec<PreflightWarning>,
    /// Verified packaged non-root service identity.
    pub account: ServiceAccount,
}

/// Checks first-install prerequisites using only the supplied observations.
///
/// # Errors
/// Rejects unsupported identity, architecture, init system, terminal, or package state.
pub fn evaluate(facts: SystemFacts) -> Result<PreflightReport, PreflightError> {
    snafu::ensure!(facts.effective_uid == 0, NotRootSnafu);
    snafu::ensure!(facts.architecture == "x86_64", ArchitectureSnafu);
    snafu::ensure!(facts.systemd_running, SystemdSnafu);
    snafu::ensure!(
        facts.stdin_is_terminal && facts.stdout_is_terminal && facts.stderr_is_terminal,
        TerminalSnafu
    );
    snafu::ensure!(facts.server_binary_present, BinaryMissingSnafu);
    let account = facts.service_account.context(AccountSnafu)?;
    let warnings = match facts.clock_synchronized {
        Some(true) => vec![],
        Some(false) => vec![PreflightWarning::ClockNotSynchronized],
        None => vec![PreflightWarning::ClockSynchronizationUnknown],
    };
    Ok(PreflightReport { warnings, account })
}

/// A prerequisite preventing safe first installation.
#[derive(Debug, Snafu)]
pub enum PreflightError {
    /// System deployment paths require root ownership.
    #[snafu(display("run setup as root"))]
    NotRoot,
    /// The packaged Server currently targets amd64 only.
    #[snafu(display("setup requires x86_64 (amd64)"))]
    Architecture,
    /// Setup requires the packaged systemd service lifecycle.
    #[snafu(display("systemd is not running"))]
    Systemd,
    /// Credentials must be entered through an interactive terminal.
    #[snafu(display("stdin, stdout and stderr must all be terminals"))]
    Terminal,
    /// The Server package has not supplied its executable.
    #[snafu(display("Server binary missing; install the Server Deb first"))]
    BinaryMissing,
    /// Account provisioning failed or returned an invalid record.
    #[snafu(display("service account unavailable; install the Server Deb first"))]
    Account {
        /// Original account lookup failure.
        source: AccountError,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    fn facts() -> SystemFacts {
        SystemFacts {
            effective_uid: 0,
            architecture: "x86_64".into(),
            systemd_running: true,
            stdin_is_terminal: true,
            stdout_is_terminal: true,
            stderr_is_terminal: true,
            server_binary_present: true,
            service_account: Ok(ServiceAccount {
                name: crate::account::SERVICE_USER.into(),
                uid: 123,
                gid: 123,
                home: "/var/lib/natsume-server".into(),
            }),
            clock_synchronized: Some(true),
        }
    }
    #[test]
    fn every_fatal_prerequisite_is_enforced() {
        let mut host = facts();
        host.effective_uid = 1;
        assert!(matches!(evaluate(host), Err(PreflightError::NotRoot)));
        let mut host = facts();
        host.architecture = "aarch64".into();
        assert!(matches!(evaluate(host), Err(PreflightError::Architecture)));
        let mut host = facts();
        host.systemd_running = false;
        assert!(matches!(evaluate(host), Err(PreflightError::Systemd)));
        let mut host = facts();
        host.stdin_is_terminal = false;
        assert!(matches!(evaluate(host), Err(PreflightError::Terminal)));
        let mut host = facts();
        host.stdout_is_terminal = false;
        assert!(matches!(evaluate(host), Err(PreflightError::Terminal)));
        let mut host = facts();
        host.stderr_is_terminal = false;
        assert!(matches!(evaluate(host), Err(PreflightError::Terminal)));
        let mut host = facts();
        host.server_binary_present = false;
        assert!(matches!(evaluate(host), Err(PreflightError::BinaryMissing)));
        let mut host = facts();
        host.service_account = Err(AccountError::Missing);
        assert!(matches!(
            evaluate(host),
            Err(PreflightError::Account { .. })
        ));
    }
    #[test]
    fn clock_conditions_are_warnings() {
        for (clock, expected) in [
            (Some(true), vec![]),
            (Some(false), vec![PreflightWarning::ClockNotSynchronized]),
            (None, vec![PreflightWarning::ClockSynchronizationUnknown]),
        ] {
            let mut host = facts();
            host.clock_synchronized = clock;
            let report = evaluate(host);
            assert!(report.is_ok());
            if let Ok(report) = report {
                assert_eq!(report.warnings, expected);
                assert_eq!(report.account.uid, 123);
            }
        }
    }
}
