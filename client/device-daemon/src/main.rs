use std::{
    env,
    io::{self, Write as _},
};

use clap::{ArgGroup, Parser};
use natsume_device_daemon::{EndpointError, parse_endpoint};
use natsume_error_code::ErrorCode;
use snafu::Snafu;

#[derive(Debug, Snafu)]
enum Error {
    #[snafu(display(
        "usage: natsume-device-daemon [--validate-endpoint|--print-canonical-endpoint] <ip> <port>"
    ))]
    Arguments,

    #[snafu(display("{}: {error}", code.as_str()))]
    Endpoint {
        code: ErrorCode,
        error: EndpointError,
    },

    #[snafu(display("the canonical endpoint could not be written"))]
    Output,

    #[snafu(display("structured logging could not be initialized"))]
    Logging,
}

#[derive(Parser)]
#[command(
    name = "natsume-device-daemon",
    disable_help_flag = true,
    disable_version_flag = true,
    group = ArgGroup::new("mode").multiple(false),
)]
struct Args {
    #[arg(
        long = "validate-endpoint",
        num_args = 2,
        value_names = ["ip", "port"],
        group = "mode"
    )]
    validate_endpoint: Option<Vec<String>>,

    #[arg(
        long = "print-canonical-endpoint",
        num_args = 2,
        value_names = ["ip", "port"],
        group = "mode"
    )]
    print_canonical_endpoint: Option<Vec<String>>,
}

fn endpoint_error(error: EndpointError) -> Error {
    Error::Endpoint {
        code: error.error_code(),
        error,
    }
}

fn run_args(args: &[String]) -> Result<(), Error> {
    let full_args: Vec<String> = std::iter::once("natsume-device-daemon".to_owned())
        .chain(args.iter().cloned())
        .collect();
    let cli = Args::try_parse_from(&full_args).map_err(|_| Error::Arguments)?;

    match (cli.validate_endpoint, cli.print_canonical_endpoint) {
        (None, None) => {
            tracing::info!(concat!(
                "natsume-device-daemon blueprint: identity check -> vault -> ",
                "provisioning-window Enrollment -> install Gateway certificate -> ",
                "Device Token-authenticated WSS control"
            ));
            Ok(())
        }
        (Some(values), None) => match values.as_slice() {
            [ip, port] => parse_endpoint(ip, port).map(|_| ()).map_err(endpoint_error),
            _ => Err(Error::Arguments),
        },
        (None, Some(values)) => match values.as_slice() {
            [ip, port] => {
                let endpoint = parse_endpoint(ip, port).map_err(endpoint_error)?;
                writeln!(io::stdout().lock(), "{} {}", endpoint.ip(), endpoint.port())
                    .map_err(|_| Error::Output)
            }
            _ => Err(Error::Arguments),
        },
        _ => Err(Error::Arguments),
    }
}

fn run() -> Result<(), Error> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    run_args(&args)
}

fn initialize_logging() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .with_ansi(false)
        .without_time()
        .with_target(false)
        .try_init()
        .map_err(|_| Error::Logging)
}

#[snafu::report]
fn main() -> Result<(), Error> {
    initialize_logging()?;
    run()
}

#[cfg(test)]
mod tests {
    use natsume_device_daemon::EndpointError;
    use natsume_error_code::{ErrorCode, common::CommonErrorCode};

    use super::*;

    const USAGE: &str =
        "usage: natsume-device-daemon [--validate-endpoint|--print-canonical-endpoint] <ip> <port>";

    fn assert_usage_error(args: &[String]) {
        let Err(error) = run_args(args) else {
            panic!("expected usage error");
        };
        let display = format!("{error}");
        assert_eq!(display, USAGE);
    }

    #[test]
    fn endpoint_errors_map_to_stable_invalid_request() {
        assert_eq!(
            EndpointError::Ip.error_code(),
            ErrorCode::Common(CommonErrorCode::InvalidRequest)
        );
        assert_eq!(
            EndpointError::Port.error_code(),
            ErrorCode::Common(CommonErrorCode::InvalidRequest)
        );
    }

    #[test]
    fn report_contains_only_reviewed_text_and_code() {
        let rejected_ip = "/sensitive/path secret-value source-chain-canary";
        let Err(error) = parse_endpoint(rejected_ip, "8443") else {
            panic!("invalid IP must be rejected");
        };
        let report = endpoint_error(error);
        let display = format!("{report}");
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
    fn no_args_is_blueprint() {
        let result = run_args(&[]);
        assert!(result.is_ok(), "no-args blueprint must succeed");
    }

    #[test]
    fn validate_endpoint_mode_with_valid_args_succeeds() {
        let args = vec![
            "--validate-endpoint".to_owned(),
            "192.0.2.10".to_owned(),
            "8443".to_owned(),
        ];
        assert!(
            run_args(&args).is_ok(),
            "valid --validate-endpoint must succeed"
        );
    }

    #[test]
    fn print_canonical_endpoint_mode_with_valid_args_succeeds() {
        let args = vec![
            "--print-canonical-endpoint".to_owned(),
            "192.0.2.10".to_owned(),
            "8443".to_owned(),
        ];
        assert!(
            run_args(&args).is_ok(),
            "valid --print-canonical-endpoint must succeed"
        );
    }

    #[test]
    fn cli_usage_error_is_local_and_has_no_stable_code() {
        let args = vec!["--unknown".to_owned()];
        let Err(error) = run_args(&args) else {
            panic!("unknown arguments must be rejected");
        };
        let display = format!("{error}");
        assert_eq!(display, USAGE);
        assert!(!display.contains("INVALID_REQUEST"));
    }

    #[test]
    fn help_flag_produces_usage() {
        assert_usage_error(&["--help".to_owned()]);
    }

    #[test]
    fn version_flag_produces_usage() {
        assert_usage_error(&["--version".to_owned()]);
    }

    #[test]
    fn validate_endpoint_missing_port_produces_usage() {
        assert_usage_error(&["--validate-endpoint".to_owned(), "192.0.2.10".to_owned()]);
    }

    #[test]
    fn validate_endpoint_extra_args_produce_usage() {
        assert_usage_error(&[
            "--validate-endpoint".to_owned(),
            "192.0.2.10".to_owned(),
            "8443".to_owned(),
            "extra".to_owned(),
        ]);
    }

    #[test]
    fn conflicting_flags_produce_usage() {
        let args = vec![
            "--validate-endpoint".to_owned(),
            "192.0.2.10".to_owned(),
            "8443".to_owned(),
            "--print-canonical-endpoint".to_owned(),
            "2001:db8::1".to_owned(),
            "443".to_owned(),
        ];
        assert_usage_error(&args);
    }
}
