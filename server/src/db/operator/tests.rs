use std::{collections::BTreeSet, fs, path::PathBuf};

use argon2::{
    Algorithm, Argon2, Params, Version,
    password_hash::{PasswordHash, PasswordVerifier},
};
use diesel::{
    ExpressionMethods, QueryDsl, QueryableByName, RunQueryDsl,
    connection::SimpleConnection,
    sql_types::{BigInt, Binary, Text},
    sqlite::SqliteConnection,
};
use sha2::{Digest, Sha256};
use snafu::Snafu;
use uuid::Uuid;

use crate::{
    application::operator::{
        OperatorCredentials, OperatorError, OperatorIdentity, OperatorRole, SessionCredentialHex,
        authenticate_session, hash_password, terminate_session as terminate_application_session,
        tests::{operator_identity, session_credential_hash},
    },
    audit::{self, AuditEvent, AuditEventId, CorrelationId},
    db::{
        Database, DatabaseConfig,
        schema::{audit_events, operator_accounts, operator_sessions},
        tests::{test_lock_database, test_observer},
    },
};

use super::{
    CreateFirstAdminError, OperatorStoreError, ResetOperatorPasswordError, create_first_admin,
    create_first_admin_with_ids, create_session, create_session_with_audit_id, read_session,
    reset_operator_password_with_ids, terminate_session_with_audit_id,
};

// The fixture helpers below are called from outside `db`, so they speak the
// application error. Every caller collapses the failure into one
// `TestFailure`, so the store vocabulary would add nothing.
pub(crate) async fn test_business_counts(database: &Database) -> Result<(i64, i64), OperatorError> {
    database
        .interact(|connection| {
            let accounts = operator_accounts::table
                .count()
                .get_result(connection)
                .map_err(|_| OperatorError::PersistenceFailed)?;
            let audits = audit_events::table
                .count()
                .get_result(connection)
                .map_err(|_| OperatorError::PersistenceFailed)?;
            Ok((accounts, audits))
        })
        .await
        .map_err(|_| OperatorError::PersistenceFailed)?
}

pub(crate) async fn test_password_hash(database: &Database) -> Result<String, OperatorError> {
    database
        .interact(|connection| {
            operator_accounts::table
                .select(operator_accounts::password_hash)
                .first(connection)
                .map_err(|_| OperatorError::PersistenceFailed)
        })
        .await
        .map_err(|_| OperatorError::PersistenceFailed)?
}

pub(crate) async fn test_insert_account(
    database: &Database,
    login_name: &str,
    role: OperatorRole,
    password_hash: &str,
) -> Result<Uuid, OperatorError> {
    let operator_id = Uuid::now_v7();
    let login_name = login_name.to_owned();
    let password_hash = password_hash.to_owned();
    database
        .interact(move |connection| {
            diesel::insert_into(operator_accounts::table)
                .values((
                    operator_accounts::operator_id.eq(operator_id.to_string()),
                    operator_accounts::login_name.eq(login_name),
                    operator_accounts::role.eq(role.as_persisted()),
                    operator_accounts::password_hash.eq(password_hash),
                ))
                .execute(connection)
                .map_err(|_| OperatorError::PersistenceFailed)?;
            Ok(operator_id)
        })
        .await
        .map_err(|_| OperatorError::PersistenceFailed)?
}

pub(crate) async fn test_session_hashes(
    database: &Database,
) -> Result<Vec<Vec<u8>>, OperatorError> {
    database
        .interact(|connection| {
            operator_sessions::table
                .select(operator_sessions::session_credential_hash)
                .load(connection)
                .map_err(|_| OperatorError::PersistenceFailed)
        })
        .await
        .map_err(|_| OperatorError::PersistenceFailed)?
}

pub(crate) async fn test_session_and_audit_counts(
    database: &Database,
) -> Result<(i64, i64), OperatorError> {
    database
        .interact(|connection| {
            let sessions = operator_sessions::table
                .count()
                .get_result(connection)
                .map_err(|_| OperatorError::PersistenceFailed)?;
            let audits = audit_events::table
                .count()
                .get_result(connection)
                .map_err(|_| OperatorError::PersistenceFailed)?;
            Ok((sessions, audits))
        })
        .await
        .map_err(|_| OperatorError::PersistenceFailed)?
}

/// Reads the database clock so callers can bracket an insert.
pub(crate) async fn test_now(database: &Database) -> Result<String, OperatorError> {
    database
        .interact(|connection| {
            diesel::sql_query("SELECT strftime('%Y-%m-%dT%H:%M:%fZ', 'now') AS value")
                .get_result::<TestTextRow>(connection)
                .map(|row| row.value)
                .map_err(|_| OperatorError::PersistenceFailed)
        })
        .await
        .map_err(|_| OperatorError::PersistenceFailed)?
}
/// `operator_sessions` has no `created_at`, so the caller brackets the
/// inserts with [`test_now`] and the frozen TTL is asserted against that
/// bracket. A wall-clock tolerance would instead fail under load.
pub(crate) async fn test_sessions_have_eight_hour_ttl(
    database: &Database,
    before_insert: &str,
    after_insert: &str,
) -> Result<bool, OperatorError> {
    let before_insert = before_insert.to_owned();
    let after_insert = after_insert.to_owned();
    database
        .interact(move |connection| {
            diesel::sql_query(
                "SELECT COUNT(*) > 0 AND MIN(\
                 expires_at >= strftime('%Y-%m-%dT%H:%M:%fZ', ?, '+57600 seconds') \
                 AND expires_at <= strftime('%Y-%m-%dT%H:%M:%fZ', ?, '+57600 seconds')\
             ) AS value FROM operator_sessions",
            )
            .bind::<Text, _>(before_insert)
            .bind::<Text, _>(after_insert)
            .get_result::<TestIntegerRow>(connection)
            .map(|row| row.value == 1)
            .map_err(|_| OperatorError::PersistenceFailed)
        })
        .await
        .map_err(|_| OperatorError::PersistenceFailed)?
}

pub(crate) async fn test_database_contains_credential_canary(
    database: &Database,
    wire_credential: &str,
) -> Result<bool, OperatorError> {
    let wire_credential = wire_credential.to_owned();
    database
        .interact(move |connection| {
            let table_names = diesel::sql_query(
                "SELECT name AS value FROM pragma_table_list WHERE schema = 'main' \
             AND name NOT LIKE 'sqlite_%' ORDER BY name",
            )
            .load::<TestTextRow>(connection)
            .map_err(|_| OperatorError::PersistenceFailed)?;
            for table_name in table_names {
                let columns = diesel::sql_query(
                    "SELECT name AS value FROM pragma_table_xinfo(?) ORDER BY cid",
                )
                .bind::<Text, _>(&table_name.value)
                .load::<TestTextRow>(connection)
                .map_err(|_| OperatorError::PersistenceFailed)?;
                if columns.is_empty() {
                    continue;
                }
                let projection = columns
                    .iter()
                    .map(|column| format!("quote({})", quote_identifier(&column.value)))
                    .collect::<Vec<_>>()
                    .join(" || '|' || ");
                let query = format!(
                    "SELECT ({projection}) AS value FROM {}",
                    quote_identifier(&table_name.value)
                );
                let rows = diesel::sql_query(query)
                    .load::<TestTextRow>(connection)
                    .map_err(|_| OperatorError::PersistenceFailed)?;
                if rows
                    .iter()
                    .any(|row| row.value.to_ascii_lowercase().contains(&wire_credential))
                {
                    return Ok(true);
                }
            }
            Ok(false)
        })
        .await
        .map_err(|_| OperatorError::PersistenceFailed)?
}

