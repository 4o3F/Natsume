use clap::Parser;
use natsume_server::{
    commands::{self, Command, CommandError},
    config::ServerConfig,
};

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[tokio::main]
async fn main() -> Result<(), CommandError> {
    let cli = Cli::parse();
    let config = ServerConfig::load().map_err(|_| CommandError::Configuration)?;
    commands::run(config, cli.command).await
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
