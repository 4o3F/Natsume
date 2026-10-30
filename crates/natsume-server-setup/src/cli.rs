//! Command-line boundary for the first-install tool and its service-user probes.

use crate::config::DEFAULT_PKI_DIRECTORY;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// Administrator options; credentials are collected only through the terminal.
#[derive(Debug, Parser)]
#[command(name = "natsume-server-setup")]
pub struct Cli {
    /// Administrator PKI storage, separate from the online service keys.
    #[arg(long, default_value = DEFAULT_PKI_DIRECTORY)]
    pub pki_dir: PathBuf,
    /// Internal service-user operation, absent for interactive installation.
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// Internal commands started after dropping service-user privileges.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Performs a read-only check under the packaged service identity.
    #[command(hide = true, subcommand)]
    Probe(#[doc = "Deployment boundary inspected by the service-user child."] ProbeCommand),
}

/// Checks available to the unprivileged probe child.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Subcommand)]
pub enum ProbeCommand {
    /// Checks whether an operator account has already been initialized.
    Bootstrap,
    /// Checks that deployed identity files satisfy runtime validation.
    Deployment,
}

impl ProbeCommand {
    /// Returns the fixed argument accepted by the hidden CLI.
    #[must_use]
    pub const fn argument(self) -> &'static str {
        match self {
            Self::Bootstrap => "bootstrap",
            Self::Deployment => "deployment",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use snafu::Snafu;

    #[derive(Debug, Snafu)]
    #[snafu(context(false), display("fixture CLI parsing failed"))]
    struct TestError {
        source: clap::Error,
    }

    #[test]
    fn rejects_unknown_and_extra_arguments() {
        for args in [
            vec!["setup", "unknown"],
            vec!["setup", "probe"],
            vec!["setup", "probe", "bootstrap", "extra"],
            vec!["setup", "probe", "deployment", "extra"],
        ] {
            assert!(Cli::try_parse_from(args).is_err());
        }
    }

    #[test]
    fn parses_probe_and_pki_directory() -> Result<(), TestError> {
        for probe in [ProbeCommand::Bootstrap, ProbeCommand::Deployment] {
            let cli = Cli::try_parse_from(["setup", "probe", probe.argument()])?;
            assert!(matches!(cli.command, Some(Command::Probe(command)) if command == probe));
        }
        let cli = Cli::try_parse_from(["setup", "--pki-dir", "/root/offline"])?;
        assert_eq!(cli.pki_dir, PathBuf::from("/root/offline"));
        assert_eq!(
            Cli::try_parse_from(["setup"])?.pki_dir,
            PathBuf::from(DEFAULT_PKI_DIRECTORY)
        );
        Ok(())
    }
}
