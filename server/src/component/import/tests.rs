use super::{
    ImportError,
    candidate::{CreatedImportCandidate, create_import_candidate},
    commit::commit_import,
};
use crate::{
    db::{Database, DatabaseConfig},
    vault,
};
use diesel::{
    QueryableByName, RunQueryDsl as _,
    connection::SimpleConnection as _,
    sql_types::{BigInt, Binary, Text},
};
use std::{
    fmt::Write as _,
    fs,
    io::{Cursor, Write as _},
    os::unix::fs::PermissionsExt as _,
    path::PathBuf,
    sync::Arc,
};
use uuid::Uuid;
use zip::{ZipArchive, ZipWriter, write::SimpleFileOptions};

fn checked<T, E: std::fmt::Debug>(value: Result<T, E>) -> T {
    value.unwrap_or_else(|error| panic!("test operation failed: {error:?}"))
}

fn row<'a>(school: &'a str, account: &'a str, seat: &'a str, password: &'a str) -> [&'a str; 9] {
    [
        school,
        "",
        "CHN",
        account,
        password,
        seat,
        "示例队",
        "Example team",
        "participant",
    ]
}

pub(crate) fn workbook(rows: &[[&str; 9]]) -> Vec<u8> {
    let headers = [
        "organization_zh",
        "organization_en",
        "country",
        "account",
        "password",
        "seat",
        "team_name_zh",
        "team_name_en",
        "category",
    ];
    let mut sheet = String::from(
        r#"<?xml version="1.0" encoding="UTF-8"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>"#,
    );
    for (index, values) in std::iter::once(&headers).chain(rows).enumerate() {
        let number = index + 1;
        checked(write!(sheet, r#"<row r="{number}">"#));
        for (column, value) in values.iter().enumerate() {
            let letter = char::from(b'A' + checked(u8::try_from(column)));
            let escaped = value
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;");
            checked(write!(
                sheet,
                r#"<c r="{letter}{number}" t="inlineStr"><is><t xml:space="preserve">{escaped}</t></is></c>"#
            ));
        }
        sheet.push_str("</row>");
    }
    sheet.push_str("</sheetData></worksheet>");
    let mut source = checked(ZipArchive::new(Cursor::new(include_bytes!(
        "../../../../crates/roster/examples/template.xlsx"
    ))));
    let mut output = ZipWriter::new(Cursor::new(Vec::new()));
    for index in 0..source.len() {
        let mut entry = checked(source.by_index(index));
        checked(output.start_file(entry.name(), SimpleFileOptions::default()));
        if entry.name() == "xl/worksheets/sheet1.xml" {
            checked(output.write_all(sheet.as_bytes()));
        } else {
            checked(std::io::copy(&mut entry, &mut output));
        }
    }
    checked(output.finish()).into_inner()
}

impl Fixture {
    async fn preview(&self, bytes: &[u8]) -> CreatedImportCandidate {
        checked(create_import_candidate(&self.database, Arc::clone(&self.vault), bytes).await)
    }
    async fn commit(
        &self,
        preview: &CreatedImportCandidate,
        bytes: &[u8],
    ) -> Result<(), ImportError> {
        commit_import(
            &self.database,
            Arc::clone(&self.vault),
            preview.candidate_id(),
            preview.preview_token_bytes(),
            bytes,
        )
        .await
    }
    async fn import(&self, bytes: &[u8]) {
        checked(self.commit(&self.preview(bytes).await, bytes).await);
    }
    async fn fingerprint(&self) -> [u8; 32] {
        checked(
            self.database
                .read(|tx| super::db::query::read_baseline(tx).map(|value| value.fingerprint()))
                .await,
        )
    }
    async fn execute(&self, sql: &'static str) {
        checked(
            self.database
                .write(move |tx| {
                    tx.connection()
                        .batch_execute(sql)
                        .map_err(|_| ImportError::PersistenceFailure)
                })
                .await,
        );
    }
}

#[tokio::test]
async fn invalid_workbook_is_rejected_before_persisting_a_preview_or_secret() {
    let fixture = Fixture::new().await;
    let bytes = workbook(&[
        row("大学", "team-1", "A-01", "first"),
        row("大学", "{invalid}", "A-02", "secret-canary"),
    ]);
    let error = create_import_candidate(&fixture.database, Arc::clone(&fixture.vault), &bytes)
        .await
        .err();
    assert_eq!(
        error,
        Some(ImportError::InvalidWorkbook(natsume_roster::InputError {
            row: Some(3),
            column: Some(4),
            kind: natsume_roster::InputErrorKind::InvalidIdentifier
        }))
    );
    assert!(
        checked(super::candidate::read_pending_import_candidate(&fixture.database).await).is_none()
    );
    assert_eq!(vault_record_count(&fixture.database).await, 0);
}

#[tokio::test]
async fn identical_and_metadata_only_imports_preserve_credentials_and_binding_targets() {
    let fixture = Fixture::new().await;
    let original = row("大学", "team-1", "A-01", "first-password-canary");
    let bytes = workbook(&[original]);
    let preview = fixture.preview(&bytes).await;
    assert!(
        !pending_preview_json(&fixture.database)
            .await
            .contains("first-password-canary")
    );
    assert_eq!(vault_record_count(&fixture.database).await, 0);
    checked(fixture.commit(&preview, &bytes).await);
    install_binding(&fixture.database).await;
    let before = fixture.fingerprint().await;
    let credentials = contest_evidence(&fixture.database).await;
    let same = fixture.preview(&bytes).await;
    assert!(!same.diff().has_changes());
    checked(fixture.commit(&same, &bytes).await);
    assert_eq!(before, fixture.fingerprint().await);
    assert_eq!(credentials, contest_evidence(&fixture.database).await);
    let mut renamed = original;
    renamed[1] = "University";
    renamed[6] = "新队名";
    renamed[8] = "star";
    let bytes = workbook(&[renamed]);
    let preview = fixture.preview(&bytes).await;
    assert_eq!(preview.diff().organization_changes.len(), 1);
    assert_eq!(preview.diff().team_changes.len(), 1);
    assert_eq!(preview.diff().affected_account_count, 1);
    assert_eq!(preview.diff().binding_impacts.len(), 1);
    assert!(!preview.diff().blocks_commit());
    assert!(preview.diff().passwords_changed.is_empty());
    checked(fixture.commit(&preview, &bytes).await);
    assert_eq!(credentials, contest_evidence(&fixture.database).await);
    assert_eq!(binding_and_seat_counts(&fixture.database).await, (1, 1));
    let target = checked(
        fixture
            .database
            .read(|tx| {
                diesel::sql_query("SELECT foreground_target AS value FROM device_session_targets")
                    .get_result::<TextValue>(tx.connection())
                    .map(|row| row.value)
                    .map_err(|_| ImportError::PersistenceFailure)
            })
            .await,
    );
    assert_eq!(target, "contest");
}

#[tokio::test]
async fn only_changed_passwords_advance_revisions_and_commit_checks_the_change_set() {
    let fixture = Fixture::new().await;
    let original = row("大学", "team-1", "A-01", "old-password-canary");
    fixture.import(&workbook(&[original])).await;
    let before = fixture.fingerprint().await;
    let mut changed = original;
    changed[4] = "new-password-canary";
    let bytes = workbook(&[changed]);
    let preview = fixture.preview(&bytes).await;
    assert_eq!(preview.diff().passwords_changed, ["team-1"]);
    assert!(preview.diff().team_changes.is_empty());
    assert!(preview.diff().organization_changes.is_empty());
    assert!(
        !pending_preview_json(&fixture.database)
            .await
            .contains("password-canary")
    );
    assert_eq!(
        fixture.commit(&preview, &workbook(&[original])).await,
        Err(ImportError::PreviewStale)
    );
    assert_eq!(before, fixture.fingerprint().await);
    checked(fixture.commit(&preview, &bytes).await);
    let evidence = contest_evidence(&fixture.database).await;
    assert_eq!(evidence.credential_revision, 2);
    assert_eq!(
        checked(fixture.vault.open(&evidence.nonce, &evidence.ciphertext)).as_slice(),
        b"new-password-canary"
    );
    fixture
        .execute("UPDATE accounts SET credential_revision = 9223372036854775807")
        .await;
    fixture.import(&bytes).await;
    assert_eq!(
        contest_evidence(&fixture.database)
            .await
            .credential_revision,
        i64::MAX
    );
}

#[tokio::test]
async fn password_rotation_updates_only_affected_accounts_in_a_complete_roster() {
    let fixture = Fixture::new().await;
    let a = row("大学", "team-a", "A-01", "one");
    let mut b = row("大学", "team-b", "A-02", "two");
    fixture.import(&workbook(&[a, b])).await;
    let before = checked(fixture.database.read(super::db::query::read_baseline).await);
    b[4] = "updated";
    let bytes = workbook(&[b, a]);
    let preview = fixture.preview(&bytes).await;
    assert_eq!(preview.diff().passwords_changed, ["team-b"]);
    assert_eq!(preview.diff().unchanged_count, 1);
    checked(fixture.commit(&preview, &bytes).await);
    let after = checked(fixture.database.read(super::db::query::read_baseline).await);
    for name in ["team-a", "team-b"] {
        let original = &before.accounts[name];
        let current = &after.accounts[name];
        assert_eq!(original.account_id(), current.account_id());
        if name == "team-a" {
            assert_eq!(
                original.credential_revision(),
                current.credential_revision()
            );
            assert_eq!(original.nonce, current.nonce);
            assert_eq!(original.ciphertext, current.ciphertext);
        } else {
            assert_eq!(
                current.credential_revision(),
                original.credential_revision() + 1
            );
            assert_ne!(original.nonce, current.nonce);
            assert_eq!(
                checked(fixture.vault.open(&current.nonce, &current.ciphertext)).as_slice(),
                b"updated"
            );
        }
    }
    assert_eq!(before.teams, after.teams);
    assert_eq!(before.organizations, after.organizations);
    for (code, seat) in &before.seats {
        assert_eq!(seat.seat_id(), after.seats[code].seat_id());
    }
}

#[tokio::test]
async fn organization_ids_are_sorted_stable_and_never_reused() {
    let fixture = Fixture::new().await;
    let a = row("A大学", "team-a", "A-01", "one");
    let b = row("B大学", "team-b", "A-02", "two");
    let bytes = workbook(&[b, a]);
    let first = fixture.preview(&bytes).await;
    let schools = &first.diff().organizations;
    assert_eq!(
        (schools[0].name_zh.as_str(), schools[0].organization_id),
        ("A大学", 1)
    );
    assert_eq!(
        (schools[1].name_zh.as_str(), schools[1].organization_id),
        ("B大学", 2)
    );
    checked(fixture.commit(&first, &bytes).await);
    let before = fixture.fingerprint().await;
    fixture.import(&workbook(&[a, b])).await;
    assert_eq!(before, fixture.fingerprint().await);
    fixture.import(&workbook(&[a])).await;
    let third = row("0大学", "team-c", "A-03", "three");
    let next = fixture.preview(&workbook(&[third, a])).await;
    assert_eq!(
        (
            next.diff().organizations[0].name_zh.as_str(),
            next.diff().organizations[0].organization_id
        ),
        ("0大学", 3)
    );
    assert_eq!(next.diff().organizations[1].organization_id, 1);
    checked(fixture.commit(&next, &workbook(&[third, a])).await);
    fixture
        .execute("UPDATE sqlite_sequence SET seq = 999 WHERE name = 'organizations'")
        .await;
    let more = fixture.preview(&workbook(&[third, a, b])).await;
    assert_eq!(more.diff().organizations[2].organization_id, 1000);
}

#[tokio::test]
async fn occupied_seat_removal_is_blocked_but_swaps_preserve_the_binding() {
    let fixture = Fixture::new().await;
    let a = row("大学", "team-a", "A-01", "one");
    let b = row("大学", "team-b", "A-02", "two");
    fixture.import(&workbook(&[a, b])).await;
    install_binding(&fixture.database).await;
    let before = fixture.fingerprint().await;
    let removal = workbook(&[b]);
    let preview = fixture.preview(&removal).await;
    assert!(preview.diff().blocks_commit());
    assert_eq!(
        fixture.commit(&preview, &removal).await,
        Err(ImportError::SeatOccupied)
    );
    assert_eq!(before, fixture.fingerprint().await);
    checked(super::candidate::discard_import(&fixture.database, preview.candidate_id()).await);
    let mut a = a;
    let mut b = b;
    a[5] = "A-02";
    b[5] = "A-01";
    let bytes = workbook(&[a, b]);
    let swap = fixture.preview(&bytes).await;
    assert_eq!(swap.diff().mappings_changed.len(), 2);
    assert_eq!(swap.diff().binding_impacts.len(), 1);
    assert!(!swap.diff().blocks_commit());
    checked(fixture.commit(&swap, &bytes).await);
    assert_eq!(binding_and_seat_counts(&fixture.database).await, (1, 2));
    assert!(!fixture.preview(&bytes).await.diff().has_changes());
}

#[tokio::test]
async fn concurrent_baseline_changes_and_changed_uploads_require_a_new_preview() {
    let fixture = Fixture::new().await;
    let original = row("大学", "team-1", "A-01", "one");
    let bytes = workbook(&[original]);
    fixture.import(&bytes).await;
    let preview = fixture.preview(&bytes).await;
    let mut renamed = original;
    renamed[6] = "Different";
    assert_eq!(
        fixture.commit(&preview, &workbook(&[renamed])).await,
        Err(ImportError::PreviewStale)
    );
    install_binding(&fixture.database).await;
    assert_eq!(
        fixture.commit(&preview, &bytes).await,
        Err(ImportError::PreviewStale)
    );
}

#[tokio::test]
async fn transaction_failure_rolls_back_roster_ids_credentials_and_candidate_consumption() {
    let fixture = Fixture::new().await;
    let bytes = workbook(&[row("大学", "team-1", "A-01", "one")]);
    let preview = fixture.preview(&bytes).await;
    let before = fixture.fingerprint().await;
    fixture.execute("CREATE TRIGGER reject_team BEFORE INSERT ON teams BEGIN SELECT RAISE(ABORT, 'test failure'); END").await;
    assert_eq!(
        fixture.commit(&preview, &bytes).await,
        Err(ImportError::PersistenceFailure)
    );
    assert_eq!(before, fixture.fingerprint().await);
    assert_eq!(vault_record_count(&fixture.database).await, 0);
    assert!(
        checked(super::candidate::read_pending_import_candidate(&fixture.database).await).is_some()
    );
    fixture.execute("DROP TRIGGER reject_team").await;
    checked(fixture.commit(&preview, &bytes).await);
    assert!(
        checked(super::candidate::read_pending_import_candidate(&fixture.database).await).is_none()
    );
}

#[tokio::test]
async fn full_roster_fills_legacy_metadata_without_replacing_the_account_or_vault() {
    let fixture = Fixture::new().await;
    let bytes = workbook(&[row("大学", "team-1", "A-01", "one")]);
    fixture.import(&bytes).await;
    install_binding(&fixture.database).await;
    let before = contest_evidence(&fixture.database).await;
    fixture
        .execute("DELETE FROM teams; DELETE FROM organizations")
        .await;
    let preview = fixture.preview(&bytes).await;
    assert_eq!(preview.diff().team_changes.len(), 1);
    assert!(preview.diff().passwords_changed.is_empty());
    checked(fixture.commit(&preview, &bytes).await);
    assert_eq!(before, contest_evidence(&fixture.database).await);
    assert_eq!(binding_and_seat_counts(&fixture.database).await, (1, 1));
}

#[derive(Debug, PartialEq, Eq, QueryableByName)]
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
                    "INSERT INTO devices (device_id, machine_hardware_id, evidence_quality, state, created_at_unix_ms) VALUES \
                     ('{device_id}', '550e8400-e29b-51d4-a716-446655440000', \
                      'strong', 'enabled', 1); \
                     INSERT INTO device_bindings VALUES \
                     ('{binding_id}', '{device_id}', '{seat_id}'); \
                     INSERT INTO device_session_targets VALUES ('{device_id}', 'contest', NULL);"
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
    vault: Arc<vault::VaultSession>,
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
        let vault = vault::load(&master_key)
            .unwrap_or_else(|_| panic!("fixture vault key could not be loaded"));
        let database =
            Database::connect_and_migrate(&DatabaseConfig::new(root.join("server.sqlite3"), true))
                .await
                .unwrap_or_else(|_| panic!("fixture database could not be created"));
        Self {
            database,
            root,
            vault: Arc::new(vault),
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
