#![forbid(unsafe_code)]

#[cfg(target_os = "linux")]
mod atomic_write;
#[cfg(target_os = "linux")]
mod control;
#[cfg(target_os = "linux")]
mod diagnostics;
#[cfg(target_os = "linux")]
mod identity_record;
#[cfg(target_os = "linux")]
mod reconcile;
#[cfg(target_os = "linux")]
mod startup;

#[cfg(target_os = "linux")]
use application::{CanonicalEndpoint, canonical_uuid, canonical_uuid_v7};

#[cfg(target_os = "linux")]
fn main() -> std::process::ExitCode {
    std::process::Termination::report(application::run_application())
}

#[cfg(not(target_os = "linux"))]
fn main() -> std::process::ExitCode {
    tracing_subscriber::fmt().with_target(false).init();
    tracing::error!("the workstation Device Daemon requires Linux");
    std::process::ExitCode::FAILURE
}

#[cfg(target_os = "linux")]
mod application {
    use super::startup;
    use std::{net::IpAddr, num::NonZeroU16};

    use clap::{CommandFactory as _, Parser, Subcommand};
    use serde::Deserialize;
    use snafu::Snafu;
    use uuid::{Uuid, Variant, Version};

    pub(super) fn canonical_uuid(value: &str) -> Option<Uuid> {
        Uuid::parse_str(value)
            .ok()
            .filter(|uuid| uuid.hyphenated().to_string() == value)
    }

    pub(super) fn canonical_uuid_v7(value: &str) -> Option<Uuid> {
        canonical_uuid(value).filter(|uuid| {
            uuid.get_version() == Some(Version::SortRand) && uuid.get_variant() == Variant::RFC4122
        })
    }

    #[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
    pub(super) struct CanonicalEndpoint {
        pub(super) ip: IpAddr,
        pub(super) port: NonZeroU16,
    }

    #[derive(Debug, Snafu)]
    pub(super) enum Error {
        #[snafu(display("{}", Args::command().render_usage()))]
        Arguments,

        #[snafu(display("structured logging could not be initialized"))]
        Logging,

        #[snafu(display("{source}"))]
        Startup { source: startup::StartupError },
    }

    #[derive(Subcommand)]
    enum Command {
        #[command(disable_help_flag = true, disable_version_flag = true)]
        Run,
    }

    #[derive(Parser)]
    #[command(
        name = "natsume-device-daemon",
        disable_help_flag = true,
        disable_version_flag = true
    )]
    struct Args {
        #[command(subcommand)]
        command: Command,
    }

    async fn execute(command: Command) -> Result<(), Error> {
        match command {
            Command::Run => {
                startup::run_production()
                    .await
                    .map_err(|source| Error::Startup { source })?;
                Ok(())
            }
        }
    }

    #[tokio::main]
    #[snafu::report]
    pub(super) async fn run_application() -> Result<(), Error> {
        tracing_subscriber::fmt()
            .with_ansi(false)
            .without_time()
            .with_target(false)
            .try_init()
            .map_err(|_| Error::Logging)?;
        let command = Args::try_parse().map_err(|_| Error::Arguments)?.command;
        execute(command).await
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn parse_test_args(args: &[&str]) -> Result<Command, Error> {
            Args::try_parse_from(
                std::iter::once("natsume-device-daemon").chain(args.iter().copied()),
            )
            .map(|args| args.command)
            .map_err(|_| Error::Arguments)
        }

        fn assert_usage_error(args: &[&str]) {
            let Err(error) = parse_test_args(args) else {
                panic!("expected usage error");
            };
            let display = format!("{error}");
            assert_eq!(display, Args::command().render_usage().to_string());
        }

        #[test]
        fn run_command_selects_daemon_startup_without_starting_it() {
            assert!(matches!(parse_test_args(&["run"]), Ok(Command::Run)));
        }

        #[test]
        fn missing_command_produces_usage() {
            assert_usage_error(&[]);
        }

        #[test]
        fn run_extra_args_produce_usage() {
            assert_usage_error(&["run", "extra"]);
        }

        #[test]
        fn unknown_command_is_a_local_usage_error_without_a_stable_code() {
            let Err(error) = parse_test_args(&["unknown"]) else {
                panic!("unknown command must be rejected");
            };
            let display = format!("{error}");
            assert_eq!(display, Args::command().render_usage().to_string());
            assert!(!display.contains("INVALID_REQUEST"));
        }

        #[test]
        fn help_flag_produces_usage() {
            assert_usage_error(&["--help"]);
        }

        #[test]
        fn version_flag_produces_usage() {
            assert_usage_error(&["--version"]);
        }
    }
}
