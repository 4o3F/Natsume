#![forbid(unsafe_code)]

mod atomic_write;
mod control;
mod identity_record;
mod reconcile;
mod startup;

use std::{net::IpAddr, num::NonZeroU16};

use clap::{CommandFactory as _, Parser, Subcommand};
use serde::Deserialize;
use snafu::Snafu;
use uuid::{Uuid, Variant, Version};

fn canonical_uuid(value: &str) -> Option<Uuid> {
    Uuid::parse_str(value)
        .ok()
        .filter(|uuid| uuid.hyphenated().to_string() == value)
}

fn canonical_uuid_v7(value: &str) -> Option<Uuid> {
    canonical_uuid(value).filter(|uuid| {
        uuid.get_version() == Some(Version::SortRand) && uuid.get_variant() == Variant::RFC4122
    })
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
struct CanonicalEndpoint {
    ip: IpAddr,
    port: NonZeroU16,
}

#[derive(Debug, Snafu)]
enum Error {
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
async fn main() -> Result<(), Error> {
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
        Args::try_parse_from(std::iter::once("natsume-device-daemon").chain(args.iter().copied()))
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
