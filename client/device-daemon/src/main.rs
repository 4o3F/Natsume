use std::{env, net::IpAddr, process::ExitCode, str::FromStr};

use snafu::Snafu;

#[derive(Debug, Snafu)]
enum Error {
    #[snafu(display("usage: natsume-device-daemon --validate-endpoint <ip> <port>"))]
    Arguments,

    #[snafu(display("invalid Natsume Server IP literal: {value}"))]
    Ip { value: String },

    #[snafu(display("invalid Natsume Server port: {value}"))]
    Port { value: String },
}

fn validate_endpoint(ip: &str, port: &str) -> Result<(), Error> {
    IpAddr::from_str(ip).map_err(|_| Error::Ip {
        value: ip.to_owned(),
    })?;
    let port = port.parse::<u16>().map_err(|_| Error::Port {
        value: port.to_owned(),
    })?;
    if port == 0 {
        return Err(Error::Port {
            value: "0".to_owned(),
        });
    }
    Ok(())
}

fn run() -> Result<(), Error> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    match args.as_slice() {
        [] => {
            println!(concat!(
                "natsume-device-daemon blueprint: identity check -> vault -> ",
                "Device-only enrollment -> mTLS control -> SYNC_STATE Gateway PKI"
            ));
            Ok(())
        }
        [flag, ip, port] if flag == "--validate-endpoint" => validate_endpoint(ip, port),
        _ => Err(Error::Arguments),
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