#[derive(QueryableByName)]
struct TestIntegerRow {
    #[diesel(sql_type = BigInt)]
    value: i64,
}

#[derive(QueryableByName)]
struct TestTextRow {
    #[diesel(sql_type = Text)]
    value: String,
}

#[tokio::test]
async fn first_admin_is_persisted_with_frozen_hash_and_redacted_audit() -> Result<(), TestFailure> {
    let fixture = TestDatabase::new().await?;
    let login_name = "first-admin-login-canary";
    let password = "first-admin-password-canary";
    let credentials = credentials(login_name, password)?;
    let password_hash =
        hash_password(credentials.password()).map_err(|_| TestFailure::PasswordHashingFailed)?;

    let operator_id =
        create_first_admin(&fixture.database, credentials.login_name(), &password_hash)
            .await
            .map_err(|_| TestFailure::FirstAdminCreationFailed)?;

    if operator_id.get_version_num() != 7 {
        return Err(TestFailure::OperatorIdWasNotUuidV7);
    }
    let account = fixture
        .database
        .interact(|connection| {
            operator_accounts::table
                .select((
                    operator_accounts::operator_id,
                    operator_accounts::login_name,
                    operator_accounts::role,
                    operator_accounts::password_hash,
                ))
                .first::<(String, String, String, String)>(connection)
        })
        .await
        .map_err(|_| TestFailure::OperatorAccountWasNotReadable)?
        .map_err(|_| TestFailure::OperatorAccountWasNotReadable)?;
    if account.0 != operator_id.to_string()
        || account.1 != login_name
        || account.2 != "admin"
        || account.3 != password_hash
    {
        return Err(TestFailure::PersistedOperatorAccountWasNotExact);
    }

    let parsed = PasswordHash::new(&account.3).map_err(|_| TestFailure::PersistedPhcWasInvalid)?;
    if parsed.algorithm.as_str() != "argon2id"
        || parsed.version != Some(19)
        || parsed.params.get_decimal("m") != Some(19_456)
        || parsed.params.get_decimal("t") != Some(2)
        || parsed.params.get_decimal("p") != Some(1)
    {
        return Err(TestFailure::PersistedArgon2ProfileWasNotFrozen);
    }
    let verifier = frozen_argon2()?;
    verifier
        .verify_password(password.as_bytes(), &parsed)
        .map_err(|_| TestFailure::CorrectPasswordWasRejected)?;
    if verifier
        .verify_password(b"incorrect-password-canary", &parsed)
        .is_ok()
    {
        return Err(TestFailure::IncorrectPasswordWasAccepted);
    }

    let audit = fixture
        .database
        .interact(|connection| {
            audit_events::table
                .select((
                    audit_events::actor,
                    audit_events::action_kind,
                    audit_events::resource_type,
                    audit_events::resource_id,
                    audit_events::result,
                    audit_events::reason_code,
                    audit_events::redacted_detail_json,
                ))
                .first::<(
                    String,
                    String,
                    String,
                    Option<String>,
                    String,
                    Option<String>,
                    String,
                )>(connection)
        })
        .await
        .map_err(|_| TestFailure::FirstAdminAuditWasNotReadable)?
        .map_err(|_| TestFailure::FirstAdminAuditWasNotReadable)?;
    if audit.0 != "system:bootstrap"
        || audit.1 != "create_first_admin"
        || audit.2 != "operator_account"
        || audit.3.as_deref() != Some(operator_id.to_string().as_str())
        || audit.4 != "succeeded"
        || audit.5.as_deref() != Some("initial_provisioning")
        || audit.6 != r#"{"role":"admin"}"#
    {
        return Err(TestFailure::FirstAdminAuditWasNotExact);
    }
    assert_database_excludes_canaries(&fixture.database, &[login_name, password, &password_hash])
        .await?;
    Ok(())
}

#[tokio::test]
async fn repeated_first_admin_creation_is_a_zero_write_rejection() -> Result<(), TestFailure> {
    let fixture = TestDatabase::new().await?;
    create_first_admin(&fixture.database, "first-admin", "first-hash")
        .await
        .map_err(|_| TestFailure::FirstAdminCreationFailed)?;
    let before = counts(&fixture.database).await?;

    // The typed first-admin vocabulary stops at the module boundary, so the
    // rejection is asserted against the ID-injecting inner function the public
    // wrapper delegates to. `app::tests` covers the wrapper's own rejection.
    let Err(error) = create_first_admin_with_ids(
        &fixture.database,
        "second-admin-login-canary",
        "second-password-hash-canary",
        Uuid::now_v7(),
        AuditEventId::from_uuid(Uuid::now_v7()),
        correlation_id(),
    )
    .await
    else {
        return Err(TestFailure::RepeatedFirstAdminCreationSucceeded);
    };

    if error != CreateFirstAdminError::AccountAlreadyExists {
        return Err(TestFailure::RepeatedFirstAdminErrorWasNotTyped);
    }
    if counts(&fixture.database).await? != before {
        return Err(TestFailure::RepeatedFirstAdminCreationWroteRows);
    }
    for encoded in [error.to_string(), format!("{error:?}")] {
        if encoded.contains("second-admin-login-canary")
            || encoded.contains("second-password-hash-canary")
        {
            return Err(TestFailure::FirstAdminErrorWasNotRedacted);
        }
    }
    Ok(())
}

#[tokio::test]
async fn audit_insert_failure_rolls_back_the_account() -> Result<(), TestFailure> {
    let fixture = TestDatabase::new().await?;
    let duplicate_audit_id = AuditEventId::from_uuid(Uuid::now_v7());
    insert_existing_audit(&fixture.database, duplicate_audit_id).await?;

    let Err(error) = create_first_admin_with_ids(
        &fixture.database,
        "rollback-login-canary",
        "rollback-hash-canary",
        Uuid::now_v7(),
        duplicate_audit_id,
        CorrelationId::from_uuid(Uuid::now_v7()),
    )
    .await
    else {
        return Err(TestFailure::DuplicateAuditIdCommitted);
    };
    if error != CreateFirstAdminError::AuditInsertFailed {
        return Err(TestFailure::AuditInsertFailureWasNotTyped);
    }
    if counts(&fixture.database).await? != (0, 1) {
        return Err(TestFailure::AuditInsertFailureDidNotRollBackAccount);
    }
    Ok(())
}

