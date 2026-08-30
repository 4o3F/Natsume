use crate::{
    component::operator::{OperatorComponent, OperatorCredentials, hash_password},
    config::ServerConfig,
    db::{Database, DatabaseConfig},
    vault::ensure_master_key,
};

use super::{CommandError, credentials};

pub(super) async fn execute(config: ServerConfig) -> Result<(), CommandError> {
    bootstrap_with(config, read_credentials_from_tty).await?;
    tracing::info!("bootstrap completed");
    Ok(())
}

async fn bootstrap_with<F>(config: ServerConfig, read_credentials: F) -> Result<(), CommandError>
where
    F: FnOnce() -> Result<OperatorCredentials, CommandError>,
{
    let database_config = DatabaseConfig::new(config.database_path(), true);
    let database = Database::connect_and_migrate(&database_config)
        .await
        .map_err(|_| CommandError::Database)?;
    tracing::info!("database ready");
    ensure_master_key(config.vault_master_key_path()).map_err(|_| CommandError::Vault)?;
    tracing::info!("vault key verified");
    let credentials = read_credentials()?;
    let password_hash =
        hash_password(credentials.password()).map_err(|_| CommandError::Bootstrap)?;
    OperatorComponent::new(database)
        .create_first_admin(credentials.login_name(), &password_hash)
        .await
        .map_err(|_| CommandError::Bootstrap)?;
    Ok(())
}

fn read_credentials_from_tty() -> Result<OperatorCredentials, CommandError> {
    credentials::read_from_tty(CommandError::Bootstrap)
}
