use std::{env, net::IpAddr, str::FromStr};

use natsume_error_code::{AsErrorCode, CodedReport, ErrorCode};
use snafu::Snafu;

#[derive(Debug, Snafu)]
enum EndpointError {
    #[snafu(display("invalid Natsume Server IP literal"))]
    Ip,

    #[snafu(display("invalid Natsume Server port"))]
    Port,
}

impl AsErrorCode for EndpointError {
    fn error_code(&self) -> ErrorCode {
        match self {
            Self::Ip => ErrorCode::InstallEndpointInvalidIp,
            Self::Port => ErrorCode::InstallEndpointInvalidPort,
        }
    }
}

#[derive(Debug, Snafu)]
enum Error {
    #[snafu(display("usage: natsume-device-daemon --validate-endpoint <ip> <port>"))]
    Arguments,

    #[snafu(display("{report}"))]
    Endpoint { report: CodedReport },
}

fn validate_endpoint(ip: &str, port: &str) -> Result<(), EndpointError> {
    IpAddr::from_str(ip).map_err(|_| EndpointError::Ip)?;
    let parsed = port.parse::<u16>().map_err(|_| EndpointError::Port)?;
    if parsed == 0 {
        return Err(EndpointError::Port);
    }
    Ok(())
}

fn run_args(args: &[String]) -> Result<(), Error> {
    match args {
        [] => {
            println!(concat!(
                "natsume-device-daemon blueprint: identity check -> vault -> ",
                "Device-only enrollment -> mTLS control -> SYNC_STATE Gateway PKI"
            ));
            Ok(())
        }
        [flag, ip, port] if flag == "--validate-endpoint" => {
            validate_endpoint(ip, port).map_err(|error| Error::Endpoint {
                report: CodedReport::from_error(&error),
            })
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
    use super::*;

    #[test]
    fn endpoint_errors_map_to_stable_install_codes() {
        assert_eq!(
            EndpointError::Ip.error_code(),
            ErrorCode::InstallEndpointInvalidIp
        );
        assert_eq!(
            EndpointError::Port.error_code(),
            ErrorCode::InstallEndpointInvalidPort
        );
    }

    #[test]
    fn report_contains_code_without_rejected_input() {
        let rejected_ip = "203.0.113.999";
        let Err(error) = validate_endpoint(rejected_ip, "8443") else {
            panic!("invalid IP must be rejected");
        };
        let report = CodedReport::from_error(&error);
        let display = format!("{report}");
        assert!(display.starts_with("INSTALL_ENDPOINT_INVALID_IP: "));
        assert!(!display.contains(rejected_ip));
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
            "usage: natsume-device-daemon --validate-endpoint <ip> <port>"
        );
        assert!(!display.contains("PACKAGE_LAYOUT_INVALID"));
    }
}
