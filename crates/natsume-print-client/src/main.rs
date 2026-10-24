use std::process::ExitCode;

#[cfg(windows)]
use clap::Parser as _;
#[cfg(windows)]
use snafu::{ResultExt as _, Snafu};
#[cfg(windows)]
use std::path::PathBuf;

#[cfg(windows)]
#[derive(clap::Parser)]
#[command(about = "Natsume Windows print client")]
struct Arguments {
    #[arg(long)]
    config: Option<PathBuf>,
}

#[cfg(windows)]
#[derive(Debug, Snafu)]
enum StartupError {
    #[snafu(display("invalid client command line"))]
    Arguments { source: clap::Error },
    #[snafu(display("print client stopped: {source}"))]
    Client {
        source: natsume_print_client::client::ClientError,
    },
}

#[cfg(windows)]
fn execute() -> Result<(), StartupError> {
    let arguments = Arguments::try_parse().context(ArgumentsSnafu)?;
    natsume_print_client::client::run_gui(arguments.config).context(ClientSnafu)
}

#[cfg(all(test, windows))]
mod tests {
    use snafu::IntoError as _;

    use super::*;

    #[test]
    fn entry_point_keeps_the_client_print_failure_context() {
        let source = natsume_print_client::client::ClientError::Print {
            source: natsume_print_client::PrintError::DriverCall {
                operation: "create printer context",
                code: 0,
            },
        };
        let error = ClientSnafu.into_error(source);

        assert_eq!(
            error.to_string(),
            "print client stopped: native print operation failed: printer driver operation failed: create printer context (code 0)"
        );
    }
}

fn main() -> ExitCode {
    tracing_subscriber::fmt().with_target(false).init();
    #[cfg(windows)]
    match execute() {
        Ok(()) => ExitCode::SUCCESS,
        Err(StartupError::Arguments { source })
            if matches!(
                source.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            ) =>
        {
            tracing::info!(help = ?source.to_string(), "print client usage");
            ExitCode::SUCCESS
        }
        Err(error) => {
            tracing::error!(%error, "print client startup or execution failed; no new work will be claimed");
            ExitCode::FAILURE
        }
    }
    #[cfg(not(windows))]
    {
        tracing::error!("the print client application requires Windows 10 or 11 x64");
        ExitCode::FAILURE
    }
}
