use std::process::ExitCode;

use clap::Parser;
use natsume_server::{
    commands::{self, Command},
    config::{CONFIG_PATH, ServerConfig},
};

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let config = match ServerConfig::load() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("error: {CONFIG_PATH}: {error}");
            return ExitCode::FAILURE;
        }
    };
    match commands::run(config, cli.command).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
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
            Cli::try_parse_from(["natsume-server", "reset-operator-password", "extra"]).is_err()
        );
    }
}
