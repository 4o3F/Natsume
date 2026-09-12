use std::{future::Future, path::Path, sync::Arc};

use tracing::instrument::WithSubscriber as _;

use crate::{
    component::{operator::OperatorComponent, runtime::RuntimeConfigComponent},
    config::ServerConfig,
    db::{Database, DatabaseConfig},
    http,
    server_state::ServerState,
    tls::TlsListener,
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
    let state = Arc::new(load_state(&config).await?);
    Ok(http::router(state, web_root))
}

async fn load_state(config: &ServerConfig) -> Result<ServerState, CommandError> {
    let database_config = DatabaseConfig::new(config.database_path(), false);
    let database = Database::connect_and_migrate(&database_config)
        .await
        .map_err(|_| CommandError::Database)?;
    tracing::info!("database ready");
    let state = ServerState::load(database.clone(), config).map_err(CommandError::from)?;
    configure_runtime(&database, config.domjudge_origin()).await?;
    Ok(state)
}

async fn configure_runtime(database: &Database, origin: &str) -> Result<(), CommandError> {
    let origin = origin.to_owned();
    database
        .write(move |transaction| {
            if !OperatorComponent::is_initialized(transaction)
                .map_err(|_| CommandError::Database)?
            {
                return Err(CommandError::NotBootstrapped);
            }
            RuntimeConfigComponent::apply_deployment_config(transaction, &origin)
                .map_err(|_| CommandError::Configuration)
        })
        .await
        .map_err(|error| match error {
            crate::db::TransactionError::Operation(error) => error,
            crate::db::TransactionError::Persistence(_) => CommandError::Database,
        })
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
    let state = Arc::new(load_state(&config).await?);
    tracing::info!("server state ready");
    let listener = TlsListener::bind(
        config.listen_address(),
        config.tls_certificate_path(),
        config.tls_private_key_path(),
    )
    .await
    .map_err(|_| CommandError::Tls)?;
    tracing::info!("TLS identity loaded");
    tracing::info!(listen_address = %config.listen_address(), "listener bound");
    let router = http::router(state, Path::new(WEB_ASSETS_PATH));

    let dispatcher = tracing::dispatcher::get_default(Clone::clone);
    let shutdown = async move {
        shutdown.await;
        tracing::info!("graceful shutdown initiated");
    }
    .with_subscriber(dispatcher);
    let result = axum::serve(listener, router)
        .with_graceful_shutdown(shutdown)
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

#[cfg(test)]
mod tests {
    use crate::{commands::bootstrap::tests::Fixture, component::runtime::RuntimeConfigComponent};

    use super::{CommandError, configure_runtime};

    #[tokio::test]
    async fn startup_applies_deployment_origin_and_skips_unchanged_values() {
        let fixture = Fixture::new();
        fixture
            .bootstrap("admin")
            .await
            .unwrap_or_else(|error| panic!("bootstrap failed: {error}"));
        let database = fixture.database(false).await;
        configure_runtime(&database, "https://new.example.test")
            .await
            .unwrap_or_else(|error| panic!("configure runtime failed: {error}"));
        assert_eq!(
            RuntimeConfigComponent::new(database.clone())
                .materialize()
                .await,
            Ok("https://new.example.test".to_owned())
        );
        fixture.sql("CREATE TRIGGER fail_runtime_update BEFORE UPDATE ON runtime_config BEGIN SELECT RAISE(ABORT, 'unexpected write'); END;").await;
        assert_eq!(
            configure_runtime(&database, "https://new.example.test").await,
            Ok(())
        );
    }

    #[tokio::test]
    async fn startup_repairs_missing_runtime_in_an_initialized_database() {
        let fixture = Fixture::new();
        fixture
            .bootstrap("admin")
            .await
            .unwrap_or_else(|error| panic!("bootstrap failed: {error}"));
        fixture.sql("DELETE FROM runtime_config;").await;
        let database = fixture.database(false).await;
        configure_runtime(&database, "https://judge.example.test")
            .await
            .unwrap_or_else(|error| panic!("initialize runtime failed: {error}"));
        assert_eq!(
            RuntimeConfigComponent::new(database).materialize().await,
            Ok("https://judge.example.test".to_owned())
        );
    }

    #[tokio::test]
    async fn startup_rejects_database_without_completed_bootstrap() {
        let fixture = Fixture::new();
        let database = fixture.database(true).await;
        assert_eq!(
            configure_runtime(&database, "https://judge.example.test").await,
            Err(CommandError::NotBootstrapped)
        );
        assert_eq!(
            RuntimeConfigComponent::new(database).read_current().await,
            Ok(None)
        );
    }
}
