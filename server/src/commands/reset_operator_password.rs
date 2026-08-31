use crate::{
    component::operator::{OperatorComponent, OperatorCredentials},
    config::ServerConfig,
    db::{Database, DatabaseConfig},
};

use super::{CommandError, credentials};

pub(super) async fn execute(config: ServerConfig) -> Result<(), CommandError> {
    reset_operator_password_with(config, read_credentials_from_tty).await?;
    tracing::info!("operator password reset completed");
    Ok(())
}

async fn reset_operator_password_with<F>(
    config: ServerConfig,
    read_credentials: F,
) -> Result<(), CommandError>
where
    F: FnOnce() -> Result<OperatorCredentials, CommandError>,
{
    let database_config = DatabaseConfig::new(config.database_path(), false);
    let database = Database::connect_and_migrate(&database_config)
        .await
        .map_err(|_| CommandError::Database)?;
    tracing::info!("database ready");
    let credentials = read_credentials()?;
    let password_hash = credentials
        .hash_password()
        .map_err(|_| CommandError::PasswordReset)?;
    OperatorComponent::new(database)
        .reset_password(credentials.login_name(), &password_hash)
        .await
        .map_err(|_| CommandError::PasswordReset)
}

fn read_credentials_from_tty() -> Result<OperatorCredentials, CommandError> {
    credentials::read_from_tty(CommandError::PasswordReset)
}
