use std::{
    fs::OpenOptions,
    future::Future,
    io::{BufRead, BufReader, Write},
};
use tracing::instrument::WithSubscriber as _;

use crate::{
    application::{
        operator::{FirstAdminCredentials, hash_password},
        provisioning,
    },
    config::ServerConfig,
    db::{self, Database, DatabaseConfig},
    error::AppError,
    http, logging,
    tls::TlsListener,
    vault::{ensure_master_key, require_master_key},
};

const LOGIN_NAME_PROMPT: &[u8] = b"Login name: ";
const PASSWORD_PROMPT: &str = "Password: ";
const PASSWORD_CONFIRMATION_PROMPT: &str = "Confirm password: ";

/// Runs the Server until SIGINT or SIGTERM requests graceful shutdown.
///
/// # Errors
///
/// Returns a redacted [`AppError`] when a startup stage fails.
pub async fn serve(config: ServerConfig) -> Result<(), AppError> {
    logging::initialize(config.log_level()).map_err(|_| AppError::Logging)?;
    log_mode("serve");
    let shutdown = shutdown_signal()?;
    run_until(config, shutdown).await
}

pub(crate) async fn run_until<F>(config: ServerConfig, shutdown: F) -> Result<(), AppError>
where
    F: Future<Output = ()> + Send + 'static,
{
    let database_config = DatabaseConfig::new(config.database_path(), false);
    let database = Database::connect_and_migrate(&database_config)
        .await
        .map_err(|_| AppError::Database)?;
    provisioning::recover_on_startup(&database)
        .await
        .map_err(|_| AppError::Database)?;
    tracing::info!("database ready");
    require_master_key(config.vault_master_key_path()).map_err(|_| AppError::Vault)?;
    tracing::info!("vault key verified");
    let listener = TlsListener::bind(
        config.listen_address(),
        config.tls_certificate_path(),
        config.tls_private_key_path(),
    )
    .await
    .map_err(|_| AppError::Tls)?;
    tracing::info!("TLS identity loaded");
    tracing::info!(listen_address = %config.listen_address(), "listener bound");
    let router = http::router(database);

    let dispatcher = tracing::dispatcher::get_default(Clone::clone);
    let shutdown = async move {
        shutdown.await;
        tracing::info!("graceful shutdown initiated");
    }
    .with_subscriber(dispatcher);
    let result = axum::serve(listener, router)
        .with_graceful_shutdown(shutdown)
        .await
        .map_err(|_| AppError::Http);
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
/// Returns a redacted [`AppError`] when bootstrap fails.
pub async fn bootstrap(config: ServerConfig) -> Result<(), AppError> {
    logging::initialize(config.log_level()).map_err(|_| AppError::Logging)?;
    log_mode("bootstrap");
    bootstrap_with(config, read_bootstrap_credentials_from_tty).await?;
    tracing::info!("bootstrap completed");
    Ok(())
}

fn log_mode(mode: &'static str) {
    tracing::info!(mode, "server mode running");
}

async fn bootstrap_with<F>(config: ServerConfig, read_credentials: F) -> Result<(), AppError>
where
    F: FnOnce() -> Result<FirstAdminCredentials, AppError>,
{
    let database_config = DatabaseConfig::new(config.database_path(), true);
    let database = Database::connect_and_migrate(&database_config)
        .await
        .map_err(|_| AppError::Database)?;
    tracing::info!("database ready");
    ensure_master_key(config.vault_master_key_path()).map_err(|_| AppError::Vault)?;
    tracing::info!("vault key verified");
    let credentials = read_credentials()?;
    let password_hash = hash_password(credentials.password()).map_err(|_| AppError::Bootstrap)?;
    db::operator::create_first_admin(&database, credentials.login_name(), &password_hash)
        .await
        .map_err(|_| AppError::Bootstrap)?;
    Ok(())
}

fn read_bootstrap_credentials_from_tty() -> Result<FirstAdminCredentials, AppError> {
    let mut terminal = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .map_err(|_| AppError::Bootstrap)?;
    terminal
        .write_all(LOGIN_NAME_PROMPT)
        .and_then(|()| terminal.flush())
        .map_err(|_| AppError::Bootstrap)?;
    let mut login_name = String::new();
    BufReader::new(terminal)
        .read_line(&mut login_name)
        .map_err(|_| AppError::Bootstrap)?;
    while login_name.ends_with(['\r', '\n']) {
        login_name.pop();
    }

    let password = rpassword::prompt_password(PASSWORD_PROMPT).map_err(|_| AppError::Bootstrap)?;
    let password_confirmation = rpassword::prompt_password(PASSWORD_CONFIRMATION_PROMPT)
        .map_err(|_| AppError::Bootstrap)?;
    FirstAdminCredentials::new(login_name, password, password_confirmation)
        .map_err(|_| AppError::Bootstrap)
}

fn shutdown_signal() -> Result<impl Future<Output = ()>, AppError> {
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .map_err(|_| AppError::Signal)?;
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .map_err(|_| AppError::Signal)?;
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
    use std::{
        fs,
        future::ready,
        net::{Ipv4Addr, SocketAddr, TcpListener as StdTcpListener},
        os::unix::fs::PermissionsExt,
        path::{Path, PathBuf},
    };

    use argon2::password_hash::PasswordHash;
    use diesel::{
        Connection, QueryableByName, RunQueryDsl,
        sql_types::{BigInt, Text},
        sqlite::SqliteConnection,
    };
    use snafu::Snafu;
    use tracing::instrument::WithSubscriber as _;
    use zeroize::Zeroizing;

    use crate::{
        application::operator::FirstAdminCredentials,
        config::{LogLevel, ServerConfig},
        db::{
            Database, DatabaseConfig, operator as db_operator,
            tests::{test_data_version, test_observer},
        },
        error::AppError,
        logging::tests::{CapturedLogs, SubscriberTestGuard},
        tls::tests::TestIdentity,
        vault::ensure_master_key,
    };

    use super::{bootstrap_with, log_mode, run_until};

    const LOCALHOST: Ipv4Addr = Ipv4Addr::LOCALHOST;

    #[tokio::test]
    async fn bootstrap_creates_artifacts_and_repeat_is_zero_write() -> Result<(), TestFailure> {
        let identity =
            TestIdentity::new(LOCALHOST).map_err(|_| TestFailure::FixtureCreationFailed)?;
        let key_directory = identity.directory_path().join("keys");
        create_private_directory(&key_directory)?;
        let database_path = identity.directory_path().join("server.db");
        let master_key_path = key_directory.join("server-root.key");
        let occupied_listener = StdTcpListener::bind(SocketAddr::from((LOCALHOST, 0)))
            .map_err(|_| TestFailure::FixtureCreationFailed)?;
        let occupied_address = occupied_listener
            .local_addr()
            .map_err(|_| TestFailure::FixtureCreationFailed)?;
        let config_path = write_config(
            &identity,
            occupied_address,
            &database_path,
            &master_key_path,
            &identity.directory_path().join("missing-certificate.der"),
            &identity.directory_path().join("missing-private-key.pk8"),
        )?;

        let first_config = ServerConfig::load_from(&config_path)
            .map_err(|_| TestFailure::UnexpectedStartupFailure)?;
        bootstrap_with(first_config, || {
            credentials("first-admin", "bootstrap-password")
        })
        .await
        .map_err(|_| TestFailure::UnexpectedStartupFailure)?;
        if !database_path.is_file() || !master_key_path.is_file() {
            return Err(TestFailure::StartupArtifactMissing);
        }
        let database = Database::connect_and_migrate(&DatabaseConfig::new(&database_path, false))
            .await
            .map_err(|_| TestFailure::FixtureIoFailed)?;
        let counts_before = business_counts(&database).await?;
        if counts_before != (1, 1) {
            return Err(TestFailure::UnexpectedBusinessRows);
        }
        drop(database);
        let content_before =
            Zeroizing::new(fs::read(&master_key_path).map_err(|_| TestFailure::FixtureIoFailed)?);
        let modified_before = key_modified_at(&master_key_path)?;

        let second_config = ServerConfig::load_from(&config_path)
            .map_err(|_| TestFailure::UnexpectedStartupFailure)?;
        assert_startup_error(
            bootstrap_with(second_config, || {
                credentials("second-admin-canary", "second-password-canary")
            })
            .await,
            AppError::Bootstrap,
        )?;
        let content_after =
            Zeroizing::new(fs::read(&master_key_path).map_err(|_| TestFailure::FixtureIoFailed)?);
        let modified_after = key_modified_at(&master_key_path)?;
        if content_before.as_slice() != content_after.as_slice()
            || modified_before != modified_after
        {
            return Err(TestFailure::MasterKeyWasRewritten);
        }
        let database = Database::connect_and_migrate(&DatabaseConfig::new(&database_path, false))
            .await
            .map_err(|_| TestFailure::FixtureIoFailed)?;
        if business_counts(&database).await? != counts_before {
            return Err(TestFailure::RepeatedBootstrapWroteBusinessRows);
        }
        drop(occupied_listener);
        Ok(())
    }

    #[tokio::test]
    async fn repeated_bootstrap_does_not_recover_an_open_provisioning_window()
    -> Result<(), TestFailure> {
        let identity =
            TestIdentity::new(LOCALHOST).map_err(|_| TestFailure::FixtureCreationFailed)?;
        let key_directory = identity.directory_path().join("keys");
        create_private_directory(&key_directory)?;
        let database_path = identity.directory_path().join("server.db");
        let master_key_path = key_directory.join("server-root.key");
        let config_path = write_config(
            &identity,
            SocketAddr::from((LOCALHOST, 0)),
            &database_path,
            &master_key_path,
            &identity.directory_path().join("missing-certificate.der"),
            &identity.directory_path().join("missing-private-key.pk8"),
        )?;

        let first_config = ServerConfig::load_from(&config_path)
            .map_err(|_| TestFailure::FixtureCreationFailed)?;
        bootstrap_with(first_config, || {
            credentials("first-admin", "first-password")
        })
        .await
        .map_err(|_| TestFailure::UnexpectedStartupFailure)?;

        let database = Database::connect_and_migrate(&DatabaseConfig::new(&database_path, false))
            .await
            .map_err(|_| TestFailure::FixtureIoFailed)?;
        let opening_audit_id = uuid::Uuid::now_v7().to_string();
        let correlation_id = uuid::Uuid::now_v7().to_string();
        let opening_audit_id_for_seed = opening_audit_id.clone();
        database
            .interact(move |connection| {
                diesel::sql_query(
                    "INSERT INTO audit_events (audit_event_id, occurred_at, actor, action_kind, \
                     resource_type, resource_id, result, reason_code, correlation_id, \
                     group_correlation_id, redacted_detail_json) VALUES (?, \
                     strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), 'operator:test', \
                     'open_provisioning_window', 'provisioning_window', NULL, 'succeeded', \
                     NULL, ?, NULL, '{}')",
                )
                .bind::<Text, _>(&opening_audit_id_for_seed)
                .bind::<Text, _>(&correlation_id)
                .execute(connection)?;
                diesel::sql_query(
                    "UPDATE provisioning_window SET state = 'open', revision = 1, \
                     last_audit_event_id = ? WHERE singleton = 1",
                )
                .bind::<Text, _>(&opening_audit_id_for_seed)
                .execute(connection)
            })
            .await
            .map_err(|_| TestFailure::FixtureIoFailed)?
            .map_err(|_| TestFailure::FixtureIoFailed)?;

        let mut observer =
            test_observer(&database_path).map_err(|_| TestFailure::FixtureIoFailed)?;
        let counts_before = bootstrap_business_counts(&mut observer)?;
        let version_before =
            test_data_version(&mut observer).map_err(|_| TestFailure::FixtureIoFailed)?;

        let second_config = ServerConfig::load_from(&config_path)
            .map_err(|_| TestFailure::FixtureCreationFailed)?;
        assert_startup_error(
            bootstrap_with(second_config, || {
                credentials("second-admin", "second-password")
            })
            .await,
            AppError::Bootstrap,
        )?;

        let counts_after = bootstrap_business_counts(&mut observer)?;
        let version_after =
            test_data_version(&mut observer).map_err(|_| TestFailure::FixtureIoFailed)?;
        let window = diesel::sql_query(
            "SELECT state, revision FROM provisioning_window WHERE singleton = 1",
        )
        .get_result::<WindowRow>(&mut observer)
        .map_err(|_| TestFailure::FixtureIoFailed)?;
        let recovery_count = diesel::sql_query(
            "SELECT COUNT(*) AS value FROM audit_events WHERE actor = 'system:recovery'",
        )
        .get_result::<CountRow>(&mut observer)
        .map_err(|_| TestFailure::FixtureIoFailed)?
        .value;
        if counts_after != counts_before
            || version_after != version_before
            || window.state != "open"
            || window.revision != 1
            || recovery_count != 0
        {
            return Err(TestFailure::BootstrapRanServeRecovery);
        }
        Ok(())
    }

    #[tokio::test]
    async fn serve_missing_database_creates_no_sqlite_artifacts() -> Result<(), TestFailure> {
        let _subscriber_guard = SubscriberTestGuard::acquire();
        let identity =
            TestIdentity::new(LOCALHOST).map_err(|_| TestFailure::FixtureCreationFailed)?;
        let key_directory = identity.directory_path().join("keys");
        create_private_directory(&key_directory)?;
        let database_path = identity.directory_path().join("absent-server.db");
        let master_key_path = key_directory.join("server-root.key");
        ensure_master_key(&master_key_path).map_err(|_| TestFailure::FixtureCreationFailed)?;
        let config_path = write_config(
            &identity,
            SocketAddr::from((LOCALHOST, 0)),
            &database_path,
            &master_key_path,
            identity.certificate_path(),
            identity.private_key_path(),
        )?;
        let config = ServerConfig::load_from(&config_path)
            .map_err(|_| TestFailure::FixtureCreationFailed)?;

        assert_startup_error(run_until(config, ready(())).await, AppError::Database)?;
        for path in [
            database_path.clone(),
            sqlite_sidecar(&database_path, "wal"),
            sqlite_sidecar(&database_path, "shm"),
        ] {
            if path.exists() {
                return Err(TestFailure::UnexpectedDatabaseArtifact);
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn serve_missing_vault_key_creates_no_key_artifacts() -> Result<(), TestFailure> {
        let _subscriber_guard = SubscriberTestGuard::acquire();
        let identity =
            TestIdentity::new(LOCALHOST).map_err(|_| TestFailure::FixtureCreationFailed)?;
        let key_directory = identity.directory_path().join("keys");
        create_private_directory(&key_directory)?;
        let database_path = identity.directory_path().join("server.db");
        create_database(&database_path).await?;
        let master_key_path = key_directory.join("server-root.key");
        let config_path = write_config(
            &identity,
            SocketAddr::from((LOCALHOST, 0)),
            &database_path,
            &master_key_path,
            identity.certificate_path(),
            identity.private_key_path(),
        )?;
        let config = ServerConfig::load_from(&config_path)
            .map_err(|_| TestFailure::FixtureCreationFailed)?;

        assert_startup_error(run_until(config, ready(())).await, AppError::Vault)?;
        if master_key_path.exists() || master_key_path.with_extension("tmp").exists() {
            return Err(TestFailure::UnexpectedKeyArtifact);
        }
        Ok(())
    }

    #[tokio::test]
    async fn startup_failures_preserve_stage_order() -> Result<(), TestFailure> {
        let _subscriber_guard = SubscriberTestGuard::acquire();
        let invalid_config_identity =
            TestIdentity::new(LOCALHOST).map_err(|_| TestFailure::FixtureCreationFailed)?;
        let invalid_config_path = write_config(
            &invalid_config_identity,
            SocketAddr::from((LOCALHOST, 0)),
            Path::new("relative-database-canary.db"),
            &invalid_config_identity.directory_path().join("root.key"),
            invalid_config_identity.certificate_path(),
            invalid_config_identity.private_key_path(),
        )?;
        if ServerConfig::load_from(&invalid_config_path).is_ok() {
            return Err(TestFailure::ExpectedStartupFailure);
        }

        let invalid_database_identity =
            TestIdentity::new(LOCALHOST).map_err(|_| TestFailure::FixtureCreationFailed)?;
        let invalid_database_guard = StdTcpListener::bind(SocketAddr::from((LOCALHOST, 0)))
            .map_err(|_| TestFailure::FixtureCreationFailed)?;
        let invalid_database_address = invalid_database_guard
            .local_addr()
            .map_err(|_| TestFailure::FixtureCreationFailed)?;
        let invalid_database_path = write_config(
            &invalid_database_identity,
            invalid_database_address,
            &invalid_database_identity
                .directory_path()
                .join("missing")
                .join("server.db"),
            &invalid_database_identity.directory_path().join("root.key"),
            invalid_database_identity.certificate_path(),
            invalid_database_identity.private_key_path(),
        )?;
        let invalid_database_config = ServerConfig::load_from(&invalid_database_path)
            .map_err(|_| TestFailure::FixtureCreationFailed)?;
        assert_startup_error(
            run_until(invalid_database_config, ready(())).await,
            AppError::Database,
        )?;
        drop(invalid_database_guard);

        let invalid_vault_identity =
            TestIdentity::new(LOCALHOST).map_err(|_| TestFailure::FixtureCreationFailed)?;
        let invalid_vault_guard = StdTcpListener::bind(SocketAddr::from((LOCALHOST, 0)))
            .map_err(|_| TestFailure::FixtureCreationFailed)?;
        let invalid_vault_address = invalid_vault_guard
            .local_addr()
            .map_err(|_| TestFailure::FixtureCreationFailed)?;
        let wide_key_directory = invalid_vault_identity.directory_path().join("wide-keys");
        fs::create_dir(&wide_key_directory).map_err(|_| TestFailure::FixtureCreationFailed)?;
        fs::set_permissions(&wide_key_directory, fs::Permissions::from_mode(0o755))
            .map_err(|_| TestFailure::FixtureCreationFailed)?;
        let invalid_vault_database = invalid_vault_identity.directory_path().join("server.db");
        create_database(&invalid_vault_database).await?;
        let invalid_vault_path = write_config(
            &invalid_vault_identity,
            invalid_vault_address,
            &invalid_vault_database,
            &wide_key_directory.join("root.key"),
            invalid_vault_identity.certificate_path(),
            invalid_vault_identity.private_key_path(),
        )?;
        let invalid_vault_config = ServerConfig::load_from(&invalid_vault_path)
            .map_err(|_| TestFailure::FixtureCreationFailed)?;
        assert_startup_error(
            run_until(invalid_vault_config, ready(())).await,
            AppError::Vault,
        )?;
        drop(invalid_vault_guard);

        let invalid_tls_identity =
            TestIdentity::new(LOCALHOST).map_err(|_| TestFailure::FixtureCreationFailed)?;
        let valid_key_directory = invalid_tls_identity.directory_path().join("keys");
        create_private_directory(&valid_key_directory)?;
        let invalid_tls_database = invalid_tls_identity.directory_path().join("server.db");
        create_database(&invalid_tls_database).await?;
        let valid_master_key = valid_key_directory.join("root.key");
        ensure_master_key(&valid_master_key).map_err(|_| TestFailure::FixtureCreationFailed)?;
        fs::write(
            invalid_tls_identity.certificate_path(),
            b"invalid-startup-tls-canary",
        )
        .map_err(|_| TestFailure::FixtureCreationFailed)?;
        let invalid_tls_path = write_config(
            &invalid_tls_identity,
            SocketAddr::from((LOCALHOST, 0)),
            &invalid_tls_database,
            &valid_master_key,
            invalid_tls_identity.certificate_path(),
            invalid_tls_identity.private_key_path(),
        )?;
        let invalid_tls_config = ServerConfig::load_from(&invalid_tls_path)
            .map_err(|_| TestFailure::FixtureCreationFailed)?;
        assert_startup_error(
            run_until(invalid_tls_config, ready(())).await,
            AppError::Tls,
        )
    }

    #[tokio::test]
    async fn serve_runs_migrations_and_close_once_recovery() -> Result<(), TestFailure> {
        let _subscriber_guard = SubscriberTestGuard::acquire();
        let identity =
            TestIdentity::new(LOCALHOST).map_err(|_| TestFailure::FixtureCreationFailed)?;
        let key_directory = identity.directory_path().join("keys");
        create_private_directory(&key_directory)?;
        let master_key_path = key_directory.join("server-root.key");
        ensure_master_key(&master_key_path).map_err(|_| TestFailure::FixtureCreationFailed)?;
        let database_path = identity.directory_path().join("server.db");
        let empty_database = SqliteConnection::establish(
            database_path
                .to_str()
                .ok_or(TestFailure::FixtureCreationFailed)?,
        )
        .map_err(|_| TestFailure::FixtureCreationFailed)?;
        drop(empty_database);
        let config_path = write_config(
            &identity,
            SocketAddr::from((LOCALHOST, 0)),
            &database_path,
            &master_key_path,
            identity.certificate_path(),
            identity.private_key_path(),
        )?;
        let migration_config = ServerConfig::load_from(&config_path)
            .map_err(|_| TestFailure::FixtureCreationFailed)?;
        run_until(migration_config, ready(()))
            .await
            .map_err(|_| TestFailure::UnexpectedStartupFailure)?;

        let mut connection =
            test_observer(&database_path).map_err(|_| TestFailure::FixtureIoFailed)?;
        let migrated_table_count = diesel::sql_query(
            "SELECT COUNT(*) AS value FROM pragma_table_list WHERE name = 'site_identity'",
        )
        .get_result::<CountRow>(&mut connection)
        .map_err(|_| TestFailure::FixtureIoFailed)?
        .value;
        if migrated_table_count != 1 {
            return Err(TestFailure::MigrationsDidNotRun);
        }
        let opening_audit_id = uuid::Uuid::now_v7().to_string();
        let correlation_id = uuid::Uuid::now_v7().to_string();
        diesel::sql_query(
            "INSERT INTO audit_events (audit_event_id, occurred_at, actor, action_kind, \
             resource_type, resource_id, result, reason_code, correlation_id, \
             group_correlation_id, redacted_detail_json) VALUES (?, \
             strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), 'operator:test', \
             'open_provisioning_window', 'provisioning_window', NULL, 'succeeded', \
             NULL, ?, NULL, '{}')",
        )
        .bind::<Text, _>(&opening_audit_id)
        .bind::<Text, _>(&correlation_id)
        .execute(&mut connection)
        .map_err(|_| TestFailure::FixtureIoFailed)?;
        diesel::sql_query(
            "UPDATE provisioning_window SET state = 'open', revision = 1, \
             last_audit_event_id = ? WHERE singleton = 1",
        )
        .bind::<Text, _>(&opening_audit_id)
        .execute(&mut connection)
        .map_err(|_| TestFailure::FixtureIoFailed)?;
        drop(connection);

        let config = ServerConfig::load_from(&config_path)
            .map_err(|_| TestFailure::FixtureCreationFailed)?;
        run_until(config, ready(()))
            .await
            .map_err(|_| TestFailure::UnexpectedStartupFailure)?;

        let mut observer =
            test_observer(&database_path).map_err(|_| TestFailure::FixtureIoFailed)?;
        let window = diesel::sql_query(
            "SELECT state, revision FROM provisioning_window WHERE singleton = 1",
        )
        .get_result::<WindowRow>(&mut observer)
        .map_err(|_| TestFailure::FixtureIoFailed)?;
        let recovery_count = diesel::sql_query(
            "SELECT COUNT(*) AS value FROM audit_events WHERE actor = 'system:recovery'",
        )
        .get_result::<CountRow>(&mut observer)
        .map_err(|_| TestFailure::FixtureIoFailed)?
        .value;
        if window.state != "closed" || window.revision != 2 || recovery_count != 1 {
            return Err(TestFailure::RecoveryDidNotRun);
        }
        Ok(())
    }

    #[tokio::test]
    async fn startup_logging_is_complete_and_excludes_configuration_paths()
    -> Result<(), TestFailure> {
        let _subscriber_guard = SubscriberTestGuard::acquire();
        let identity =
            TestIdentity::new(LOCALHOST).map_err(|_| TestFailure::FixtureCreationFailed)?;
        let key_directory = identity.directory_path().join("structured-log-keys-canary");
        create_private_directory(&key_directory)?;
        let master_key_path = key_directory.join("structured-log-root-key-canary");
        ensure_master_key(&master_key_path).map_err(|_| TestFailure::FixtureCreationFailed)?;
        let database_path = identity
            .directory_path()
            .join("structured-log-database-canary.sqlite3");
        create_database(&database_path).await?;
        let config_path = write_config(
            &identity,
            SocketAddr::from((LOCALHOST, 0)),
            &database_path,
            &master_key_path,
            identity.certificate_path(),
            identity.private_key_path(),
        )?;
        let config = ServerConfig::load_from(&config_path)
            .map_err(|_| TestFailure::FixtureCreationFailed)?;
        let captured = CapturedLogs::default();
        let subscriber = captured.subscriber(LogLevel::Info);
        async {
            log_mode("serve");
            run_until(config, ready(())).await
        }
        .with_subscriber(subscriber)
        .await
        .map_err(|_| TestFailure::UnexpectedStartupFailure)?;
        let output = captured
            .text()
            .map_err(|()| TestFailure::LogCaptureFailed)?;
        for required in [
            "server mode running mode=\"serve\"",
            "database ready",
            "vault key verified",
            "TLS identity loaded",
            "listener bound listen_address=127.0.0.1:0",
            "graceful shutdown initiated",
            "graceful shutdown completed",
        ] {
            if !output.contains(required) {
                return Err(TestFailure::StartupLogContractChanged);
            }
        }
        for forbidden in [
            config_path.as_path(),
            database_path.as_path(),
            master_key_path.as_path(),
            identity.certificate_path(),
            identity.private_key_path(),
            identity.directory_path(),
        ] {
            if output.contains(forbidden.to_string_lossy().as_ref()) {
                return Err(TestFailure::StartupLogExposedPath);
            }
        }
        if output.to_ascii_uppercase().contains("SELECT")
            || output.to_ascii_uppercase().contains("INSERT")
        {
            return Err(TestFailure::StartupLogExposedPath);
        }
        Ok(())
    }

    #[tokio::test]
    async fn separate_bootstraps_use_distinct_password_salts() -> Result<(), TestFailure> {
        let first = TestIdentity::new(LOCALHOST).map_err(|_| TestFailure::FixtureCreationFailed)?;
        let second =
            TestIdentity::new(LOCALHOST).map_err(|_| TestFailure::FixtureCreationFailed)?;
        let first_salt = bootstrap_and_read_salt(&first, "first-admin", "same-password").await?;
        let second_salt = bootstrap_and_read_salt(&second, "second-admin", "same-password").await?;
        if first_salt == second_salt {
            return Err(TestFailure::PasswordSaltsMatched);
        }
        Ok(())
    }

    async fn bootstrap_and_read_salt(
        identity: &TestIdentity,
        login_name: &'static str,
        password: &'static str,
    ) -> Result<String, TestFailure> {
        let key_directory = identity.directory_path().join("keys");
        create_private_directory(&key_directory)?;
        let database_path = identity.directory_path().join("server.db");
        let config_path = write_config(
            identity,
            SocketAddr::from((LOCALHOST, 0)),
            &database_path,
            &key_directory.join("server-root.key"),
            &identity.directory_path().join("missing-certificate.der"),
            &identity.directory_path().join("missing-private-key.pk8"),
        )?;
        let config = ServerConfig::load_from(&config_path)
            .map_err(|_| TestFailure::FixtureCreationFailed)?;
        bootstrap_with(config, || credentials(login_name, password))
            .await
            .map_err(|_| TestFailure::UnexpectedStartupFailure)?;
        let database = Database::connect_and_migrate(&DatabaseConfig::new(&database_path, false))
            .await
            .map_err(|_| TestFailure::FixtureIoFailed)?;
        let encoded = db_operator::tests::test_password_hash(&database)
            .await
            .map_err(|_| TestFailure::FixtureIoFailed)?;
        let parsed = PasswordHash::new(&encoded).map_err(|_| TestFailure::InvalidPasswordHash)?;
        parsed
            .salt
            .map(|salt| salt.as_str().to_owned())
            .ok_or(TestFailure::InvalidPasswordHash)
    }

    fn credentials(login_name: &str, password: &str) -> Result<FirstAdminCredentials, AppError> {
        FirstAdminCredentials::new(
            login_name.to_owned(),
            password.to_owned(),
            password.to_owned(),
        )
        .map_err(|_| AppError::Bootstrap)
    }

    async fn create_database(path: &Path) -> Result<(), TestFailure> {
        Database::connect_and_migrate(&DatabaseConfig::new(path, true))
            .await
            .map_err(|_| TestFailure::FixtureCreationFailed)?;
        Ok(())
    }

    async fn business_counts(database: &Database) -> Result<(i64, i64), TestFailure> {
        db_operator::tests::test_business_counts(database)
            .await
            .map_err(|_| TestFailure::FixtureIoFailed)
    }

    fn bootstrap_business_counts(
        connection: &mut SqliteConnection,
    ) -> Result<(i64, i64, i64), TestFailure> {
        diesel::sql_query(
            "SELECT (SELECT COUNT(*) FROM operator_accounts) AS accounts, \
             (SELECT COUNT(*) FROM operator_sessions) AS sessions, \
             (SELECT COUNT(*) FROM audit_events) AS audits",
        )
        .get_result::<BootstrapCountsRow>(connection)
        .map(|row| (row.accounts, row.sessions, row.audits))
        .map_err(|_| TestFailure::FixtureIoFailed)
    }

    fn sqlite_sidecar(path: &Path, suffix: &str) -> PathBuf {
        PathBuf::from(format!("{}-{suffix}", path.display()))
    }

    fn write_config(
        identity: &TestIdentity,
        listen_address: SocketAddr,
        database_path: &Path,
        master_key_path: &Path,
        certificate_path: &Path,
        private_key_path: &Path,
    ) -> Result<PathBuf, TestFailure> {
        let config_path = identity.directory_path().join("config.toml");
        let config = format!(
            "[listen]\nhttps = \"{listen_address}\"\n\
             [storage]\ndatabase = \"{}\"\nroot_key = \"{}\"\n\
             [tls]\ncertificate = \"{}\"\nprivate_key = \"{}\"\n",
            database_path.display(),
            master_key_path.display(),
            certificate_path.display(),
            private_key_path.display(),
        );
        fs::write(&config_path, config).map_err(|_| TestFailure::FixtureCreationFailed)?;
        Ok(config_path)
    }

    fn create_private_directory(path: &Path) -> Result<(), TestFailure> {
        fs::create_dir(path).map_err(|_| TestFailure::FixtureCreationFailed)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|_| TestFailure::FixtureCreationFailed)
    }

    fn assert_startup_error(
        result: Result<(), AppError>,
        expected: AppError,
    ) -> Result<(), TestFailure> {
        match result {
            Err(error) if error == expected => Ok(()),
            Ok(()) | Err(_) => Err(TestFailure::UnexpectedStartupFailure),
        }
    }

    fn key_modified_at(path: &Path) -> Result<std::time::SystemTime, TestFailure> {
        fs::metadata(path)
            .and_then(|metadata| metadata.modified())
            .map_err(|_| TestFailure::FixtureIoFailed)
    }

    #[derive(QueryableByName)]
    struct BootstrapCountsRow {
        #[diesel(sql_type = BigInt)]
        accounts: i64,
        #[diesel(sql_type = BigInt)]
        sessions: i64,
        #[diesel(sql_type = BigInt)]
        audits: i64,
    }

    #[derive(QueryableByName)]
    struct WindowRow {
        #[diesel(sql_type = Text)]
        state: String,
        #[diesel(sql_type = BigInt)]
        revision: i64,
    }

    #[derive(QueryableByName)]
    struct CountRow {
        #[diesel(sql_type = BigInt)]
        value: i64,
    }

    #[derive(Debug, Snafu)]
    enum TestFailure {
        #[snafu(display("the startup fixture could not be created"))]
        FixtureCreationFailed,
        #[snafu(display("the startup fixture operation failed"))]
        FixtureIoFailed,
        #[snafu(display("the startup sequence failed unexpectedly"))]
        UnexpectedStartupFailure,
        #[snafu(display("captured startup logs could not be read"))]
        LogCaptureFailed,
        #[snafu(display("the startup logging contract changed"))]
        StartupLogContractChanged,
        #[snafu(display("startup logging exposed a configuration path"))]
        StartupLogExposedPath,
        #[snafu(display("a required startup artifact was not created"))]
        StartupArtifactMissing,
        #[snafu(display("serve mode created a database artifact"))]
        UnexpectedDatabaseArtifact,
        #[snafu(display("serve mode created a vault-key artifact"))]
        UnexpectedKeyArtifact,
        #[snafu(display("bootstrap created unexpected business rows"))]
        UnexpectedBusinessRows,
        #[snafu(display("repeated bootstrap changed business rows"))]
        RepeatedBootstrapWroteBusinessRows,
        #[snafu(display("the startup sequence rewrote the vault master key"))]
        MasterKeyWasRewritten,
        #[snafu(display("a startup failure was expected"))]
        ExpectedStartupFailure,
        #[snafu(display("serve mode did not run close-once recovery"))]
        RecoveryDidNotRun,
        #[snafu(display("bootstrap ran serve-only provisioning recovery"))]
        BootstrapRanServeRecovery,
        #[snafu(display("serve mode did not apply migrations"))]
        MigrationsDidNotRun,
        #[snafu(display("the persisted password hash was invalid"))]
        InvalidPasswordHash,
        #[snafu(display("independent bootstraps reused a password salt"))]
        PasswordSaltsMatched,
    }
}