#[tokio::test]
async fn password_reset_audit_failure_rolls_back_phc_and_session_purge() -> Result<(), TestFailure>
{
    let fixture = TestDatabase::new().await?;
    let operator_id = test_insert_account(
        &fixture.database,
        "atomic-reset-admin",
        OperatorRole::Admin,
        "old-reset-phc",
    )
    .await
    .map_err(|_| TestFailure::OperatorFixtureInsertFailed)?;
    let credential_hash = session_credential_hash(&[0x2a_u8; 32]);
    create_session(
        &fixture.database,
        &credential_hash,
        operator_identity(operator_id, OperatorRole::Admin),
        correlation_id(),
    )
    .await
    .map_err(|_| TestFailure::SessionCreationFailed)?;
    let duplicate_audit_id = AuditEventId::from_uuid(Uuid::now_v7());
    insert_existing_audit(&fixture.database, duplicate_audit_id).await?;
    let password_hash_before = test_password_hash(&fixture.database)
        .await
        .map_err(|_| TestFailure::OperatorAccountWasNotReadable)?;
    let sessions_before = test_session_hashes(&fixture.database)
        .await
        .map_err(|_| TestFailure::SessionRowWasNotReadable)?;
    let audit_count_before = audit_count(&fixture.database).await?;

    let error = reset_operator_password_with_ids(
        &fixture.database,
        "atomic-reset-admin",
        "new-reset-phc",
        duplicate_audit_id,
        correlation_id(),
    )
    .await
    .err()
    .ok_or(TestFailure::DuplicatePasswordResetAuditCommitted)?;
    if error != ResetOperatorPasswordError::AuditPersistenceFailed {
        return Err(TestFailure::PasswordResetAuditFailureWasNotTyped);
    }
    let password_hash_after = test_password_hash(&fixture.database)
        .await
        .map_err(|_| TestFailure::OperatorAccountWasNotReadable)?;
    let sessions_after = test_session_hashes(&fixture.database)
        .await
        .map_err(|_| TestFailure::SessionRowWasNotReadable)?;
    if password_hash_after != password_hash_before
        || sessions_after != sessions_before
        || audit_count(&fixture.database).await? != audit_count_before
    {
        return Err(TestFailure::PasswordResetAuditFailureDidNotRollBack);
    }
    Ok(())
}

#[tokio::test]
async fn session_creation_persists_raw_hash_frozen_ttl_and_exact_audit() -> Result<(), TestFailure>
{
    let fixture = TestDatabase::new().await?;
    let identity = insert_operator(&fixture.database, "session-admin", OperatorRole::Admin).await?;
    let raw = [0x3c_u8; 32];
    let wire = "3c".repeat(32);
    let credential_hash = session_credential_hash(&raw);
    // `operator_sessions` has no `created_at`, so the insert is bracketed by
    // two database-clock reads and the frozen TTL is asserted against that
    // bracket. A wall-clock tolerance would instead fail under load.
    let before_insert = test_now(&fixture.database)
        .await
        .map_err(|_| TestFailure::SessionRowWasNotReadable)?;
    create_session(
        &fixture.database,
        &credential_hash,
        identity,
        correlation_id(),
    )
    .await
    .map_err(|_| TestFailure::SessionCreationFailed)?;
    let after_insert = test_now(&fixture.database)
        .await
        .map_err(|_| TestFailure::SessionRowWasNotReadable)?;

    let row = fixture
        .database
        .interact(move |connection| {
            diesel::sql_query(
                "SELECT session_credential_hash AS credential_hash, operator_id, expires_at, \
                 expires_at >= strftime('%Y-%m-%dT%H:%M:%fZ', ?, '+57600 seconds') \
                 AND expires_at <= strftime('%Y-%m-%dT%H:%M:%fZ', ?, '+57600 seconds') \
                     AS ttl_valid FROM operator_sessions",
            )
            .bind::<Text, _>(before_insert)
            .bind::<Text, _>(after_insert)
            .get_result::<SessionCreationRow>(connection)
        })
        .await
        .map_err(|_| TestFailure::SessionRowWasNotReadable)?
        .map_err(|_| TestFailure::SessionRowWasNotReadable)?;
    let raw_hash = Sha256::digest(raw);
    let forbidden_hex_hash = Sha256::digest(wire.as_bytes());
    if row.credential_hash.as_slice() != raw_hash.as_slice()
        || row.credential_hash.as_slice() == forbidden_hex_hash.as_slice()
        || row.credential_hash.len() != 32
        || row.operator_id != identity.operator_id().to_string()
        || row.expires_at.is_empty()
        || row.ttl_valid != 1
    {
        return Err(TestFailure::PersistedSessionRowWasNotExact);
    }
    if test_database_contains_credential_canary(&fixture.database, &wire)
        .await
        .map_err(|_| TestFailure::SessionEvidenceWasNotReadable)?
    {
        return Err(TestFailure::SessionCredentialEscapedIntoDatabase);
    }
    let audit = latest_audit(&fixture.database).await?;
    if audit.actor != "operator:self"
        || audit.action != "establish_session"
        || audit.resource_type != "operator_session"
        || audit.resource_id != identity.operator_id().to_string()
        || audit.result != "succeeded"
        || audit.reason != "credentials_verified"
        || audit.detail != r#"{"role":"admin"}"#
    {
        return Err(TestFailure::SessionEstablishedAuditWasNotExact);
    }

    let error = create_session(
        &fixture.database,
        &credential_hash,
        identity,
        correlation_id(),
    )
    .await
    .err()
    .ok_or(TestFailure::DuplicateSessionCreationSucceeded)?;
    for encoded in [error.to_string(), format!("{error:?}")] {
        if encoded.contains(&wire) {
            return Err(TestFailure::SessionStoreErrorWasNotRedacted);
        }
    }
    Ok(())
}

