#![forbid(unsafe_code)]

mod atomic_write;
mod control;
mod identity_record;
mod reconcile;
mod startup;

use std::{
    io::{self, Write as _},
    net::IpAddr,
    num::NonZeroU16,
    str::FromStr,
};

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
enum EndpointError {
    #[snafu(display("invalid Natsume Server IP literal"))]
    Ip,

    #[snafu(display("invalid Natsume Server port"))]
    Port,
}

/// Parses an IP literal and non-zero TCP/UDP port into canonical typed values.
///
/// # Errors
///
/// Returns [`EndpointError::Ip`] for a hostname, bracketed address or malformed IP literal,
/// and [`EndpointError::Port`] for zero or a value outside the `u16` range.
fn parse_endpoint(ip: &str, port: &str) -> Result<CanonicalEndpoint, EndpointError> {
    let ip = IpAddr::from_str(ip).map_err(|_| EndpointError::Ip)?;
    let port = port.parse::<u16>().map_err(|_| EndpointError::Port)?;
    let Some(port) = NonZeroU16::new(port) else {
        return Err(EndpointError::Port);
    };
    Ok(CanonicalEndpoint { ip, port })
}

#[derive(Debug, Snafu)]
enum Error {
    #[snafu(display("{}", Args::command().render_usage()))]
    Arguments,

    #[snafu(display("INVALID_REQUEST: {error}"))]
    Endpoint { error: EndpointError },

    #[snafu(display("the canonical endpoint could not be written"))]
    Output,

    #[snafu(display("structured logging could not be initialized"))]
    Logging,

    #[snafu(display("{source}"))]
    Startup { source: startup::StartupError },
}

#[derive(Subcommand)]
enum Command {
    #[command(disable_help_flag = true, disable_version_flag = true)]
    Run,
    #[command(disable_help_flag = true, disable_version_flag = true)]
    CanonicalizeEndpoint { ip: String, port: String },
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
        Command::CanonicalizeEndpoint { ip, port } => {
            let endpoint = parse_endpoint(&ip, &port).map_err(|error| Error::Endpoint { error })?;
            writeln!(io::stdout().lock(), "{} {}", endpoint.ip, endpoint.port)
                .map_err(|_| Error::Output)
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

    #[tokio::test]
    async fn report_contains_only_reviewed_text_and_code() {
        let rejected_ip = "/sensitive/path secret-value source-chain-canary";
        let Err(error) = execute(Command::CanonicalizeEndpoint {
            ip: rejected_ip.to_owned(),
            port: "8443".to_owned(),
        })
        .await
        else {
            panic!("invalid IP must be rejected");
        };
        let display = format!("{error}");
        assert_eq!(
            display,
            "INVALID_REQUEST: invalid Natsume Server IP literal"
        );
        assert!(!display.contains(rejected_ip));
        assert!(!display.contains("/sensitive/path"));
        assert!(!display.contains("secret-value"));
        assert!(!display.contains("source-chain-canary"));
    }

    #[test]
    fn run_command_selects_daemon_startup_without_starting_it() {
        assert!(matches!(parse_test_args(&["run"]), Ok(Command::Run)));
    }

    #[test]
    fn canonicalize_endpoint_command_preserves_valid_arguments() {
        let Ok(Command::CanonicalizeEndpoint { ip, port }) =
            parse_test_args(&["canonicalize-endpoint", "192.0.2.10", "8443"])
        else {
            panic!("canonicalize-endpoint command must parse");
        };
        assert_eq!(ip, "192.0.2.10");
        assert_eq!(port, "8443");
    }

    #[test]
    fn missing_command_produces_usage() {
        assert_usage_error(&[]);
    }

    #[test]
    fn canonicalize_endpoint_missing_port_produces_usage() {
        assert_usage_error(&["canonicalize-endpoint", "192.0.2.10"]);
    }

    #[test]
    fn canonicalize_endpoint_extra_args_produce_usage() {
        assert_usage_error(&["canonicalize-endpoint", "192.0.2.10", "8443", "extra"]);
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

    #[test]
    fn canonicalizes_ipv4_and_ipv6_literals() {
        let Ok(ipv4) = parse_endpoint("192.0.2.10", "8443") else {
            panic!("valid IPv4 endpoint must parse");
        };
        assert_eq!(ipv4.ip.to_string(), "192.0.2.10");
        assert_eq!(ipv4.port.get(), 8443);

        let Ok(ipv6) = parse_endpoint("2001:0db8:0:0:0:0:0:1", "443") else {
            panic!("valid IPv6 endpoint must parse");
        };
        assert_eq!(ipv6.ip.to_string(), "2001:db8::1");
        assert_eq!(ipv6.port.get(), 443);
    }

    #[test]
    fn rejects_hostnames_brackets_and_zero_port() {
        assert!(parse_endpoint("server.example", "8443").is_err());
        assert!(parse_endpoint("[2001:db8::1]", "8443").is_err());
        assert!(parse_endpoint("192.0.2.10", "0").is_err());
        assert!(parse_endpoint("192.0.2.10", "65536").is_err());
    }
}
