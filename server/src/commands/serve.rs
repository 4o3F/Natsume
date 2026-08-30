use std::{future::Future, path::Path, sync::Arc};

use tracing::instrument::WithSubscriber as _;

use crate::{
    config::{GatewaySiteConfig, ServerConfig},
    db::{Database, DatabaseConfig},
    http,
    server_state::ServerState,
    tls::TlsListener,
    vault::load as load_vault,
};

use super::CommandError;

const WEB_ASSETS_PATH: &str = "/usr/share/natsume-server/web";

pub(super) async fn execute(config: ServerConfig) -> Result<(), CommandError> {
    if !Path::new(WEB_ASSETS_PATH).join("index.html").is_file() {
        return Err(CommandError::WebAssets);
    }
    let shutdown = shutdown_signal()?;
    run_until(config, shutdown).await
}

/// Builds the mounted Server HTTP surface over an already-bootstrapped database.
///
/// # Errors
///
/// Returns a redacted [`CommandError`] when startup infrastructure cannot be loaded.
pub async fn router(config: ServerConfig, web_root: &Path) -> Result<axum::Router, CommandError> {
    let database_config = DatabaseConfig::new(config.database_path(), false);
    let database = Database::connect_and_migrate(&database_config)
        .await
        .map_err(|_| CommandError::Database)?;
    let vault = load_vault(config.vault_master_key_path()).map_err(|_| CommandError::Vault)?;
    let state = Arc::new(ServerState::new(database, Arc::new(vault)));
    Ok(http::router(state, web_root))
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
    GatewaySiteConfig::load_from(config.site_config_path())
        .map_err(|_| CommandError::SiteConfiguration)?;
    let database_config = DatabaseConfig::new(config.database_path(), false);
    let database = Database::connect_and_migrate(&database_config)
        .await
        .map_err(|_| CommandError::Database)?;
    tracing::info!("database ready");
    let vault = load_vault(config.vault_master_key_path()).map_err(|_| CommandError::Vault)?;
    tracing::info!("vault ready");
    let listener = TlsListener::bind(
        config.listen_address(),
        config.tls_certificate_path(),
        config.tls_private_key_path(),
    )
    .await
    .map_err(|_| CommandError::Tls)?;
    tracing::info!("TLS identity loaded");
    tracing::info!(listen_address = %config.listen_address(), "listener bound");
    let state = Arc::new(ServerState::new(database, Arc::new(vault)));
    let router = http::router(state, Path::new(WEB_ASSETS_PATH));

    let dispatcher = tracing::dispatcher::get_default(Clone::clone);
    let shutdown = async move {
        shutdown.await;
        tracing::info!("graceful shutdown initiated");
    }
    .with_subscriber(dispatcher);
    let result = http::serve_until(listener, router, shutdown)
        .await
        .map_err(|_| CommandError::Http);
    if result.is_ok() {
        tracing::info!("graceful shutdown completed");
    }
    result
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