#[tokio::test]
async fn session_reads_do_not_renew_and_expiry_is_lazy_once() -> Result<(), TestFailure> {
    let fixture = TestDatabase::new().await?;
    let identity = insert_operator(&fixture.database, "expiry-admin", OperatorRole::Admin).await?;
    let credential_hash = session_credential_hash(&[0x4d_u8; 32]);
    create_session(
        &fixture.database,
        &credential_hash,
        identity,
        correlation_id(),
    )
    .await
    .map_err(|_| TestFailure::SessionCreationFailed)?;
    let mut observer = fixture.observer()?;

    let expires_before = session_expiry(&fixture.database).await?;
    let snapshot_before = snapshot(&fixture.database, &mut observer).await?;
    let Some(facts) = read_session(&fixture.database, &credential_hash, correlation_id())
        .await
        .map_err(|_| TestFailure::SessionReadFailed)?
    else {
        return Err(TestFailure::LiveSessionWasNotActive);
    };
    if facts.operator_id != identity.operator_id().to_string()
        || facts.role != "admin"
        || session_expiry(&fixture.database).await? != expires_before
        || snapshot(&fixture.database, &mut observer).await? != snapshot_before
    {
        return Err(TestFailure::LiveSessionReadChangedState);
    }

    fixture
        .database
        .interact(|connection| {
            diesel::update(operator_sessions::table)
                .set(operator_sessions::expires_at.eq(diesel::dsl::sql::<Text>(
                    "strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-1 second')",
                )))
                .execute(connection)
        })
        .await
        .map_err(|_| TestFailure::SessionExpiryFixtureFailed)?
        .map_err(|_| TestFailure::SessionExpiryFixtureFailed)?;
    let before_expiry_read = snapshot(&fixture.database, &mut observer).await?;
    let expiry_error = authenticate_session(
        &fixture.database,
        correlation_id(),
        SessionCredentialHex::new("4d".repeat(32)),
    )
    .await
    .err()
    .ok_or(TestFailure::ExpiredSessionAuthenticated)?;
    if expiry_error != OperatorError::SessionAuthenticationFailed {
        return Err(TestFailure::SessionAuthenticationErrorWasNotUnified);
    }
    let after_expiry_read = snapshot(&fixture.database, &mut observer).await?;
    if after_expiry_read.sessions != 0
        || after_expiry_read.audits != before_expiry_read.audits + 1
        || after_expiry_read.data_version == before_expiry_read.data_version
    {
        return Err(TestFailure::FirstExpiryReadWasNotOneTransition);
    }
    let audit = latest_audit(&fixture.database).await?;
    if audit.actor != "system:expiry"
        || audit.action != "expire_session"
        || audit.resource_type != "operator_session"
        || audit.resource_id != identity.operator_id().to_string()
        || audit.result != "succeeded"
        || audit.reason != "absolute_expiry_observed"
        || audit.detail != "{}"
    {
        return Err(TestFailure::SessionExpiredAuditWasNotExact);
    }

    let before_second_read = snapshot(&fixture.database, &mut observer).await?;
    if read_session(&fixture.database, &credential_hash, correlation_id())
        .await
        .map_err(|_| TestFailure::SessionReadFailed)?
        .is_some()
        || snapshot(&fixture.database, &mut observer).await? != before_second_read
    {
        return Err(TestFailure::SecondExpiryReadWroteState);
    }
    Ok(())
}

#[tokio::test]
async fn live_session_reads_do_not_take_the_write_lock() -> Result<(), TestFailure> {
    let fixture = TestDatabase::new().await?;
    let identity =
        insert_operator(&fixture.database, "write-lock-admin", OperatorRole::Admin).await?;
    let credential_hash = session_credential_hash(&[0x91_u8; 32]);
    create_session(
        &fixture.database,
        &credential_hash,
        identity,
        correlation_id(),
    )
    .await
    .map_err(|_| TestFailure::SessionCreationFailed)?;
    let _write_lock = fixture.write_lock()?;

    let Some(facts) = read_session(&fixture.database, &credential_hash, correlation_id())
        .await
        .map_err(|_| TestFailure::LiveSessionReadTookTheWriteLock)?
    else {
        return Err(TestFailure::LiveSessionWasNotActive);
    };
    if facts.operator_id != identity.operator_id().to_string() || facts.role != "admin" {
        return Err(TestFailure::LiveSessionWasNotActive);
    }
    Ok(())
}

/// The expired-cleanup log is the only place the internal cause survives,
/// so a busy database and corrupted persisted facts must stay tellable
/// apart there. Every discriminant is a compile-time constant.
#[test]
fn every_store_failure_has_a_distinct_static_cause() -> Result<(), TestFailure> {
    let causes = [
        OperatorStoreError::AcquireFailed,
        OperatorStoreError::TransactionFailed,
        OperatorStoreError::AccountReadFailed,
        OperatorStoreError::SessionReadFailed,
        OperatorStoreError::ExpiredSessionCleanupFailed,
        OperatorStoreError::InvalidPersistedFacts,
        OperatorStoreError::SessionInsertFailed,
        OperatorStoreError::SessionDeleteFailed,
        OperatorStoreError::SessionDeleteConflict,
        OperatorStoreError::AuditInsertFailed,
    ]
    .map(OperatorStoreError::cause);
    if causes.iter().collect::<BTreeSet<_>>().len() != causes.len() {
        return Err(TestFailure::StoreCauseWasNotDistinct);
    }
    if causes.iter().any(|cause| {
        cause.is_empty()
            || !cause
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
    }) {
        return Err(TestFailure::StoreCauseWasNotAStaticDiscriminant);
    }
    Ok(())
}

#[test]
fn every_password_reset_failure_has_a_distinct_static_cause() -> Result<(), TestFailure> {
    let causes = [
        ResetOperatorPasswordError::DatabaseAcquireFailed,
        ResetOperatorPasswordError::TransactionControlFailed,
        ResetOperatorPasswordError::TargetReadFailed,
        ResetOperatorPasswordError::TargetNotFound,
        ResetOperatorPasswordError::PasswordUpdateFailed,
        ResetOperatorPasswordError::PasswordUpdateConflict,
        ResetOperatorPasswordError::SessionsPurgeFailed,
        ResetOperatorPasswordError::AuditPersistenceFailed,
    ]
    .map(ResetOperatorPasswordError::cause);
    if causes.iter().collect::<BTreeSet<_>>().len() != causes.len() {
        return Err(TestFailure::StoreCauseWasNotDistinct);
    }
    if causes.iter().any(|cause| {
        cause.is_empty()
            || !cause
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
    }) {
        return Err(TestFailure::StoreCauseWasNotAStaticDiscriminant);
    }
    Ok(())
}

