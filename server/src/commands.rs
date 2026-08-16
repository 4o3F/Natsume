use std::{
    fs::OpenOptions,
    future::Future,
    io::{BufRead, BufReader, Write},
    path::Path,
};
use tracing::instrument::WithSubscriber as _;

#[path = "serve.rs"]
pub(crate) mod serve;

pub use crate::error::CommandError;

use crate::{
    application::{
        enrollment::{GatewayIssuer, GatewayIssuerError},
        operator::{OperatorCredentials, hash_password},
        provisioning,
    },
    config::{GatewaySiteConfig, ServerConfig},
    db::{self, Database, DatabaseConfig},
    http, logging,
    tls::TlsListener,
    vault::{ensure_master_key, require_master_key},
};

const WEB_ASSETS_PATH: &str = "/usr/share/natsume-server/web";
const LOGIN_NAME_PROMPT: &[u8] = b"Login name: ";
const PASSWORD_PROMPT: &str = "Password: ";
const PASSWORD_CONFIRMATION_PROMPT: &str = "Confirm password: ";

/// Builds the mounted Server HTTP surface over an already-bootstrapped database.
///
/// # Errors
///
/// Returns a redacted [`CommandError`] when the database cannot be opened or migrated.
pub async fn router(config: ServerConfig, web_root: &Path) -> Result<axum::Router, CommandError> {
    let database_config = DatabaseConfig::new(config.database_path(), false);
    let database = Database::connect_and_migrate(&database_config)
        .await
        .map_err(|_| CommandError::Database)?;
    Ok(http::router(
        database,
        config.vault_master_key_path(),
        web_root,
    ))
}

/// Runs the Server until SIGINT or SIGTERM requests graceful shutdown.
///
/// # Errors
///
/// Returns a redacted [`CommandError`] when a startup stage fails.
pub async fn serve(config: ServerConfig) -> Result<(), CommandError> {
    logging::initialize(config.log_level()).map_err(|_| CommandError::Logging)?;
    log_mode("serve");
    if !Path::new(WEB_ASSETS_PATH).join("index.html").is_file() {
        return Err(CommandError::WebAssets);
    }
    let shutdown = shutdown_signal()?;
    run_until(config, shutdown).await
}

/// Runs the production Server stack until the supplied shutdown future resolves.
///
/// # Errors
///
/// Returns a redacted [`CommandError`] when a startup or serving stage fails.
pub async fn run_until<F>(config: ServerConfig, shutdown: F) -> Result<(), CommandError>
where
    F: Future<Output = ()> + Send + 'static,
{
    let site = GatewaySiteConfig::load_from(config.site_config_path())
        .map_err(|_| CommandError::SiteConfiguration)?;
    let database_config = DatabaseConfig::new(config.database_path(), false);
    let database = Database::connect_and_migrate(&database_config)
        .await
        .map_err(|_| CommandError::Database)?;
    provisioning::recover_on_startup(&database)
        .await
        .map_err(|error| match error {
            provisioning::ProvisioningError::RevisionOverflow => {
                tracing::error!("provisioning window revision overflow prevented startup");
                CommandError::ProvisioningRevisionOverflow
            }
            provisioning::ProvisioningError::PersistenceFailed => CommandError::Database,
        })?;
    tracing::info!("database ready");
    require_master_key(config.vault_master_key_path()).map_err(|_| CommandError::Vault)?;
    tracing::info!("vault key verified");
    let origin_ca_certificate_path = config
        .origin_ca_certificate_path()
        .map_err(|_| CommandError::Configuration)?;
    let origin_ca_private_key_path = config
        .origin_ca_private_key_path()
        .map_err(|_| CommandError::Configuration)?;
    let gateway_issuer = GatewayIssuer::load(
        &origin_ca_certificate_path,
        &origin_ca_private_key_path,
        config.local_origin_root_path(),
        site,
    )
    .map_err(|error| match error {
        GatewayIssuerError::TrustRootMismatch => CommandError::OriginCaTrustRootMismatch,
        _ => CommandError::OriginCa,
    })?;
    tracing::info!("Origin CA issuing material verified");
    let listener = TlsListener::bind(
        config.listen_address(),
        config.tls_certificate_path(),
        config.tls_private_key_path(),
    )
    .await
    .map_err(|_| CommandError::Tls)?;
    tracing::info!("TLS identity loaded");
    tracing::info!(listen_address = %config.listen_address(), "listener bound");
    let router = http::router_with_enrollment(
        database,
        config.vault_master_key_path(),
        Path::new(WEB_ASSETS_PATH),
        gateway_issuer,
    );

    let dispatcher = tracing::dispatcher::get_default(Clone::clone);
    let shutdown = async move {
        shutdown.await;
        tracing::info!("graceful shutdown initiated");
    }
    .with_subscriber(dispatcher);
    let result = serve::serve_until(listener, router, shutdown)
        .await
        .map_err(|_| CommandError::Http);
    if result.is_ok() {
        tracing::info!("graceful shutdown completed");
    }
    result
}

