use std::{fs, os::unix::fs::PermissionsExt as _, path::PathBuf};

use diesel::{
    QueryableByName, RunQueryDsl as _,
    connection::SimpleConnection as _,
    sql_types::{BigInt, Binary, Text},
};
use uuid::Uuid;

use crate::{
    audit::CorrelationId,
    db::{Database, DatabaseConfig},
    vault,
};

use super::{ImportError, commit_import, create_import_candidate};

#[derive(QueryableByName)]
struct ContestEvidence {
    #[diesel(sql_type = BigInt)]
    seats: i64,
    #[diesel(sql_type = BigInt)]
    accounts: i64,
    #[diesel(sql_type = BigInt)]
    vault_records: i64,
    #[diesel(sql_type = BigInt)]
    credential_revision: i64,
    #[diesel(sql_type = Binary)]
    nonce: Vec<u8>,
    #[diesel(sql_type = Binary)]
    ciphertext: Vec<u8>,
}

#[tokio::test]
async fn preview_is_non_secret_and_each_commit_replaces_the_current_credential() {
    let fixture = Fixture::new().await;
    let first_csv = b"seat,account,password\nA-01,team-alpha,first-password-canary";
    let first = create_import_candidate(
        &fixture.database,
        first_csv,
        CorrelationId::from_uuid(Uuid::now_v7()),
    )
    .await
    .unwrap_or_else(|error| panic!("first preview failed: {error}"));
    let preview_json = pending_preview_json(&fixture.database).await;
    assert!(!preview_json.contains("first-password-canary"));
    assert_eq!(vault_record_count(&fixture.database).await, 0);

    commit_import(
        &fixture.database,
        &fixture.master_key,
        first.candidate_id(),
        first.preview_token(),
        first_csv,
        CorrelationId::from_uuid(Uuid::now_v7()),
    )
    .await
    .unwrap_or_else(|error| panic!("first commit failed: {error}"));
    let first_evidence = contest_evidence(&fixture.database).await;
    assert_eq!(
        (
            first_evidence.seats,
            first_evidence.accounts,
            first_evidence.vault_records
        ),
        (1, 1, 1)
    );
    assert_eq!(first_evidence.credential_revision, 1);
    assert_eq!(
        vault::open(
            &fixture.master_key,
            &first_evidence.nonce,
            &first_evidence.ciphertext,
        )
        .unwrap_or_else(|_| panic!("first credential could not be opened"))
        .as_slice(),
        b"first-password-canary"
    );

    let second_csv = b"seat,account,password\nA-01,team-alpha,second-password-canary";
    let second = create_import_candidate(
        &fixture.database,
        second_csv,
        CorrelationId::from_uuid(Uuid::now_v7()),
    )
    .await
    .unwrap_or_else(|error| panic!("second preview failed: {error}"));
    commit_import(
        &fixture.database,
        &fixture.master_key,
        second.candidate_id(),
        second.preview_token(),
        second_csv,
        CorrelationId::from_uuid(Uuid::now_v7()),
    )
    .await
    .unwrap_or_else(|error| panic!("second commit failed: {error}"));
    let second_evidence = contest_evidence(&fixture.database).await;
    assert_eq!(second_evidence.credential_revision, 2);
    assert_ne!(second_evidence.nonce, first_evidence.nonce);
    assert_ne!(second_evidence.ciphertext, first_evidence.ciphertext);
    assert_eq!(
        vault::open(
            &fixture.master_key,
            &second_evidence.nonce,
            &second_evidence.ciphertext,
        )
        .unwrap_or_else(|_| panic!("second credential could not be opened"))
        .as_slice(),
        b"second-password-canary"
    );
}

#[tokio::test]
async fn commit_rejects_removing_a_seat_owned_by_binding() {
    let fixture = Fixture::new().await;
    let initial_csv = b"seat,account,password\nA-01,team-alpha,password-a";
    let initial = create_import_candidate(
        &fixture.database,
        initial_csv,
        CorrelationId::from_uuid(Uuid::now_v7()),
    )
    .await
    .unwrap_or_else(|error| panic!("initial preview failed: {error}"));
    commit_import(
        &fixture.database,
        &fixture.master_key,
        initial.candidate_id(),
        initial.preview_token(),
        initial_csv,
        CorrelationId::from_uuid(Uuid::now_v7()),
    )
    .await
    .unwrap_or_else(|error| panic!("initial commit failed: {error}"));
    install_binding(&fixture.database).await;

    let replacement_csv = b"seat,account,password\nB-02,team-beta,password-b";
    let replacement = create_import_candidate(
        &fixture.database,
        replacement_csv,
        CorrelationId::from_uuid(Uuid::now_v7()),
    )
    .await
    .unwrap_or_else(|error| panic!("replacement preview failed: {error}"));
    assert_eq!(replacement.diff().binding_impacts().len(), 1);
    let result = commit_import(
        &fixture.database,
        &fixture.master_key,
        replacement.candidate_id(),
        replacement.preview_token(),
        replacement_csv,
        CorrelationId::from_uuid(Uuid::now_v7()),
    )
    .await;
    assert_eq!(result, Err(ImportError::SeatOccupied));
    assert_eq!(binding_and_seat_counts(&fixture.database).await, (1, 1));
}

