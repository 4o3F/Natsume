use std::process::ExitCode;

#[cfg(windows)]
#[path = "../../ui/mod.rs"]
mod gui;

fn main() -> ExitCode {
    tracing_subscriber::fmt().with_target(false).init();
    #[cfg(windows)]
    {
        application::run()
    }
    #[cfg(not(windows))]
    {
        tracing::error!("Natsume Photo Client requires Windows");
        ExitCode::FAILURE
    }
}

#[cfg(windows)]
mod application {
    use clap::Parser as _;
    use natsume_photo_client::initialization::{Config, Initializer, VaultSecrets};
    use snafu::{ResultExt as _, Snafu};
    use std::{path::PathBuf, process::ExitCode};

    #[derive(clap::Parser)]
    #[command(
        version,
        about = "Natsume Photo Client offline photography workstation"
    )]
    struct Arguments {
        #[command(subcommand)]
        command: Option<Command>,
    }
    #[derive(clap::Subcommand)]
    enum Command {
        /// Authorize and download a new workspace, or resume its initialization.
        Initialize {
            path: PathBuf,
            #[arg(long)]
            server: String,
            #[arg(long)]
            ca_certificate: Option<PathBuf>,
        },
        /// Validate local workspace data and authorization without contacting Server.
        Doctor { path: PathBuf },
        /// Export current confirmed photos to one signed ZIP without contacting Server.
        Export { path: PathBuf, output: PathBuf },
    }
    #[derive(Debug, Snafu)]
    enum Error {
        #[snafu(display("Photo window could not run: {source}"))]
        Gui { source: super::gui::Error },
        #[snafu(display("offline Photo export failed: {source}"))]
        Export {
            source: natsume_photo_client::export::ExportError,
        },
        #[snafu(display("Photo command blocking task failed"))]
        Worker { source: tokio::task::JoinError },
        #[snafu(display("invalid command arguments; use --help"))]
        Arguments { source: clap::Error },
        #[snafu(display("cannot start the Photo runtime"))]
        Runtime { source: std::io::Error },
        #[snafu(display("cannot read the hidden pairing code"))]
        PairingInput { source: std::io::Error },
        #[snafu(display("Photo initialization failed: {source}"))]
        Initialization {
            source: natsume_photo_client::initialization::Error,
        },
        #[snafu(display("workspace diagnosis failed: {source}"))]
        Workspace {
            source: natsume_photo_client::workspace::Error,
        },
    }
    async fn execute(command: Command) -> Result<(), Error> {
        match command {
            Command::Export { path, output } => {
                let workspace = natsume_photo_client::workspace::Workspace::open(&path)
                    .await
                    .context(WorkspaceSnafu)?;
                natsume_photo_client::export::export_results(&workspace, &output, &VaultSecrets)
                    .await
                    .context(ExportSnafu)?;
            }
            Command::Initialize {
                path,
                server,
                ca_certificate,
            } => {
                let (initializer, needs_code) = tokio::task::spawn_blocking(move || {
                    let initializer = Initializer::open(
                        Config {
                            server_url: server,
                            workspace: path,
                            ca_certificate,
                        },
                        VaultSecrets,
                    )
                    .context(InitializationSnafu)?;
                    let needs_code = initializer
                        .needs_pairing_code()
                        .context(InitializationSnafu)?;
                    Ok::<_, Error>((initializer, needs_code))
                })
                .await
                .context(WorkerSnafu)??;
                let code = if needs_code {
                    tracing::info!(
                        "enter the administrator's one-time pairing code; input is hidden"
                    );
                    Some(zeroize::Zeroizing::new(
                        tokio::task::spawn_blocking(rpassword::read_password)
                            .await
                            .context(WorkerSnafu)?
                            .context(PairingInputSnafu)?,
                    ))
                } else {
                    None
                };
                let workspace = initializer
                    .run(code.as_deref().map(String::as_str))
                    .await
                    .context(InitializationSnafu)?;
                tracing::info!(
                    team_count = workspace
                        .roster()
                        .await
                        .context(WorkspaceSnafu)?
                        .teams()
                        .len(),
                    "Photo workspace initialized; ordinary operations are offline"
                );
            }
            Command::Doctor { path } => {
                let workspace =
                    natsume_photo_client::initialization::diagnose_path(&path, &VaultSecrets)
                        .await
                        .context(InitializationSnafu)?;
                let teams = workspace
                    .roster()
                    .await
                    .context(WorkspaceSnafu)?
                    .teams()
                    .len();
                tracing::info!(
                    team_count = teams,
                    "workspace, school logos, authorization and signing credentials verified; no network request was made"
                );
            }
        }
        Ok(())
    }
    pub(super) fn run() -> ExitCode {
        let result = (|| {
            let arguments = Arguments::try_parse().context(ArgumentsSnafu)?;
            if let Some(command) = arguments.command {
                tokio::runtime::Runtime::new()
                    .context(RuntimeSnafu)?
                    .block_on(execute(command))
            } else {
                super::gui::run().context(GuiSnafu)
            }
        })();
        match result {
            Ok(()) => ExitCode::SUCCESS,
            Err(Error::Arguments { source })
                if matches!(
                    source.kind(),
                    clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
                ) =>
            {
                tracing::info!(help=%source,"usage");
                ExitCode::SUCCESS
            }
            Err(error) => {
                tracing::error!(error=%error,"Photo Client operation failed");
                ExitCode::FAILURE
            }
        }
    }
}