/// A write-locked database blocks the lazy-expiry escalation, and only the
/// expired credential reaches it. The frozen mapping assigns `401` to an
/// expired session, so the failure must not surface as an internal one.
#[tokio::test]
async fn expired_cleanup_failure_is_indistinguishable_from_an_unknown_credential()
-> Result<(), TestFailure> {
    let fixture = TestDatabase::new().await?;
    let identity = insert_operator(
        &fixture.database,
        "cleanup-failure-admin",
        OperatorRole::Admin,
    )
    .await?;
    let live_hash = session_credential_hash(&[0xa1_u8; 32]);
    let expired_hash = session_credential_hash(&[0xa2_u8; 32]);
    for credential_hash in [&live_hash, &expired_hash] {
        create_session(
            &fixture.database,
            credential_hash,
            identity,
            correlation_id(),
        )
        .await
        .map_err(|_| TestFailure::SessionCreationFailed)?;
    }
    let expired_hash_bytes = *expired_hash.as_bytes();
    fixture
        .database
        .interact(move |connection| {
            diesel::update(operator_sessions::table.filter(
                operator_sessions::session_credential_hash.eq(expired_hash_bytes.as_slice()),
            ))
            .set(operator_sessions::expires_at.eq(diesel::dsl::sql::<Text>(
                "strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-1 second')",
            )))
            .execute(connection)
        })
        .await
        .map_err(|_| TestFailure::SessionExpiryFixtureFailed)?
        .map_err(|_| TestFailure::SessionExpiryFixtureFailed)?;
    let _write_lock = fixture.write_lock()?;

    let expired = authenticate_session(
        &fixture.database,
        correlation_id(),
        SessionCredentialHex::new("a2".repeat(32)),
    )
    .await
    .err()
    .ok_or(TestFailure::ExpiredSessionAuthenticated)?;
    let unknown = authenticate_session(
        &fixture.database,
        correlation_id(),
        SessionCredentialHex::new("a3".repeat(32)),
    )
    .await
    .err()
    .ok_or(TestFailure::InvalidSessionAuthenticated)?;
    let live = authenticate_session(
        &fixture.database,
        correlation_id(),
        SessionCredentialHex::new("a1".repeat(32)),
    )
    .await
    .map_err(|_| TestFailure::LiveSessionReadTookTheWriteLock)?;

    if expired != OperatorError::SessionAuthenticationFailed
        || unknown != expired
        || expired.to_string() != unknown.to_string()
        || format!("{expired:?}") != format!("{unknown:?}")
    {
        return Err(TestFailure::ExpiredCleanupFailureWasDistinguishable);
    }
    if live != identity {
        return Err(TestFailure::LiveSessionWasNotActive);
    }
    if session_count(&fixture.database).await? != 2 {
        return Err(TestFailure::ExpiredCleanupFailureWroteState);
    }
    Ok(())
}

#[tokio::test]
async fn malformed_and_boundary_expiry_facts_fail_closed() -> Result<(), TestFailure> {
    let fixture = TestDatabase::new().await?;
    let identity =
        insert_operator(&fixture.database, "expiry-facts-admin", OperatorRole::Admin).await?;

    let malformed_hash = session_credential_hash(&[0x4e_u8; 32]);
    create_session(
        &fixture.database,
        &malformed_hash,
        identity,
        correlation_id(),
    )
    .await
    .map_err(|_| TestFailure::SessionCreationFailed)?;
    let malformed_hash_bytes = *malformed_hash.as_bytes();
    fixture
        .database
        .interact(move |connection| {
            connection.batch_execute("PRAGMA ignore_check_constraints = ON;")?;
            diesel::update(operator_sessions::table.filter(
                operator_sessions::session_credential_hash.eq(malformed_hash_bytes.as_slice()),
            ))
            .set(operator_sessions::expires_at.eq("malformed-expiry-canary"))
            .execute(connection)?;
            connection.batch_execute("PRAGMA ignore_check_constraints = OFF;")
        })
        .await
        .map_err(|_| TestFailure::SessionExpiryFixtureFailed)?
        .map_err(|_| TestFailure::SessionExpiryFixtureFailed)?;

    let malformed_error = authenticate_session(
        &fixture.database,
        correlation_id(),
        SessionCredentialHex::new("4e".repeat(32)),
    )
    .await
    .err()
    .ok_or(TestFailure::MalformedExpiryAuthenticated)?;
    if malformed_error != OperatorError::PersistenceFailed
        || session_count(&fixture.database).await? != 1
    {
        return Err(TestFailure::MalformedExpiryDidNotFailClosed);
    }

    let boundary_hash = session_credential_hash(&[0x4f_u8; 32]);
    create_session(
        &fixture.database,
        &boundary_hash,
        identity,
        correlation_id(),
    )
    .await
    .map_err(|_| TestFailure::SessionCreationFailed)?;
    let boundary_hash_bytes = *boundary_hash.as_bytes();
    fixture
        .database
        .interact(move |connection| {
            diesel::update(operator_sessions::table.filter(
                operator_sessions::session_credential_hash.eq(boundary_hash_bytes.as_slice()),
            ))
            .set(operator_sessions::expires_at.eq(diesel::dsl::sql::<Text>(
                "strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
            )))
            .execute(connection)
        })
        .await
        .map_err(|_| TestFailure::SessionExpiryFixtureFailed)?
        .map_err(|_| TestFailure::SessionExpiryFixtureFailed)?;
    let audit_count_before = audit_count(&fixture.database).await?;
    let boundary_error = authenticate_session(
        &fixture.database,
        correlation_id(),
        SessionCredentialHex::new("4f".repeat(32)),
    )
    .await
    .err()
    .ok_or(TestFailure::BoundaryExpiryAuthenticated)?;
    if boundary_error != OperatorError::SessionAuthenticationFailed
        || session_count(&fixture.database).await? != 1
        || audit_count(&fixture.database).await? != audit_count_before + 1
    {
        return Err(TestFailure::BoundaryExpiryWasNotExpired);
    }
    Ok(())
}

#[tokio::test]
async fn termination_is_repeat_safe_and_concurrent_once() -> Result<(), TestFailure> {
    let fixture = TestDatabase::new().await?;
    let identity =
        insert_operator(&fixture.database, "termination-admin", OperatorRole::Admin).await?;
    let raw = [0x5e_u8; 32];
    let wire_text = "5e".repeat(32);
    let credential_hash = session_credential_hash(&raw);
    create_session(
        &fixture.database,
        &credential_hash,
        identity,
        correlation_id(),
    )
    .await
    .map_err(|_| TestFailure::SessionCreationFailed)?;
    let mut observer = fixture.observer()?;
    let before_termination = snapshot(&fixture.database, &mut observer).await?;
    terminate_application_session(
        &fixture.database,
        correlation_id(),
        SessionCredentialHex::new(wire_text.clone()),
    )
    .await
    .map_err(|_| TestFailure::SessionTerminationFailed)?;
    let after_termination = snapshot(&fixture.database, &mut observer).await?;
    if after_termination.sessions != 0 || after_termination.audits != before_termination.audits + 1
    {
        return Err(TestFailure::LiveTerminationWasNotOneTransition);
    }
    let audit = latest_audit(&fixture.database).await?;
    if audit.actor != "operator:self"
        || audit.action != "terminate_session"
        || audit.resource_type != "operator_session"
        || audit.resource_id != identity.operator_id().to_string()
        || audit.result != "succeeded"
        || audit.reason != "operator_requested"
        || audit.detail != "{}"
    {
        return Err(TestFailure::SessionTerminatedAuditWasNotExact);
    }
    let authentication_error = authenticate_session(
        &fixture.database,
        correlation_id(),
        SessionCredentialHex::new(wire_text.clone()),
    )
    .await
    .err()
    .ok_or(TestFailure::TerminatedSessionAuthenticated)?;
    if authentication_error != OperatorError::SessionAuthenticationFailed {
        return Err(TestFailure::SessionAuthenticationErrorWasNotUnified);
    }

    assert_termination_noops(&fixture.database, &mut observer, wire_text).await?;
    assert_concurrent_termination_once(&fixture.database, &mut observer, identity).await
}

