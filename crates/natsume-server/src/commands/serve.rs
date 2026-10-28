use std::{
    future::{Future, IntoFuture},
    path::Path,
    sync::Arc,
    time::Duration,
};

use tracing::instrument::WithSubscriber as _;

use crate::{
    background::start_background_tasks,
    component::{operator::OperatorComponent, runtime::RuntimeConfigComponent},
    config::ServerConfig,
    db::{Database, DatabaseConfig},
    http,
    server_state::ServerState,
    tls::{PeerAddress, TlsListener},
};

/// Maximum time admitted mutations may finish before actor shutdown proceeds.
const PRODUCER_WAIT_TIMEOUT: Duration = Duration::from_secs(10);

use super::error::CommandFailure as CommandError;

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
pub(super) async fn router(
    config: ServerConfig,
    web_root: &Path,
) -> Result<axum::Router, CommandError> {
    let state = Arc::new(load_state(&config).await?);
    Ok(http::router(state, web_root))
}

async fn load_state(config: &ServerConfig) -> Result<ServerState, CommandError> {
    let database_config = DatabaseConfig::new(config.database_path(), false);
    let database = Database::connect_and_migrate(&database_config)
        .await
        .map_err(|source| CommandError::Database { source })?;
    tracing::info!("database ready");
    let state = ServerState::load(database.clone(), config)
        .await
        .map_err(CommandError::from)?;
    configure_runtime(&database, config.domjudge_origin()).await?;
    Ok(state)
}

async fn configure_runtime(database: &Database, origin: &str) -> Result<(), CommandError> {
    let origin = origin.to_owned();
    database
        .write(move |transaction| {
            if !OperatorComponent::is_initialized(transaction)
                .map_err(|source| CommandError::Operator { source })?
            {
                return Err(CommandError::NotBootstrapped);
            }
            RuntimeConfigComponent::apply_deployment_config(transaction, &origin)
                .map_err(|source| CommandError::Runtime { source })
        })
        .await
        .map_err(|error| match error {
            crate::db::TransactionError::Operation(error) => error,
            crate::db::TransactionError::Persistence(source) => {
                CommandError::Persistence { source }
            }
        })
}

/// Runs the production Server stack until the supplied shutdown future resolves.
///
/// # Errors
///
/// Returns a redacted [`CommandError`] when a startup or serving stage fails.
pub(super) async fn run_until<F>(config: ServerConfig, shutdown: F) -> Result<(), CommandError>
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
    .map_err(|source| CommandError::Tls { source })?;
    tracing::info!("TLS identity loaded");
    tracing::info!(listen_address = %config.listen_address(), "listener bound");
    let background = start_background_tasks(&state)
        .await
        .map_err(|source| CommandError::Background { source })?;
    let router = http::router(Arc::clone(&state), Path::new(WEB_ASSETS_PATH));

    let http_stop = tokio_util::sync::CancellationToken::new();
    let stopped = http_stop.clone();
    let mut serving = tokio::spawn(
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<PeerAddress>(),
        )
        .with_graceful_shutdown(stopped.cancelled_owned())
        .into_future()
        .with_subscriber(tracing::dispatcher::get_default(Clone::clone)),
    );
    tokio::pin!(shutdown);
    let completed = tokio::select! {
        () = &mut shutdown => None,
        result = &mut serving => Some(result),
    };
    state.lifecycle().begin_shutdown();
    http_stop.cancel();
    tracing::info!("graceful shutdown initiated");
    if tokio::time::timeout(PRODUCER_WAIT_TIMEOUT, state.lifecycle().wait())
        .await
        .is_err()
    {
        state.journal().record_producer_timeout();
        tracing::warn!("in-flight mutation shutdown timed out");
    }
    state.device_control().shutdown().await;
    let background_result = background
        .shutdown()
        .await
        .map_err(|source| CommandError::Background { source });
    state.journal().shutdown().await;
    let serving_result = match completed {
        Some(result) => result,
        None => serving.await,
    };
    let result = match serving_result {
        Ok(result) => result.map_err(|source| CommandError::Http { source }),
        Err(source) => Err(CommandError::ServingTask {
            source: source.into(),
        }),
    }
    .and(background_result);
    if result.is_ok() {
        tracing::info!("graceful shutdown completed");
    }
    result
}

fn shutdown_signal() -> Result<impl Future<Output = ()>, CommandError> {
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .map_err(|source| CommandError::Signal { source })?;
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .map_err(|source| CommandError::Signal { source })?;
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
        assert!(matches!(RuntimeConfigComponent::new(database.clone())
                .materialize()
                .await, Ok(actual) if actual == "https://new.example.test"));
        fixture.sql("CREATE TRIGGER fail_runtime_update BEFORE UPDATE ON runtime_config BEGIN SELECT RAISE(ABORT, 'unexpected write'); END;").await;
        assert!(matches!(
            configure_runtime(&database, "https://new.example.test").await,
            Ok(())
        ));
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
        assert!(
            matches!(RuntimeConfigComponent::new(database).materialize().await, Ok(actual) if actual == "https://judge.example.test")
        );
    }

    #[tokio::test]
    async fn startup_rejects_database_without_completed_bootstrap() {
        let fixture = Fixture::new();
        let database = fixture.database(true).await;
        assert!(matches!(
            configure_runtime(&database, "https://judge.example.test").await,
            Err(CommandError::NotBootstrapped)
        ));
        assert!(
            matches!(RuntimeConfigComponent::new(database).read_current().await, Ok(actual) if actual.is_none())
        );
    }
}
