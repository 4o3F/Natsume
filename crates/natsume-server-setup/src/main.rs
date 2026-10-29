fn main() -> std::process::ExitCode {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_target(false)
        .init();
    run()
}

#[cfg(target_os = "linux")]
fn run() -> std::process::ExitCode {
    use clap::Parser as _;
    use natsume_server_setup::{
        cli::{Cli, Command},
        deployment::{
            file::{Ownership, ROOT_OWNER},
            plan::Owners,
        },
        host::SystemHost,
        interaction::terminal::TerminalInteraction,
        probe,
        wizard::{self, WizardContext, WizardOutcome},
    };
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Probe(command)) => std::process::ExitCode::from(probe::run(command).code()),
        None => {
            let mut host = match SystemHost::new() {
                Ok(host) => host,
                Err(error) => {
                    tracing::error!(error = %snafu::Report::from_error(error), "setup host initialization failed");
                    return std::process::ExitCode::FAILURE;
                }
            };
            let context = WizardContext {
                target_root: "/".into(),
                pki_dir: cli.pki_dir,
                owners: Owners {
                    root: ROOT_OWNER,
                    service: Ownership {
                        uid: host.account().uid,
                        gid: host.account().gid,
                    },
                },
            };
            match wizard::run(&mut TerminalInteraction, &mut host, &context) {
                Ok(WizardOutcome::Installed(_) | WizardOutcome::PendingSignature { .. }) => {
                    std::process::ExitCode::SUCCESS
                }
                Ok(WizardOutcome::Aborted) => std::process::ExitCode::from(2),
                Err(error) => {
                    tracing::error!(error = %snafu::Report::from_error(error), "setup failed");
                    std::process::ExitCode::FAILURE
                }
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn run() -> std::process::ExitCode {
    tracing::error!("the Natsume Server setup application requires Linux");
    std::process::ExitCode::FAILURE
}
