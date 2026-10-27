#[cfg(target_os = "linux")]
fn main() -> std::process::ExitCode {
    application::run_application()
}

#[cfg(not(target_os = "linux"))]
fn main() -> std::process::ExitCode {
    tracing_subscriber::fmt().with_target(false).init();
    tracing::error!("the Natsume Server application requires Linux");
    std::process::ExitCode::FAILURE
}

#[cfg(target_os = "linux")]
mod application {
    use std::process::ExitCode;

    use clap::Parser;
    use natsume_server::{
        commands::{self, Command, Logging},
        config::{CONFIG_PATH, ServerConfig},
    };

    #[derive(Parser)]
    struct Cli {
        #[command(subcommand)]
        command: Command,
    }

    pub(super) fn run_application() -> ExitCode {
        let logging = match commands::bootstrap_logging() {
            Ok(logging) => logging,
            Err(error) => {
                tracing::error!(config_path = CONFIG_PATH, error = %snafu::Report::from_error(error), "server logging bootstrap failed");
                return ExitCode::FAILURE;
            }
        };
        run(logging)
    }

    #[tokio::main]
    async fn run(logging: Logging) -> ExitCode {
        let cli = Cli::parse();
        let config = match ServerConfig::load() {
            Ok(config) => config,
            Err(error) => {
                tracing::error!(config_path = CONFIG_PATH, error = %snafu::Report::from_error(error), "server configuration failed");
                return ExitCode::FAILURE;
            }
        };
        match commands::run(config, cli.command, logging).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                tracing::error!(config_path = CONFIG_PATH, error = %snafu::Report::from_error(error), "server command failed");
                ExitCode::FAILURE
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use clap::Parser;

        use super::Cli;

        #[test]
        fn missing_subcommand_is_rejected() {
            assert!(Cli::try_parse_from(["natsume-server"]).is_err());
        }

        #[test]
        fn unknown_subcommand_is_rejected() {
            assert!(Cli::try_parse_from(["natsume-server", "unknown"]).is_err());
            assert!(Cli::try_parse_from(["natsume-server", "reset_operator_password"]).is_err());
        }

        #[test]
        fn password_reset_subcommand_is_bare_and_kebab_case() {
            assert!(Cli::try_parse_from(["natsume-server", "reset-operator-password"]).is_ok());
        }

        #[test]
        fn extra_arguments_are_rejected() {
            assert!(Cli::try_parse_from(["natsume-server", "serve", "extra"]).is_err());
            assert!(Cli::try_parse_from(["natsume-server", "bootstrap", "extra"]).is_err());
            assert!(
                Cli::try_parse_from(["natsume-server", "reset-operator-password", "extra"])
                    .is_err()
            );
        }
    }
}