async fn assert_termination_noops(
    database: &Database,
    observer: &mut SqliteConnection,
    deleted_wire: String,
) -> Result<(), TestFailure> {
    let before_noops = snapshot(database, observer).await?;
    let noops = [
        String::new(),
        "not-lowercase-hex".to_owned(),
        "6f".repeat(32),
        deleted_wire,
    ];
    for value in &noops {
        let error = authenticate_session(
            database,
            correlation_id(),
            SessionCredentialHex::new(value.clone()),
        )
        .await
        .err()
        .ok_or(TestFailure::InvalidSessionAuthenticated)?;
        if error != OperatorError::SessionAuthenticationFailed {
            return Err(TestFailure::SessionAuthenticationErrorWasNotUnified);
        }
    }
    for value in noops {
        terminate_application_session(database, correlation_id(), SessionCredentialHex::new(value))
            .await
            .map_err(|_| TestFailure::SessionTerminationFailed)?;
    }
    if snapshot(database, observer).await? != before_noops {
        return Err(TestFailure::TerminationNoopsWroteState);
    }
    Ok(())
}

async fn assert_concurrent_termination_once(
    database: &Database,
    observer: &mut SqliteConnection,
    identity: OperatorIdentity,
) -> Result<(), TestFailure> {
    let concurrent_raw = [0x70_u8; 32];
    let concurrent_hash = session_credential_hash(&concurrent_raw);
    create_session(database, &concurrent_hash, identity, correlation_id())
        .await
        .map_err(|_| TestFailure::SessionCreationFailed)?;
    let concurrent_wire = "70".repeat(32);
    let before_concurrent = snapshot(database, observer).await?;
    let (first, second) = tokio::join!(
        terminate_application_session(
            database,
            correlation_id(),
            SessionCredentialHex::new(concurrent_wire.clone()),
        ),
        terminate_application_session(
            database,
            correlation_id(),
            SessionCredentialHex::new(concurrent_wire),
        ),
    );
    first.map_err(|_| TestFailure::SessionTerminationFailed)?;
    second.map_err(|_| TestFailure::SessionTerminationFailed)?;
    let after_concurrent = snapshot(database, observer).await?;
    if after_concurrent.sessions != 0 || after_concurrent.audits != before_concurrent.audits + 1 {
        return Err(TestFailure::ConcurrentTerminationWasNotOneTransition);
    }
    Ok(())
}

#[tokio::test]
async fn session_audit_failures_roll_back_creation_and_termination() -> Result<(), TestFailure> {
    let fixture = TestDatabase::new().await?;
    let identity = insert_operator(&fixture.database, "atomic-admin", OperatorRole::Admin).await?;
    let duplicate_audit_id = AuditEventId::from_uuid(Uuid::now_v7());
    insert_existing_audit(&fixture.database, duplicate_audit_id).await?;

    let creation_hash = session_credential_hash(&[0x81_u8; 32]);
    let creation_error = create_session_with_audit_id(
        &fixture.database,
        &creation_hash,
        identity,
        correlation_id(),
        duplicate_audit_id,
    )
    .await
    .err()
    .ok_or(TestFailure::DuplicateSessionAuditCommitted)?;
    if creation_error != OperatorStoreError::AuditInsertFailed
        || session_count(&fixture.database).await? != 0
    {
        return Err(TestFailure::CreationAuditFailureDidNotRollBack);
    }

    let termination_hash = session_credential_hash(&[0x82_u8; 32]);
    create_session(
        &fixture.database,
        &termination_hash,
        identity,
        correlation_id(),
    )
    .await
    .map_err(|_| TestFailure::SessionCreationFailed)?;
    let audit_count_before = audit_count(&fixture.database).await?;
    let termination_error = terminate_session_with_audit_id(
        &fixture.database,
        &termination_hash,
        correlation_id(),
        duplicate_audit_id,
    )
    .await
    .err()
    .ok_or(TestFailure::DuplicateSessionAuditCommitted)?;
    if termination_error != OperatorStoreError::AuditInsertFailed
        || session_count(&fixture.database).await? != 1
        || audit_count(&fixture.database).await? != audit_count_before
    {
        return Err(TestFailure::TerminationAuditFailureDidNotRollBack);
    }
    Ok(())
}

async fn insert_operator(
    database: &Database,
    login_name: &str,
    role: OperatorRole,
) -> Result<OperatorIdentity, TestFailure> {
    let operator_id = test_insert_account(database, login_name, role, "test-password-phc")
        .await
        .map_err(|_| TestFailure::OperatorFixtureInsertFailed)?;
    Ok(operator_identity(operator_id, role))
}

async fn insert_existing_audit(
    database: &Database,
    audit_event_id: AuditEventId,
) -> Result<(), TestFailure> {
    database
        .interact(move |connection| {
            let event =
                AuditEvent::first_admin_created(audit_event_id, correlation_id(), Uuid::now_v7());
            audit::insert_diesel(connection, &event)
        })
        .await
        .map_err(|_| TestFailure::AuditFixtureInsertFailed)?
        .map_err(|_| TestFailure::AuditFixtureInsertFailed)
}

async fn latest_audit(database: &Database) -> Result<AuditRow, TestFailure> {
    let row = database
        .interact(|connection| {
            diesel::sql_query(
                "SELECT actor, action_kind AS action, resource_type, resource_id, result, \
                 reason_code AS reason, redacted_detail_json AS detail \
                 FROM audit_events ORDER BY rowid DESC LIMIT 1",
            )
            .get_result::<LatestAuditRow>(connection)
        })
        .await
        .map_err(|_| TestFailure::SessionAuditWasNotReadable)?
        .map_err(|_| TestFailure::SessionAuditWasNotReadable)?;
    Ok(AuditRow {
        actor: row.actor,
        action: row.action,
        resource_type: row.resource_type,
        resource_id: row
            .resource_id
            .ok_or(TestFailure::SessionAuditWasNotReadable)?,
        result: row.result,
        reason: row.reason.ok_or(TestFailure::SessionAuditWasNotReadable)?,
        detail: row.detail,
    })
}

async fn session_expiry(database: &Database) -> Result<String, TestFailure> {
    database
        .interact(|connection| {
            operator_sessions::table
                .select(operator_sessions::expires_at)
                .first(connection)
        })
        .await
        .map_err(|_| TestFailure::SessionRowWasNotReadable)?
        .map_err(|_| TestFailure::SessionRowWasNotReadable)
}

async fn session_count(database: &Database) -> Result<i64, TestFailure> {
    database
        .interact(|connection| operator_sessions::table.count().get_result(connection))
        .await
        .map_err(|_| TestFailure::SessionRowWasNotReadable)?
        .map_err(|_| TestFailure::SessionRowWasNotReadable)
}

