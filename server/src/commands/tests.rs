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
    sql_types::{BigInt, Binary, Text},
    sqlite::SqliteConnection,
};
use snafu::Snafu;
use tracing::instrument::WithSubscriber as _;
use zeroize::Zeroizing;

use crate::{
    application::operator::{
        OperatorCredentials, OperatorRole, hash_password, sign_in,
        tests::PasswordVerificationTestGuard,
    },
    audit::CorrelationId,
    config::{
        LogLevel, ORIGIN_CA_CERTIFICATE_FILENAME, ORIGIN_CA_PRIVATE_KEY_FILENAME, ServerConfig,
    },
    db::{
        Database, DatabaseConfig, operator as db_operator,
        tests::{test_data_version, test_observer},
    },
    error::CommandError,
    logging::tests::{CapturedLogs, SubscriberTestGuard},
    tls::tests::TestIdentity,
    vault::ensure_master_key,
};

use super::{bootstrap_with, log_mode, reset_operator_password_with, run_until};

const LOCALHOST: Ipv4Addr = Ipv4Addr::LOCALHOST;

#[tokio::test]
async fn bootstrap_creates_artifacts_and_repeat_is_zero_write() -> Result<(), TestFailure> {
    let identity = TestIdentity::new(LOCALHOST).map_err(|_| TestFailure::FixtureCreationFailed)?;
    let origin_material = origin_material_paths(&identity);
    for path in &origin_material {
        fs::remove_file(path).map_err(|_| TestFailure::FixtureIoFailed)?;
    }
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

    let first_config =
        ServerConfig::load_from(&config_path).map_err(|_| TestFailure::UnexpectedStartupFailure)?;
    bootstrap_with(first_config, || {
        credentials("first-admin", "bootstrap-password")
    })
    .await
    .map_err(|_| TestFailure::UnexpectedStartupFailure)?;
    if !database_path.is_file()
        || !master_key_path.is_file()
        || origin_material.iter().any(|path| path.exists())
    {
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

    let second_config =
        ServerConfig::load_from(&config_path).map_err(|_| TestFailure::UnexpectedStartupFailure)?;
    assert_startup_error(
        bootstrap_with(second_config, || {
            credentials("second-admin-canary", "second-password-canary")
        })
        .await,
        CommandError::Bootstrap,
    )?;
    let content_after =
        Zeroizing::new(fs::read(&master_key_path).map_err(|_| TestFailure::FixtureIoFailed)?);
    let modified_after = key_modified_at(&master_key_path)?;
    if content_before.as_slice() != content_after.as_slice() || modified_before != modified_after {
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
async fn repeated_bootstrap_does_not_recover_an_open_provisioning_window() -> Result<(), TestFailure>
{
    let identity = TestIdentity::new(LOCALHOST).map_err(|_| TestFailure::FixtureCreationFailed)?;
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

    let first_config =
        ServerConfig::load_from(&config_path).map_err(|_| TestFailure::FixtureCreationFailed)?;
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
        .test_write(move |connection| {
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

    let mut observer = test_observer(&database_path).map_err(|_| TestFailure::FixtureIoFailed)?;
    let counts_before = bootstrap_business_counts(&mut observer)?;
    let version_before =
        test_data_version(&mut observer).map_err(|_| TestFailure::FixtureIoFailed)?;

    let second_config =
        ServerConfig::load_from(&config_path).map_err(|_| TestFailure::FixtureCreationFailed)?;
    assert_startup_error(
        bootstrap_with(second_config, || {
            credentials("second-admin", "second-password")
        })
        .await,
        CommandError::Bootstrap,
    )?;

    let counts_after = bootstrap_business_counts(&mut observer)?;
    let version_after =
        test_data_version(&mut observer).map_err(|_| TestFailure::FixtureIoFailed)?;
    let window =
        diesel::sql_query("SELECT state, revision FROM provisioning_window WHERE singleton = 1")
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
async fn password_reset_updates_phc_removes_all_sessions_and_audits() -> Result<(), TestFailure> {
    let _verification_guard = PasswordVerificationTestGuard::acquire().await;
    let identity = TestIdentity::new(LOCALHOST).map_err(|_| TestFailure::FixtureCreationFailed)?;
    let (config_path, database_path, _) =
        bootstrap_password_reset_fixture(&identity, "reset-admin", "old-password").await?;
    let database = Database::connect_and_migrate(&DatabaseConfig::new(&database_path, false))
        .await
        .map_err(|_| TestFailure::FixtureIoFailed)?;
    let first_session = sign_in(
        &database,
        correlation_id(),
        "reset-admin",
        "old-password".to_owned(),
    )
    .await
    .map_err(|_| TestFailure::SessionFixtureFailed)?;
    let operator_id = first_session.identity().operator_id();
    let second_session = sign_in(
        &database,
        correlation_id(),
        "reset-admin",
        "old-password".to_owned(),
    )
    .await
    .map_err(|_| TestFailure::SessionFixtureFailed)?;
    if second_session.identity() != first_session.identity() {
        return Err(TestFailure::SessionFixtureFailed);
    }
    let counts_before = db_operator::tests::test_session_and_audit_counts(&database)
        .await
        .map_err(|_| TestFailure::FixtureIoFailed)?;

    let config =
        ServerConfig::load_from(&config_path).map_err(|_| TestFailure::FixtureCreationFailed)?;
    reset_operator_password_with(config, || {
        reset_credentials("reset-admin", "new-password", "new-password")
    })
    .await
    .map_err(|_| TestFailure::PasswordResetFailed)?;

    let counts_after = db_operator::tests::test_session_and_audit_counts(&database)
        .await
        .map_err(|_| TestFailure::FixtureIoFailed)?;
    if counts_before.0 != 2 || counts_after != (0, counts_before.1 + 1) {
        return Err(TestFailure::PasswordResetStateWasNotExact);
    }
    let mut observer = test_observer(&database_path).map_err(|_| TestFailure::FixtureIoFailed)?;
    let audits = password_reset_audits(&mut observer)?;
    if audits.len() != 1
        || audits[0].actor != "system:password-reset"
        || audits[0].action_kind != "reset_operator_password"
        || audits[0].resource_type != "operator_account"
        || audits[0].resource_id != operator_id.to_string()
        || audits[0].result != "succeeded"
        || audits[0].reason_code != "credential_recovery"
        || audits[0].redacted_detail_json != r#"{"removed_session_count":2}"#
    {
        return Err(TestFailure::PasswordResetAuditWasNotExact);
    }

    if sign_in(
        &database,
        correlation_id(),
        "reset-admin",
        "old-password".to_owned(),
    )
    .await
    .is_ok()
    {
        return Err(TestFailure::OldPasswordWasAccepted);
    }
    let signed_in = sign_in(
        &database,
        correlation_id(),
        "reset-admin",
        "new-password".to_owned(),
    )
    .await
    .map_err(|_| TestFailure::NewPasswordWasRejected)?;
    if signed_in.identity() != first_session.identity() {
        return Err(TestFailure::NewPasswordWasRejected);
    }
    Ok(())
}

#[tokio::test]
async fn password_reset_preserves_every_other_operator_session_and_phc() -> Result<(), TestFailure>
{
    let _verification_guard = PasswordVerificationTestGuard::acquire().await;
    let identity = TestIdentity::new(LOCALHOST).map_err(|_| TestFailure::FixtureCreationFailed)?;
    let (config_path, database_path, _) =
        bootstrap_password_reset_fixture(&identity, "isolation-admin-a", "password-a").await?;
    let database = Database::connect_and_migrate(&DatabaseConfig::new(&database_path, false))
        .await
        .map_err(|_| TestFailure::FixtureIoFailed)?;
    let other_credentials = OperatorCredentials::new(
        "isolation-operator-b".to_owned(),
        "password-b".to_owned(),
        "password-b".to_owned(),
    )
    .map_err(|_| TestFailure::FixtureCreationFailed)?;
    let other_password_hash = hash_password(other_credentials.password())
        .map_err(|_| TestFailure::FixtureCreationFailed)?;
    let other_id = db_operator::tests::test_insert_account(
        &database,
        "isolation-operator-b",
        OperatorRole::Viewer,
        &other_password_hash,
    )
    .await
    .map_err(|_| TestFailure::FixtureCreationFailed)?;

    let target_session = sign_in(
        &database,
        correlation_id(),
        "isolation-admin-a",
        "password-a".to_owned(),
    )
    .await
    .map_err(|_| TestFailure::SessionFixtureFailed)?;
    sign_in(
        &database,
        correlation_id(),
        "isolation-admin-a",
        "password-a".to_owned(),
    )
    .await
    .map_err(|_| TestFailure::SessionFixtureFailed)?;
    for _ in 0..2 {
        let session = sign_in(
            &database,
            correlation_id(),
            "isolation-operator-b",
            "password-b".to_owned(),
        )
        .await
        .map_err(|_| TestFailure::SessionFixtureFailed)?;
        if session.identity().operator_id() != other_id {
            return Err(TestFailure::PasswordResetOperatorIsolationFailed);
        }
    }
    let target_id = target_session.identity().operator_id();
    let mut observer = test_observer(&database_path).map_err(|_| TestFailure::FixtureIoFailed)?;
    let preserved_password_hash = operator_password_hash(&mut observer, other_id)?;
    let target_session_hashes = operator_session_hashes(&mut observer, target_id)?;
    let preserved_session_hashes = operator_session_hashes(&mut observer, other_id)?;
    if preserved_password_hash != other_password_hash
        || target_session_hashes.len() != 2
        || preserved_session_hashes.len() != 2
    {
        return Err(TestFailure::PasswordResetOperatorIsolationFailed);
    }

    let config =
        ServerConfig::load_from(&config_path).map_err(|_| TestFailure::FixtureCreationFailed)?;
    reset_operator_password_with(config, || {
        reset_credentials("isolation-admin-a", "new-password-a", "new-password-a")
    })
    .await
    .map_err(|_| TestFailure::PasswordResetFailed)?;

    let observed_password_hash = operator_password_hash(&mut observer, other_id)?;
    let remaining_target_sessions = operator_session_hashes(&mut observer, target_id)?;
    let surviving_other_sessions = operator_session_hashes(&mut observer, other_id)?;
    let audits = password_reset_audits(&mut observer)?;
    let expected_detail = format!(
        "{{\"removed_session_count\":{}}}",
        target_session_hashes.len()
    );
    if observed_password_hash != preserved_password_hash
        || surviving_other_sessions != preserved_session_hashes
        || !remaining_target_sessions.is_empty()
        || audits.len() != 1
        || audits[0].resource_id != target_id.to_string()
        || audits[0].redacted_detail_json != expected_detail
    {
        return Err(TestFailure::PasswordResetOperatorIsolationFailed);
    }
    Ok(())
}

#[tokio::test]
async fn password_reset_unknown_login_is_a_zero_write_rejection() -> Result<(), TestFailure> {
    let _verification_guard = PasswordVerificationTestGuard::acquire().await;
    let identity = TestIdentity::new(LOCALHOST).map_err(|_| TestFailure::FixtureCreationFailed)?;
    let (config_path, database_path, _) =
        bootstrap_password_reset_fixture(&identity, "known-reset-admin", "old-password").await?;
    let database = Database::connect_and_migrate(&DatabaseConfig::new(&database_path, false))
        .await
        .map_err(|_| TestFailure::FixtureIoFailed)?;
    sign_in(
        &database,
        correlation_id(),
        "known-reset-admin",
        "old-password".to_owned(),
    )
    .await
    .map_err(|_| TestFailure::SessionFixtureFailed)?;
    let state_before = password_reset_state(&database).await?;
    let mut observer = test_observer(&database_path).map_err(|_| TestFailure::FixtureIoFailed)?;
    let version_before =
        test_data_version(&mut observer).map_err(|_| TestFailure::FixtureIoFailed)?;
    let login_canary = "unknown-reset-login-canary";
    let password_canary = "unknown-reset-password-canary";

    let config =
        ServerConfig::load_from(&config_path).map_err(|_| TestFailure::FixtureCreationFailed)?;
    let error = reset_operator_password_with(config, || {
        reset_credentials(login_canary, password_canary, password_canary)
    })
    .await
    .err()
    .ok_or(TestFailure::ExpectedPasswordResetFailure)?;
    if error != CommandError::PasswordReset {
        return Err(TestFailure::UnexpectedPasswordResetFailure);
    }
    for encoded in [error.to_string(), format!("{error:?}")] {
        if encoded.contains(login_canary) || encoded.contains(password_canary) {
            return Err(TestFailure::PasswordResetErrorExposedCredentials);
        }
    }

    let version_after =
        test_data_version(&mut observer).map_err(|_| TestFailure::FixtureIoFailed)?;
    if password_reset_state(&database).await? != state_before
        || version_after != version_before
        || !password_reset_audits(&mut observer)?.is_empty()
    {
        return Err(TestFailure::RejectedPasswordResetWroteState);
    }
    Ok(())
}

#[tokio::test]
async fn password_reset_missing_database_creates_no_sqlite_artifacts() -> Result<(), TestFailure> {
    let identity = TestIdentity::new(LOCALHOST).map_err(|_| TestFailure::FixtureCreationFailed)?;
    let key_directory = identity.directory_path().join("keys");
    create_private_directory(&key_directory)?;
    let database_path = identity.directory_path().join("absent-reset.db");
    let master_key_path = key_directory.join("absent-root.key");
    let config_path = write_config(
        &identity,
        SocketAddr::from((LOCALHOST, 0)),
        &database_path,
        &master_key_path,
        &identity.directory_path().join("missing-certificate.der"),
        &identity.directory_path().join("missing-private-key.pk8"),
    )?;
    let config =
        ServerConfig::load_from(&config_path).map_err(|_| TestFailure::FixtureCreationFailed)?;
    let credentials_read = std::cell::Cell::new(false);

    assert_startup_error(
        reset_operator_password_with(config, || {
            credentials_read.set(true);
            reset_credentials("reset-admin", "new-password", "new-password")
        })
        .await,
        CommandError::Database,
    )?;
    if credentials_read.get() {
        return Err(TestFailure::CredentialsReadBeforeDatabase);
    }
    for path in [
        database_path.clone(),
        sqlite_sidecar(&database_path, "wal"),
        sqlite_sidecar(&database_path, "shm"),
    ] {
        if path.exists() {
            return Err(TestFailure::PasswordResetCreatedDatabaseArtifact);
        }
    }
    if master_key_path.exists() || master_key_path.with_extension("tmp").exists() {
        return Err(TestFailure::PasswordResetTouchedVault);
    }
    Ok(())
}

#[tokio::test]
async fn password_reset_confirmation_mismatch_precedes_every_business_write()
-> Result<(), TestFailure> {
    let _verification_guard = PasswordVerificationTestGuard::acquire().await;
    let identity = TestIdentity::new(LOCALHOST).map_err(|_| TestFailure::FixtureCreationFailed)?;
    let (config_path, database_path, _) =
        bootstrap_password_reset_fixture(&identity, "mismatch-reset-admin", "old-password").await?;
    let database = Database::connect_and_migrate(&DatabaseConfig::new(&database_path, false))
        .await
        .map_err(|_| TestFailure::FixtureIoFailed)?;
    sign_in(
        &database,
        correlation_id(),
        "mismatch-reset-admin",
        "old-password".to_owned(),
    )
    .await
    .map_err(|_| TestFailure::SessionFixtureFailed)?;
    let state_before = password_reset_state(&database).await?;
    let mut observer = test_observer(&database_path).map_err(|_| TestFailure::FixtureIoFailed)?;
    let version_before =
        test_data_version(&mut observer).map_err(|_| TestFailure::FixtureIoFailed)?;

    let config =
        ServerConfig::load_from(&config_path).map_err(|_| TestFailure::FixtureCreationFailed)?;
    assert_startup_error(
        reset_operator_password_with(config, || {
            reset_credentials("mismatch-reset-admin", "new-password", "different-password")
        })
        .await,
        CommandError::PasswordReset,
    )?;

    let version_after =
        test_data_version(&mut observer).map_err(|_| TestFailure::FixtureIoFailed)?;
    if password_reset_state(&database).await? != state_before
        || version_after != version_before
        || !password_reset_audits(&mut observer)?.is_empty()
    {
        return Err(TestFailure::RejectedPasswordResetWroteState);
    }
    Ok(())
}

#[tokio::test]
async fn password_reset_succeeds_without_the_vault_master_key() -> Result<(), TestFailure> {
    let _verification_guard = PasswordVerificationTestGuard::acquire().await;
    let identity = TestIdentity::new(LOCALHOST).map_err(|_| TestFailure::FixtureCreationFailed)?;
    let (config_path, database_path, master_key_path) =
        bootstrap_password_reset_fixture(&identity, "vault-independent-admin", "old-password")
            .await?;
    let database = Database::connect_and_migrate(&DatabaseConfig::new(&database_path, false))
        .await
        .map_err(|_| TestFailure::FixtureIoFailed)?;
    sign_in(
        &database,
        correlation_id(),
        "vault-independent-admin",
        "old-password".to_owned(),
    )
    .await
    .map_err(|_| TestFailure::SessionFixtureFailed)?;
    fs::remove_file(&master_key_path).map_err(|_| TestFailure::FixtureIoFailed)?;
    let origin_material = origin_material_paths(&identity);
    for path in &origin_material {
        fs::remove_file(path).map_err(|_| TestFailure::FixtureIoFailed)?;
    }

    let config =
        ServerConfig::load_from(&config_path).map_err(|_| TestFailure::FixtureCreationFailed)?;
    reset_operator_password_with(config, || {
        reset_credentials("vault-independent-admin", "new-password", "new-password")
    })
    .await
    .map_err(|_| TestFailure::PasswordResetFailed)?;
    if master_key_path.exists()
        || master_key_path.with_extension("tmp").exists()
        || origin_material.iter().any(|path| path.exists())
    {
        return Err(TestFailure::PasswordResetTouchedVault);
    }
    let counts = db_operator::tests::test_session_and_audit_counts(&database)
        .await
        .map_err(|_| TestFailure::FixtureIoFailed)?;
    let mut observer = test_observer(&database_path).map_err(|_| TestFailure::FixtureIoFailed)?;
    if counts.0 != 0 || password_reset_audits(&mut observer)?.len() != 1 {
        return Err(TestFailure::PasswordResetStateWasNotExact);
    }
    sign_in(
        &database,
        correlation_id(),
        "vault-independent-admin",
        "new-password".to_owned(),
    )
    .await
    .map_err(|_| TestFailure::NewPasswordWasRejected)?;
    Ok(())
}

#[tokio::test]
async fn repeated_password_reset_with_the_same_input_succeeds() -> Result<(), TestFailure> {
    let _verification_guard = PasswordVerificationTestGuard::acquire().await;
    let identity = TestIdentity::new(LOCALHOST).map_err(|_| TestFailure::FixtureCreationFailed)?;
    let (config_path, database_path, _) =
        bootstrap_password_reset_fixture(&identity, "repeat-reset-admin", "old-password").await?;
    let database = Database::connect_and_migrate(&DatabaseConfig::new(&database_path, false))
        .await
        .map_err(|_| TestFailure::FixtureIoFailed)?;
    sign_in(
        &database,
        correlation_id(),
        "repeat-reset-admin",
        "old-password".to_owned(),
    )
    .await
    .map_err(|_| TestFailure::SessionFixtureFailed)?;
    let counts_before = db_operator::tests::test_session_and_audit_counts(&database)
        .await
        .map_err(|_| TestFailure::FixtureIoFailed)?;

    for _ in 0..2 {
        let config = ServerConfig::load_from(&config_path)
            .map_err(|_| TestFailure::FixtureCreationFailed)?;
        reset_operator_password_with(config, || {
            reset_credentials("repeat-reset-admin", "new-password", "new-password")
        })
        .await
        .map_err(|_| TestFailure::PasswordResetFailed)?;
    }

    let counts_after = db_operator::tests::test_session_and_audit_counts(&database)
        .await
        .map_err(|_| TestFailure::FixtureIoFailed)?;
    let mut observer = test_observer(&database_path).map_err(|_| TestFailure::FixtureIoFailed)?;
    let audits = password_reset_audits(&mut observer)?;
    if counts_after != (0, counts_before.1 + 2)
        || audits.len() != 2
        || audits[0].redacted_detail_json != r#"{"removed_session_count":1}"#
        || audits[1].redacted_detail_json != r#"{"removed_session_count":0}"#
    {
        return Err(TestFailure::RepeatedPasswordResetWasNotExact);
    }
    sign_in(
        &database,
        correlation_id(),
        "repeat-reset-admin",
        "new-password".to_owned(),
    )
    .await
    .map_err(|_| TestFailure::NewPasswordWasRejected)?;
    Ok(())
}

#[tokio::test]
async fn serve_missing_database_creates_no_sqlite_artifacts() -> Result<(), TestFailure> {
    let _subscriber_guard = SubscriberTestGuard::acquire();
    let identity = TestIdentity::new(LOCALHOST).map_err(|_| TestFailure::FixtureCreationFailed)?;
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
    let config =
        ServerConfig::load_from(&config_path).map_err(|_| TestFailure::FixtureCreationFailed)?;

    assert_startup_error(run_until(config, ready(())).await, CommandError::Database)?;
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
    let identity = TestIdentity::new(LOCALHOST).map_err(|_| TestFailure::FixtureCreationFailed)?;
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
    let config =
        ServerConfig::load_from(&config_path).map_err(|_| TestFailure::FixtureCreationFailed)?;

    assert_startup_error(run_until(config, ready(())).await, CommandError::Vault)?;
    if master_key_path.exists() || master_key_path.with_extension("tmp").exists() {
        return Err(TestFailure::UnexpectedKeyArtifact);
    }
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
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

    let invalid_site_identity =
        TestIdentity::new(LOCALHOST).map_err(|_| TestFailure::FixtureCreationFailed)?;
    let invalid_site_path = write_config(
        &invalid_site_identity,
        SocketAddr::from((LOCALHOST, 0)),
        &invalid_site_identity.directory_path().join("missing.db"),
        &invalid_site_identity.directory_path().join("root.key"),
        invalid_site_identity.certificate_path(),
        invalid_site_identity.private_key_path(),
    )?;
    fs::write(
        invalid_site_identity.directory_path().join("site.toml"),
        "gateway_hostname = 'malformed-site-canary'\n",
    )
    .map_err(|_| TestFailure::FixtureCreationFailed)?;
    let invalid_site_config = ServerConfig::load_from(&invalid_site_path)
        .map_err(|_| TestFailure::FixtureCreationFailed)?;
    assert_startup_error(
        run_until(invalid_site_config, ready(())).await,
        CommandError::SiteConfiguration,
    )?;

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
        CommandError::Database,
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
        CommandError::Vault,
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
        CommandError::Tls,
    )
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn serve_runs_migrations_and_close_once_recovery() -> Result<(), TestFailure> {
    let _subscriber_guard = SubscriberTestGuard::acquire();
    let identity = TestIdentity::new(LOCALHOST).map_err(|_| TestFailure::FixtureCreationFailed)?;
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
    let migration_config =
        ServerConfig::load_from(&config_path).map_err(|_| TestFailure::FixtureCreationFailed)?;
    run_until(migration_config, ready(()))
        .await
        .map_err(|_| TestFailure::UnexpectedStartupFailure)?;

    let mut connection = test_observer(&database_path).map_err(|_| TestFailure::FixtureIoFailed)?;
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
    let recovery_device_id = uuid::Uuid::now_v7().to_string();
    let recovery_request_id = uuid::Uuid::now_v7().to_string();
    diesel::sql_query(
        "INSERT INTO devices (device_pk, machine_hardware_id, \
         hardware_identity_quality, state) VALUES (?, \
         '00000000-0000-5000-8000-000000000001', 'strong', 'enrolled')",
    )
    .bind::<Text, _>(&recovery_device_id)
    .execute(&mut connection)
    .map_err(|_| TestFailure::FixtureIoFailed)?;
    diesel::sql_query(
        "INSERT INTO enrollment_requests (enrollment_request_id, \
         machine_hardware_id, hardware_identity_quality, gateway_csr_der, \
         gateway_spki_sha256, client_version, protocol_version, source_ip, state, \
         resolution, resolved_device_pk, issuance_audit_event_id, created_at) \
         VALUES (?, '00000000-0000-5000-8000-000000000001', 'strong', X'30', \
         zeroblob(32), 'recovery-test', 1, '127.0.0.1', 'approved', \
         'replace_device_credentials', ?, NULL, \
         strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
    )
    .bind::<Text, _>(&recovery_request_id)
    .bind::<Text, _>(&recovery_device_id)
    .execute(&mut connection)
    .map_err(|_| TestFailure::FixtureIoFailed)?;
    drop(connection);

    let config =
        ServerConfig::load_from(&config_path).map_err(|_| TestFailure::FixtureCreationFailed)?;
    run_until(config, ready(()))
        .await
        .map_err(|_| TestFailure::UnexpectedStartupFailure)?;

    let mut observer = test_observer(&database_path).map_err(|_| TestFailure::FixtureIoFailed)?;
    let window =
        diesel::sql_query("SELECT state, revision FROM provisioning_window WHERE singleton = 1")
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
async fn startup_logging_is_complete_and_excludes_configuration_paths() -> Result<(), TestFailure> {
    let _subscriber_guard = SubscriberTestGuard::acquire();
    let identity = TestIdentity::new(LOCALHOST).map_err(|_| TestFailure::FixtureCreationFailed)?;
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
    let config =
        ServerConfig::load_from(&config_path).map_err(|_| TestFailure::FixtureCreationFailed)?;
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
    let second = TestIdentity::new(LOCALHOST).map_err(|_| TestFailure::FixtureCreationFailed)?;
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
    let config =
        ServerConfig::load_from(&config_path).map_err(|_| TestFailure::FixtureCreationFailed)?;
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

fn credentials(login_name: &str, password: &str) -> Result<OperatorCredentials, CommandError> {
    OperatorCredentials::new(
        login_name.to_owned(),
        password.to_owned(),
        password.to_owned(),
    )
    .map_err(|_| CommandError::Bootstrap)
}

fn reset_credentials(
    login_name: &str,
    password: &str,
    password_confirmation: &str,
) -> Result<OperatorCredentials, CommandError> {
    OperatorCredentials::new(
        login_name.to_owned(),
        password.to_owned(),
        password_confirmation.to_owned(),
    )
    .map_err(|_| CommandError::PasswordReset)
}

async fn bootstrap_password_reset_fixture(
    identity: &TestIdentity,
    login_name: &str,
    password: &str,
) -> Result<(PathBuf, PathBuf, PathBuf), TestFailure> {
    let key_directory = identity.directory_path().join("keys");
    create_private_directory(&key_directory)?;
    let database_path = identity.directory_path().join("server.db");
    let master_key_path = key_directory.join("server-root.key");
    let config_path = write_config(
        identity,
        SocketAddr::from((LOCALHOST, 0)),
        &database_path,
        &master_key_path,
        &identity.directory_path().join("missing-certificate.der"),
        &identity.directory_path().join("missing-private-key.pk8"),
    )?;
    let config =
        ServerConfig::load_from(&config_path).map_err(|_| TestFailure::FixtureCreationFailed)?;
    bootstrap_with(config, || credentials(login_name, password))
        .await
        .map_err(|_| TestFailure::FixtureCreationFailed)?;
    Ok((config_path, database_path, master_key_path))
}

async fn password_reset_state(
    database: &Database,
) -> Result<(String, Vec<Vec<u8>>, i64), TestFailure> {
    let password_hash = db_operator::tests::test_password_hash(database)
        .await
        .map_err(|_| TestFailure::FixtureIoFailed)?;
    let mut session_hashes = db_operator::tests::test_session_hashes(database)
        .await
        .map_err(|_| TestFailure::FixtureIoFailed)?;
    session_hashes.sort();
    let (_, audit_count) = db_operator::tests::test_session_and_audit_counts(database)
        .await
        .map_err(|_| TestFailure::FixtureIoFailed)?;
    Ok((password_hash, session_hashes, audit_count))
}

fn password_reset_audits(
    connection: &mut SqliteConnection,
) -> Result<Vec<PasswordResetAuditRow>, TestFailure> {
    diesel::sql_query(
        "SELECT actor, action_kind, resource_type, COALESCE(resource_id, '') AS resource_id, \
         result, COALESCE(reason_code, '') AS reason_code, redacted_detail_json \
         FROM audit_events WHERE action_kind = 'reset_operator_password' ORDER BY rowid",
    )
    .load(connection)
    .map_err(|_| TestFailure::FixtureIoFailed)
}

fn operator_password_hash(
    connection: &mut SqliteConnection,
    operator_id: uuid::Uuid,
) -> Result<String, TestFailure> {
    diesel::sql_query("SELECT password_hash AS value FROM operator_accounts WHERE operator_id = ?")
        .bind::<Text, _>(operator_id.to_string())
        .get_result::<TextValueRow>(connection)
        .map(|row| row.value)
        .map_err(|_| TestFailure::FixtureIoFailed)
}

fn operator_session_hashes(
    connection: &mut SqliteConnection,
    operator_id: uuid::Uuid,
) -> Result<Vec<Vec<u8>>, TestFailure> {
    diesel::sql_query(
        "SELECT session_credential_hash AS value FROM operator_sessions \
         WHERE operator_id = ? ORDER BY session_credential_hash",
    )
    .bind::<Text, _>(operator_id.to_string())
    .load::<BinaryValueRow>(connection)
    .map(|rows| rows.into_iter().map(|row| row.value).collect())
    .map_err(|_| TestFailure::FixtureIoFailed)
}

fn correlation_id() -> CorrelationId {
    CorrelationId::from_uuid(uuid::Uuid::now_v7())
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

fn origin_material_paths(identity: &TestIdentity) -> [PathBuf; 2] {
    [
        identity
            .directory_path()
            .join(ORIGIN_CA_CERTIFICATE_FILENAME),
        identity
            .directory_path()
            .join(ORIGIN_CA_PRIVATE_KEY_FILENAME),
    ]
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
    let site_config_path = identity.directory_path().join("site.toml");
    fs::write(
        &site_config_path,
        "gateway_hostname = \"gateway.contest.example\"\n\
         gateway_not_after = \"4090-01-01T00:00:00Z\"\n\
         contest_end = \"4089-12-31T00:00:00Z\"\n",
    )
    .map_err(|_| TestFailure::FixtureCreationFailed)?;
    let config = format!(
        "[listen]\nhttps = \"{listen_address}\"\n\
         [storage]\ndatabase = \"{}\"\nroot_key = \"{}\"\n\
         [tls]\ncertificate = \"{}\"\nprivate_key = \"{}\"\n\
         [site]\nconfig = \"{}\"\ncontrol_root = \"{}\"\nlocal_origin_root = \"{}\"\n",
        database_path.display(),
        master_key_path.display(),
        certificate_path.display(),
        private_key_path.display(),
        site_config_path.display(),
        identity.directory_path().join("control-ca.crt").display(),
        identity
            .directory_path()
            .join("local-origin-ca.crt")
            .display(),
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
    result: Result<(), CommandError>,
    expected: CommandError,
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

#[derive(QueryableByName)]
struct PasswordResetAuditRow {
    #[diesel(sql_type = Text)]
    actor: String,
    #[diesel(sql_type = Text)]
    action_kind: String,
    #[diesel(sql_type = Text)]
    resource_type: String,
    #[diesel(sql_type = Text)]
    resource_id: String,
    #[diesel(sql_type = Text)]
    result: String,
    #[diesel(sql_type = Text)]
    reason_code: String,
    #[diesel(sql_type = Text)]
    redacted_detail_json: String,
}

#[derive(QueryableByName)]
struct TextValueRow {
    #[diesel(sql_type = Text)]
    value: String,
}

#[derive(QueryableByName)]
struct BinaryValueRow {
    #[diesel(sql_type = Binary)]
    value: Vec<u8>,
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
    #[snafu(display("the password-reset session fixture failed"))]
    SessionFixtureFailed,
    #[snafu(display("operator password reset failed unexpectedly"))]
    PasswordResetFailed,
    #[snafu(display("an operator password reset failure was expected"))]
    ExpectedPasswordResetFailure,
    #[snafu(display("operator password reset returned an unexpected failure"))]
    UnexpectedPasswordResetFailure,
    #[snafu(display("operator password reset exposed rejected credentials"))]
    PasswordResetErrorExposedCredentials,
    #[snafu(display("operator password reset did not make the exact state transition"))]
    PasswordResetStateWasNotExact,
    #[snafu(display("the operator password-reset audit was not exact"))]
    PasswordResetAuditWasNotExact,
    #[snafu(display("the old operator password was accepted after reset"))]
    OldPasswordWasAccepted,
    #[snafu(display("the new operator password was rejected after reset"))]
    NewPasswordWasRejected,
    #[snafu(display("a rejected operator password reset wrote state"))]
    RejectedPasswordResetWroteState,
    #[snafu(display("operator password reset read credentials before opening the database"))]
    CredentialsReadBeforeDatabase,
    #[snafu(display("operator password reset created a database artifact"))]
    PasswordResetCreatedDatabaseArtifact,
    #[snafu(display("operator password reset touched the vault master key"))]
    PasswordResetTouchedVault,
    #[snafu(display("repeated operator password reset was not exact"))]
    RepeatedPasswordResetWasNotExact,
    #[snafu(display("operator password reset crossed the target-operator boundary"))]
    PasswordResetOperatorIsolationFailed,
}