async fn pending_preview_json(database: &Database) -> String {
    database
        .read(|transaction| {
            diesel::sql_query(
                "SELECT redacted_preview_json AS value FROM pending_import_candidate WHERE singleton = 1",
            )
            .get_result::<TextValue>(transaction.connection())
            .map(|row| row.value)
            .map_err(|_| ImportError::PersistenceFailure)
        })
        .await
        .unwrap_or_else(|_| panic!("pending preview could not be read"))
}

async fn vault_record_count(database: &Database) -> i64 {
    database
        .read(|transaction| {
            diesel::sql_query("SELECT COUNT(*) AS value FROM server_vault_records")
                .get_result::<IntegerValue>(transaction.connection())
                .map(|row| row.value)
                .map_err(|_| ImportError::PersistenceFailure)
        })
        .await
        .unwrap_or_else(|_| panic!("vault record count could not be read"))
}

async fn contest_evidence(database: &Database) -> ContestEvidence {
    database
        .read(|transaction| {
            diesel::sql_query(
                "SELECT (SELECT COUNT(*) FROM seats) AS seats, \
                 (SELECT COUNT(*) FROM accounts) AS accounts, \
                 (SELECT COUNT(*) FROM server_vault_records) AS vault_records, \
                 a.credential_revision, v.nonce, v.ciphertext \
                 FROM accounts a JOIN server_vault_records v ON v.account_id = a.account_id",
            )
            .get_result::<ContestEvidence>(transaction.connection())
            .map_err(|_| ImportError::PersistenceFailure)
        })
        .await
        .unwrap_or_else(|_| panic!("contest evidence could not be read"))
}

async fn install_binding(database: &Database) {
    database
        .write(|transaction| {
            let seat_id =
                diesel::sql_query("SELECT seat_id AS value FROM seats WHERE seat_code = 'A-01'")
                    .get_result::<TextValue>(transaction.connection())
                    .map_err(|_| ImportError::PersistenceFailure)?
                    .value;
            let device_id = Uuid::now_v7();
            let binding_id = Uuid::now_v7();
            transaction
                .connection()
                .batch_execute(&format!(
                    "INSERT INTO devices VALUES \
                     ('{device_id}', '550e8400-e29b-51d4-a716-446655440000', \
                      'strong', 'enabled', 1); \
                     INSERT INTO device_bindings VALUES \
                     ('{binding_id}', '{device_id}', '{seat_id}');"
                ))
                .map_err(|_| ImportError::PersistenceFailure)
        })
        .await
        .unwrap_or_else(|_| panic!("binding fixture could not be installed"));
}

async fn binding_and_seat_counts(database: &Database) -> (i64, i64) {
    database
        .read(|transaction| {
            #[derive(QueryableByName)]
            struct Counts {
                #[diesel(sql_type = BigInt)]
                bindings: i64,
                #[diesel(sql_type = BigInt)]
                seats: i64,
            }
            diesel::sql_query(
                "SELECT (SELECT COUNT(*) FROM device_bindings) AS bindings, \
                 (SELECT COUNT(*) FROM seats) AS seats",
            )
            .get_result::<Counts>(transaction.connection())
            .map(|row| (row.bindings, row.seats))
            .map_err(|_| ImportError::PersistenceFailure)
        })
        .await
        .unwrap_or_else(|_| panic!("binding evidence could not be read"))
}

#[derive(QueryableByName)]
struct IntegerValue {
    #[diesel(sql_type = BigInt)]
    value: i64,
}

#[derive(QueryableByName)]
struct TextValue {
    #[diesel(sql_type = Text)]
    value: String,
}

struct Fixture {
    database: Database,
    root: PathBuf,
    master_key: PathBuf,
}

impl Fixture {
    async fn new() -> Self {
        let root = std::env::temp_dir().join(format!("natsume-import-{}", Uuid::now_v7()));
        fs::create_dir(&root).unwrap_or_else(|_| panic!("fixture directory could not be created"));
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
            .unwrap_or_else(|_| panic!("fixture directory permissions could not be set"));
        let master_key = root.join("master.key");
        vault::ensure_master_key(&master_key)
            .unwrap_or_else(|_| panic!("fixture vault key could not be created"));
        let database =
            Database::connect_and_migrate(&DatabaseConfig::new(root.join("server.sqlite3"), true))
                .await
                .unwrap_or_else(|_| panic!("fixture database could not be created"));
        Self {
            database,
            root,
            master_key,
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