async fn audit_count(database: &Database) -> Result<i64, TestFailure> {
    database
        .interact(|connection| audit_events::table.count().get_result(connection))
        .await
        .map_err(|_| TestFailure::SessionAuditWasNotReadable)?
        .map_err(|_| TestFailure::SessionAuditWasNotReadable)
}

async fn snapshot(
    database: &Database,
    observer: &mut SqliteConnection,
) -> Result<DatabaseSnapshot, TestFailure> {
    let data_version = diesel::dsl::sql::<BigInt>("PRAGMA data_version")
        .get_result(observer)
        .map_err(|_| TestFailure::ObserverDataVersionWasNotReadable)?;
    Ok(DatabaseSnapshot {
        sessions: session_count(database).await?,
        audits: audit_count(database).await?,
        data_version,
    })
}

fn correlation_id() -> CorrelationId {
    CorrelationId::from_uuid(Uuid::now_v7())
}

#[derive(QueryableByName)]
struct EncodedTextRow {
    #[diesel(sql_type = Text)]
    value: String,
}

#[derive(QueryableByName)]
struct SessionCreationRow {
    #[diesel(sql_type = Binary)]
    credential_hash: Vec<u8>,
    #[diesel(sql_type = Text)]
    operator_id: String,
    #[diesel(sql_type = Text)]
    expires_at: String,
    #[diesel(sql_type = BigInt)]
    ttl_valid: i64,
}

#[derive(QueryableByName)]
struct LatestAuditRow {
    #[diesel(sql_type = Text)]
    actor: String,
    #[diesel(sql_type = Text)]
    action: String,
    #[diesel(sql_type = Text)]
    resource_type: String,
    #[diesel(sql_type = diesel::sql_types::Nullable<Text>)]
    resource_id: Option<String>,
    #[diesel(sql_type = Text)]
    result: String,
    #[diesel(sql_type = diesel::sql_types::Nullable<Text>)]
    reason: Option<String>,
    #[diesel(sql_type = Text)]
    detail: String,
}

struct AuditRow {
    actor: String,
    action: String,
    resource_type: String,
    resource_id: String,
    result: String,
    reason: String,
    detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DatabaseSnapshot {
    sessions: i64,
    audits: i64,
    data_version: i64,
}

fn credentials(login_name: &str, password: &str) -> Result<OperatorCredentials, TestFailure> {
    OperatorCredentials::new(
        login_name.to_owned(),
        password.to_owned(),
        password.to_owned(),
    )
    .map_err(|_| TestFailure::ValidCredentialsWereRejected)
}

fn frozen_argon2() -> Result<Argon2<'static>, TestFailure> {
    let params = Params::new(19_456, 2, 1, None)
        .map_err(|_| TestFailure::FrozenArgon2ParametersWereRejected)?;
    Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, params))
}

async fn counts(database: &Database) -> Result<(i64, i64), TestFailure> {
    test_business_counts(database)
        .await
        .map_err(|_| TestFailure::BusinessCountsWereNotReadable)
}

async fn assert_database_excludes_canaries(
    database: &Database,
    canaries: &[&str],
) -> Result<(), TestFailure> {
    let canaries = canaries
        .iter()
        .map(|canary| (*canary).to_owned())
        .collect::<Vec<_>>();
    database
        .interact(move |connection| {
            let table_names = diesel::sql_query(
                "SELECT name AS value FROM pragma_table_list WHERE schema = 'main' \
                 AND name NOT LIKE 'sqlite_%' AND name <> 'operator_accounts' ORDER BY name",
            )
            .load::<EncodedTextRow>(connection)
            .map_err(|_| TestFailure::TableNamesWereNotReadable)?;
            for table_name in table_names {
                let columns = diesel::sql_query(
                    "SELECT name AS value FROM pragma_table_xinfo(?) ORDER BY cid",
                )
                .bind::<Text, _>(&table_name.value)
                .load::<EncodedTextRow>(connection)
                .map_err(|_| TestFailure::TableColumnsWereNotReadable)?;
                if columns.is_empty() {
                    continue;
                }
                let projection = columns
                    .iter()
                    .map(|column| format!("quote({})", quote_identifier(&column.value)))
                    .collect::<Vec<_>>()
                    .join(" || '|' || ");
                let query = format!(
                    "SELECT ({projection}) AS value FROM {}",
                    quote_identifier(&table_name.value)
                );
                let encoded_rows = diesel::sql_query(query)
                    .load::<EncodedTextRow>(connection)
                    .map_err(|_| TestFailure::TableContentsWereNotReadable)?;
                if encoded_rows
                    .iter()
                    .any(|row| canaries.iter().any(|canary| row.value.contains(canary)))
                {
                    return Err(TestFailure::FirstAdminCanaryEscaped);
                }
            }
            Ok(())
        })
        .await
        .map_err(|_| TestFailure::TableContentsWereNotReadable)?
}

fn quote_identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

struct TestDatabase {
    database: Database,
    path: PathBuf,
}

impl TestDatabase {
    async fn new() -> Result<Self, TestFailure> {
        let path = std::env::temp_dir().join(format!(
            "natsume-first-admin-test-{}.sqlite3",
            Uuid::now_v7()
        ));
        let database = Database::connect_and_migrate(&DatabaseConfig::new(&path, true))
            .await
            .map_err(|_| TestFailure::TestDatabaseCreationFailed)?;
        Ok(Self { database, path })
    }

    fn observer(&self) -> Result<SqliteConnection, TestFailure> {
        test_observer(&self.path).map_err(|_| TestFailure::ObserverConnectionFailed)
    }

    fn write_lock(&self) -> Result<SqliteConnection, TestFailure> {
        test_lock_database(&self.path).map_err(|_| TestFailure::ObserverConnectionFailed)
    }
}

impl Drop for TestDatabase {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        let _ = fs::remove_file(PathBuf::from(format!("{}-wal", self.path.display())));
        let _ = fs::remove_file(PathBuf::from(format!("{}-shm", self.path.display())));
    }
}

