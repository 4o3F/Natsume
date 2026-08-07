use std::env;

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
}

fn endpoint_error(error: EndpointError) -> Error {
    Error::Endpoint {
        code: error.error_code(),
        error,
    }
}

fn run_args(args: &[String]) -> Result<(), Error> {
    match args {
        [] => {
            println!(concat!(
                "natsume-device-daemon blueprint: identity check -> vault -> ",
                "provisioning-window Enrollment -> install Gateway certificate -> Device Token-authenticated WSS control"
            ));
            Ok(())
        }
        [flag, ip, port] if flag == "--validate-endpoint" => {
            parse_endpoint(ip, port).map(|_| ()).map_err(endpoint_error)
        }
        [flag, ip, port] if flag == "--print-canonical-endpoint" => {
            let endpoint = parse_endpoint(ip, port).map_err(endpoint_error)?;
            println!("{} {}", endpoint.ip(), endpoint.port());
            Ok(())
        }
        _ => Err(Error::Arguments),
    }
}

fn run() -> Result<(), Error> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    run_args(&args)
}

#[snafu::report]
fn main() -> Result<(), Error> {
    run()
}

#[cfg(test)]
mod tests {
    use natsume_device_daemon::EndpointError;
    use natsume_error_code::{ErrorCode, common::CommonErrorCode};

    use super::*;

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
    fn cli_usage_error_is_local_and_has_no_stable_code() {
        let args = vec!["--unknown".to_owned()];
        let Err(error) = run_args(&args) else {
            panic!("unknown arguments must be rejected");
        };
        let display = format!("{error}");
        assert_eq!(
            display,
            "usage: natsume-device-daemon [--validate-endpoint|--print-canonical-endpoint] <ip> <port>"
        );
        assert!(!display.contains("INVALID_REQUEST"));
    }
}
