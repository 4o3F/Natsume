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
    config: PathBuf,
}

#[cfg(windows)]
#[derive(Debug, Snafu)]
enum StartupError {
    #[snafu(display("invalid client command line"))]
    Arguments { source: clap::Error },
    #[snafu(display("could not start the print client runtime"))]
    Runtime { source: std::io::Error },
    #[snafu(display("print client stopped"))]
    Client {
        source: natsume_print_client::client::ClientError,
    },
}

#[cfg(windows)]
fn execute() -> Result<(), StartupError> {
    let arguments = Arguments::try_parse().context(ArgumentsSnafu)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context(RuntimeSnafu)?;
    runtime
        .block_on(natsume_print_client::client::run(&arguments.config))
        .context(ClientSnafu)
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