#[derive(Debug, Snafu)]
enum TestFailure {
    #[snafu(display("the first-admin test database could not be created"))]
    TestDatabaseCreationFailed,
    #[snafu(display("valid first-admin credentials were rejected"))]
    ValidCredentialsWereRejected,
    #[snafu(display("first-admin password hashing failed"))]
    PasswordHashingFailed,
    #[snafu(display("first-admin creation failed"))]
    FirstAdminCreationFailed,
    #[snafu(display("the first administrator ID was not UUIDv7"))]
    OperatorIdWasNotUuidV7,
    #[snafu(display("the operator account could not be read"))]
    OperatorAccountWasNotReadable,
    #[snafu(display("the persisted operator account was not exact"))]
    PersistedOperatorAccountWasNotExact,
    #[snafu(display("the persisted password PHC string was invalid"))]
    PersistedPhcWasInvalid,
    #[snafu(display("the persisted Argon2 profile was not frozen"))]
    PersistedArgon2ProfileWasNotFrozen,
    #[snafu(display("the frozen Argon2 parameters were rejected"))]
    FrozenArgon2ParametersWereRejected,
    #[snafu(display("the correct first-admin password was rejected"))]
    CorrectPasswordWasRejected,
    #[snafu(display("an incorrect first-admin password was accepted"))]
    IncorrectPasswordWasAccepted,
    #[snafu(display("the first-admin audit row could not be read"))]
    FirstAdminAuditWasNotReadable,
    #[snafu(display("the first-admin audit envelope was not exact"))]
    FirstAdminAuditWasNotExact,
    #[snafu(display("repeated first-admin creation succeeded"))]
    RepeatedFirstAdminCreationSucceeded,
    #[snafu(display("repeated first-admin creation returned the wrong typed error"))]
    RepeatedFirstAdminErrorWasNotTyped,
    #[snafu(display("repeated first-admin creation wrote business rows"))]
    RepeatedFirstAdminCreationWroteRows,
    #[snafu(display("a first-admin error exposed rejected context"))]
    FirstAdminErrorWasNotRedacted,
    #[snafu(display("the business row counts could not be read"))]
    BusinessCountsWereNotReadable,
    #[snafu(display("the audit fixture transaction could not begin"))]
    AuditFixtureTransactionFailed,
    #[snafu(display("the audit fixture row could not be inserted"))]
    AuditFixtureInsertFailed,
    #[snafu(display("the audit fixture transaction could not commit"))]
    AuditFixtureCommitFailed,
    #[snafu(display("a duplicate audit ID was committed"))]
    DuplicateAuditIdCommitted,
    #[snafu(display("the audit insert failure returned the wrong typed error"))]
    AuditInsertFailureWasNotTyped,
    #[snafu(display("the audit insert failure did not roll back the account"))]
    AuditInsertFailureDidNotRollBackAccount,
    #[snafu(display("a duplicate password-reset audit ID was committed"))]
    DuplicatePasswordResetAuditCommitted,
    #[snafu(display("the password-reset audit failure returned the wrong typed error"))]
    PasswordResetAuditFailureWasNotTyped,
    #[snafu(display("the password-reset audit failure did not roll back its mutations"))]
    PasswordResetAuditFailureDidNotRollBack,
    #[snafu(display("the application table names could not be read"))]
    TableNamesWereNotReadable,
    #[snafu(display("an application table's columns could not be read"))]
    TableColumnsWereNotReadable,
    #[snafu(display("an application table column name was invalid"))]
    TableColumnNameWasInvalid,
    #[snafu(display("an application table's contents could not be read"))]
    TableContentsWereNotReadable,
    #[snafu(display("a first-admin canary escaped into another table"))]
    FirstAdminCanaryEscaped,
    #[snafu(display("an operator account fixture could not be inserted"))]
    OperatorFixtureInsertFailed,
    #[snafu(display("an operator session could not be created"))]
    SessionCreationFailed,
    #[snafu(display("the operator session row could not be read"))]
    SessionRowWasNotReadable,
    #[snafu(display("the persisted operator session row was not exact"))]
    PersistedSessionRowWasNotExact,
    #[snafu(display("a session credential escaped into the database"))]
    SessionCredentialEscapedIntoDatabase,
    #[snafu(display("operator session evidence could not be read"))]
    SessionEvidenceWasNotReadable,
    #[snafu(display("the session-established audit was not exact"))]
    SessionEstablishedAuditWasNotExact,
    #[snafu(display("duplicate operator session creation succeeded"))]
    DuplicateSessionCreationSucceeded,
    #[snafu(display("an operator session persistence error exposed credential context"))]
    SessionStoreErrorWasNotRedacted,
    #[snafu(display("the operator-session observer connection could not be opened"))]
    ObserverConnectionFailed,
    #[snafu(display("the observer data version could not be read"))]
    ObserverDataVersionWasNotReadable,
    #[snafu(display("the operator session could not be read"))]
    SessionReadFailed,
    #[snafu(display("a live operator session was not active"))]
    LiveSessionWasNotActive,
    #[snafu(display("reading a live operator session changed state"))]
    LiveSessionReadChangedState,
    #[snafu(display("reading a live operator session waited for the write lock"))]
    LiveSessionReadTookTheWriteLock,
    #[snafu(display("the expired-session fixture could not be prepared"))]
    SessionExpiryFixtureFailed,
    #[snafu(display("a session with malformed expiry facts authenticated"))]
    MalformedExpiryAuthenticated,
    #[snafu(display("a malformed persisted expiry did not fail closed"))]
    MalformedExpiryDidNotFailClosed,
    #[snafu(display("a session at the exact expiry boundary authenticated"))]
    BoundaryExpiryAuthenticated,
    #[snafu(display("the exact expiry boundary was not expired"))]
    BoundaryExpiryWasNotExpired,
    #[snafu(display("an expired operator session authenticated"))]
    ExpiredSessionAuthenticated,
    #[snafu(display("the first expiry read was not exactly one transition"))]
    FirstExpiryReadWasNotOneTransition,
    #[snafu(display("the session-expired audit was not exact"))]
    SessionExpiredAuditWasNotExact,
    #[snafu(display("a second expired-session read wrote state"))]
    SecondExpiryReadWroteState,
    #[snafu(display("a failed expired-session cleanup was distinguishable"))]
    ExpiredCleanupFailureWasDistinguishable,
    #[snafu(display("a failed expired-session cleanup wrote state"))]
    ExpiredCleanupFailureWroteState,
    #[snafu(display("two operator store failures shared one cause"))]
    StoreCauseWasNotDistinct,
    #[snafu(display("an operator store cause was not a static discriminant"))]
    StoreCauseWasNotAStaticDiscriminant,
    #[snafu(display("the operator session could not be terminated"))]
    SessionTerminationFailed,
    #[snafu(display("live session termination was not exactly one transition"))]
    LiveTerminationWasNotOneTransition,
    #[snafu(display("the session-terminated audit was not exact"))]
    SessionTerminatedAuditWasNotExact,
    #[snafu(display("a terminated operator session authenticated"))]
    TerminatedSessionAuthenticated,
    #[snafu(display("an invalid operator session authenticated"))]
    InvalidSessionAuthenticated,
    #[snafu(display("session authentication failures were distinguishable"))]
    SessionAuthenticationErrorWasNotUnified,
    #[snafu(display("a repeat-safe session termination wrote state"))]
    TerminationNoopsWroteState,
    #[snafu(display("concurrent session termination was not exactly one transition"))]
    ConcurrentTerminationWasNotOneTransition,
    #[snafu(display("a duplicate operator-session audit was committed"))]
    DuplicateSessionAuditCommitted,
    #[snafu(display("session creation survived an audit failure"))]
    CreationAuditFailureDidNotRollBack,
    #[snafu(display("session termination survived an audit failure"))]
    TerminationAuditFailureDidNotRollBack,
    #[snafu(display("the operator-session audit could not be read"))]
    SessionAuditWasNotReadable,
}