/// Creates the Server database, vault master key, and single first
/// administrator without constructing any HTTP or TLS serving state.
///
/// # Errors
///
/// Returns a redacted [`CommandError`] when bootstrap fails.
pub async fn bootstrap(config: ServerConfig) -> Result<(), CommandError> {
    logging::initialize(config.log_level()).map_err(|_| CommandError::Logging)?;
    log_mode("bootstrap");
    bootstrap_with(config, read_bootstrap_credentials_from_tty).await?;
    tracing::info!("bootstrap completed");
    Ok(())
}

/// Resets one existing operator password and invalidates all of that
/// operator's sessions without constructing any HTTP, TLS, or vault state.
///
/// # Errors
///
/// Returns a redacted [`CommandError`] when password reset fails.
pub async fn reset_operator_password(config: ServerConfig) -> Result<(), CommandError> {
    logging::initialize(config.log_level()).map_err(|_| CommandError::Logging)?;
    log_mode("reset-operator-password");
    reset_operator_password_with(config, read_reset_credentials_from_tty).await?;
    tracing::info!("operator password reset completed");
    Ok(())
}

fn log_mode(mode: &'static str) {
    tracing::info!(mode, "server mode running");
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
    db::operator::create_first_admin(&database, credentials.login_name(), &password_hash)
        .await
        .map_err(|_| CommandError::Bootstrap)?;
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
    let password_hash =
        hash_password(credentials.password()).map_err(|_| CommandError::PasswordReset)?;
    db::operator::reset_operator_password(&database, credentials.login_name(), &password_hash)
        .await
        .map_err(|_| CommandError::PasswordReset)
}

fn read_bootstrap_credentials_from_tty() -> Result<OperatorCredentials, CommandError> {
    read_credentials_from_tty(CommandError::Bootstrap)
}

fn read_reset_credentials_from_tty() -> Result<OperatorCredentials, CommandError> {
    read_credentials_from_tty(CommandError::PasswordReset)
}

fn read_credentials_from_tty(
    credential_error: CommandError,
) -> Result<OperatorCredentials, CommandError> {
    let mut terminal = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .map_err(|_| credential_error)?;
    terminal
        .write_all(LOGIN_NAME_PROMPT)
        .and_then(|()| terminal.flush())
        .map_err(|_| credential_error)?;
    let mut login_name = String::new();
    BufReader::new(terminal)
        .read_line(&mut login_name)
        .map_err(|_| credential_error)?;
    while login_name.ends_with(['\r', '\n']) {
        login_name.pop();
    }

    let password = rpassword::prompt_password(PASSWORD_PROMPT).map_err(|_| credential_error)?;
    let password_confirmation =
        rpassword::prompt_password(PASSWORD_CONFIRMATION_PROMPT).map_err(|_| credential_error)?;
    OperatorCredentials::new(login_name, password, password_confirmation)
        .map_err(|_| credential_error)
}

fn shutdown_signal() -> Result<impl Future<Output = ()>, CommandError> {
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .map_err(|_| CommandError::Signal)?;
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .map_err(|_| CommandError::Signal)?;
    Ok(async move {
        tokio::select! {
            Some(()) = interrupt.recv() => {}
            Some(()) = terminate.recv() => {}
            else => std::future::pending::<()>().await,
        }
    })
}

#[cfg(test)]
mod tests;
