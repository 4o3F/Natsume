use diesel::{ExpressionMethods, QueryDsl, RunQueryDsl};
use sha2::{Digest as _, Sha256};

use super::{Fixture, PrintComponent, PrintError};
use crate::{
    component::print::{db, pairing::PairingLimits},
    diesel_schema::print_printers,
};
use std::{
    net::IpAddr,
    time::{Duration, Instant},
};

fn must<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
    result.unwrap_or_else(|error| panic!("print pairing fixture: {error:?}"))
}

async fn printer(f: &Fixture) -> String {
    let id = uuid::Uuid::now_v7().to_string();
    let saved = id.clone();
    must(
        f.database
            .write(move |tx| db::printers::insert(tx, &saved, "现场打印机"))
            .await,
    );
    id
}

async fn issue(f: &Fixture, id: &str, code: &str, now: i64) {
    let id = id.to_owned();
    let hash: [u8; 32] = sha2::Sha256::digest(code.as_bytes()).into();
    must(
        f.database
            .write(move |tx| db::printers::issue_code(tx, &id, &hash, now))
            .await,
    );
}

#[tokio::test]
async fn pairing_consumes_code_requires_confirmation_and_never_reveals_secrets_in_list() {
    let f = Fixture::new().await;
    let id = printer(&f).await;
    issue(&f, &id, "ABCD2345", i64::MAX - 600_000).await;
    assert!(matches!(
        f.print.pair_client("ABCD2346").await,
        Err(PrintError::Authentication)
    ));
    let issued = must(f.print.pair_client("abcd-2345").await);
    assert_eq!(issued.printer_id, id);
    assert!(
        issued
            .token
            .starts_with(&format!("npc1.{}.", issued.client_id))
    );
    assert!(matches!(
        f.print.pair_client("ABCD2345").await,
        Err(PrintError::Authentication)
    ));
    let rows = must(f.database.read(db::printers::list).await);
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].client_id.as_deref(),
        Some(issued.client_id.as_str())
    );
    assert!(!rows[0].client_confirmed);
    assert!(rows[0].pairing_expires_at_unix_ms.is_none());
    let metadata = must(serde_json::to_string(&rows));
    assert!(!metadata.contains("token"));
    assert!(!metadata.contains("hash"));
    assert!(!metadata.contains("ABCD"));
    let confirmed = must(f.print.confirm_client(&issued.token).await);
    assert_eq!(
        confirmed.client_id.as_deref(),
        Some(issued.client_id.as_str())
    );
    assert!(confirmed.client_confirmed);
    // A repeated confirmation cannot create another identity or reveal the token again.
    assert!(must(f.print.confirm_client(&issued.token).await).client_confirmed);
    assert!(matches!(
        f.print.confirm_client(&format!("{}0", issued.token)).await,
        Err(PrintError::Authentication)
    ));
    assert!(matches!(
        f.print.confirm_client("nps1.invalid").await,
        Err(PrintError::Authentication)
    ));
}

#[tokio::test]
async fn reissue_invalidates_old_code_without_displacing_a_binding() {
    let f = Fixture::new().await;
    let id = printer(&f).await;
    issue(&f, &id, "ABCD2345", i64::MAX - 600_000).await;
    issue(&f, &id, "EFGH6789", i64::MAX - 600_000).await;
    assert!(matches!(
        f.print.pair_client("ABCD2345").await,
        Err(PrintError::Authentication)
    ));
    let issued = must(f.print.pair_client("EFGH6789").await);
    let other_hash: [u8; 32] = sha2::Sha256::digest(b"JKLM2345").into();
    let saved = id.clone();
    let refusal = f
        .database
        .write(move |tx| db::printers::issue_code(tx, &saved, &other_hash, 1))
        .await;
    assert!(matches!(
        refusal,
        Err(crate::db::TransactionError::Operation(PrintError::Conflict))
    ));
    must(f.print.confirm_client(&issued.token).await);
    let saved = id.clone();
    let refusal = f
        .database
        .write(move |tx| db::printers::issue_code(tx, &saved, &other_hash, 1))
        .await;
    assert!(matches!(
        refusal,
        Err(crate::db::TransactionError::Operation(PrintError::Conflict))
    ));
    must(
        f.database
            .write(move |tx| db::printers::clear_binding(tx, &id))
            .await,
    );
    assert!(matches!(
        f.print.confirm_client(&issued.token).await,
        Err(PrintError::Authentication)
    ));
}

#[tokio::test]
async fn code_expiry_is_exclusive_at_ten_minutes() {
    let f = Fixture::new().await;
    let id = printer(&f).await;
    issue(&f, &id, "ABCD2345", 1_000).await;
    let hash: [u8; 32] = sha2::Sha256::digest(b"ABCD2345").into();
    let client = uuid::Uuid::now_v7().to_string();
    let refused = f
        .database
        .write(move |tx| db::printers::pair(tx, &hash, &client, &[0; 32], 601_000))
        .await;
    assert!(matches!(
        refused,
        Err(crate::db::TransactionError::Operation(
            PrintError::Authentication
        ))
    ));
    let client = uuid::Uuid::now_v7().to_string();
    let accepted = must(
        f.database
            .write(move |tx| db::printers::pair(tx, &hash, &client, &[0; 32], 600_999))
            .await,
    );
    assert_eq!(accepted, id);
}

#[tokio::test]
async fn concurrent_pairing_cannot_consume_the_same_code_twice() {
    let f = Fixture::new().await;
    let id = printer(&f).await;
    issue(&f, &id, "ABCD2345", i64::MAX - 600_000).await;
    let (first, second) = tokio::join!(
        f.print.pair_client("ABCD2345"),
        f.print.pair_client("abcd-2345")
    );
    assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
}

#[tokio::test]
async fn incomplete_pairing_survives_reload_and_needs_explicit_cleanup() {
    let f = Fixture::new().await;
    let id = printer(&f).await;
    issue(&f, &id, "ABCD2345", i64::MAX - 600_000).await;
    let issued = must(f.print.pair_client("ABCD2345").await);
    let reloaded = must(PrintComponent::load(f.database.clone(), f.documents_path()).await);
    assert!(matches!(
        reloaded.pair_client("ABCD2345").await,
        Err(PrintError::Authentication)
    ));
    let rows = must(f.database.read(db::printers::list).await);
    assert_eq!(
        rows[0].client_id.as_deref(),
        Some(issued.client_id.as_str())
    );
    assert!(!rows[0].client_confirmed);
    must(
        f.database
            .write(move |tx| db::printers::clear_binding(tx, &id))
            .await,
    );
    assert!(matches!(
        reloaded.confirm_client(&issued.token).await,
        Err(PrintError::Authentication)
    ));
}

#[tokio::test]
async fn credentials_are_stored_only_as_hashes_and_confirmation_survives_restart() {
    let f = Fixture::new().await;
    let id = printer(&f).await;
    issue(&f, &id, "ABCD2345", i64::MAX - 600_000).await;
    let saved = id.clone();
    let stored_code = must(
        f.database
            .read(move |tx| {
                print_printers::table
                    .select(print_printers::pairing_code_hash)
                    .filter(print_printers::printer_id.eq(saved))
                    .first::<Option<Vec<u8>>>(tx.connection())
                    .map_err(PrintError::from)
            })
            .await,
    );
    assert_eq!(stored_code, Some(Sha256::digest(b"ABCD2345").to_vec()));
    let issued = must(f.print.pair_client("ABCD2345").await);
    let stored = must(
        f.database
            .read(move |tx| {
                print_printers::table
                    .select((
                        print_printers::pairing_code_hash,
                        print_printers::client_token_hash,
                    ))
                    .filter(print_printers::printer_id.eq(id))
                    .first::<(Option<Vec<u8>>, Option<Vec<u8>>)>(tx.connection())
                    .map_err(PrintError::from)
            })
            .await,
    );
    assert!(stored.0.is_none());
    assert_eq!(
        stored.1,
        Some(Sha256::digest(issued.token.as_bytes()).to_vec())
    );
    must(f.print.confirm_client(&issued.token).await);
    let reloaded = must(PrintComponent::load(f.database.clone(), f.documents_path()).await);
    assert!(must(reloaded.confirm_client(&issued.token).await).client_confirmed);
}

#[tokio::test]
async fn transaction_rollback_does_not_consume_a_code_or_leave_a_binding() {
    let f = Fixture::new().await;
    let id = printer(&f).await;
    issue(&f, &id, "ABCD2345", 1_000).await;
    let hash: [u8; 32] = Sha256::digest(b"ABCD2345").into();
    let result = f
        .database
        .write(move |tx| {
            db::printers::pair(
                tx,
                &hash,
                &uuid::Uuid::now_v7().to_string(),
                &[0; 32],
                1_000,
            )?;
            Err::<(), _>(PrintError::Conflict)
        })
        .await;
    assert!(result.is_err());
    let accepted = must(
        f.database
            .write(move |tx| {
                db::printers::pair(
                    tx,
                    &hash,
                    &uuid::Uuid::now_v7().to_string(),
                    &[1; 32],
                    1_001,
                )
            })
            .await,
    );
    assert_eq!(accepted, id);
}

#[tokio::test]
async fn print_migration_rollback_preserves_roster_and_document_files() {
    use crate::diesel_schema::accounts;
    use diesel::Connection as _;
    use diesel_migrations::{FileBasedMigrations, MigrationHarness as _};

    let f = Fixture::new().await;
    let request = f.register().await;
    let ready = must(
        f.print
            .upload(f.source.clone(), request.request_id.clone(), super::pdf(1))
            .await,
    );
    let path = f.path.to_str().unwrap_or_else(|| panic!("fixture path"));
    let mut connection = must(diesel::sqlite::SqliteConnection::establish(path));
    let migrations = || {
        must(FileBasedMigrations::from_path(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("diesel/business/migrations"),
        ))
    };
    // Rolling back the Print migrations removes their database records,
    // not roster facts or files owned by the document directory.
    // Photo initialization is later in the sequence and must be reverted first.
    must(connection.revert_last_migration(migrations()));
    must(connection.revert_last_migration(migrations()));
    must(connection.revert_last_migration(migrations()));
    assert_eq!(
        must(accounts::table.count().get_result::<i64>(&mut connection)),
        1
    );
    let document = ready
        .document
        .as_ref()
        .unwrap_or_else(|| panic!("document"));
    must(f.print.documents.verify(&document.sha256, document.bytes));
    assert_eq!(
        must(connection.run_pending_migrations(migrations())).len(),
        3
    );
    assert!(must(f.database.read(db::printers::list).await).is_empty());
    assert!(must(f.print.list_sources().await).is_empty());
    assert!(must(f.print.list_executions().await).is_empty());
}

#[test]
fn source_limit_blocks_for_sixty_seconds_without_blocking_other_sources() {
    let mut limits = PairingLimits::default();
    let now = Instant::now();
    let first = IpAddr::from([192, 0, 2, 1]);
    let second = IpAddr::from([192, 0, 2, 2]);
    for _ in 0..5 {
        assert!(limits.check(first, now).is_ok());
    }
    assert!(matches!(
        limits.check(first, now + Duration::from_secs(30)),
        Err(PrintError::RateLimited)
    ));
    assert!(limits.check(second, now + Duration::from_secs(30)).is_ok());
    assert!(matches!(
        limits.check(first, now + Duration::from_secs(89)),
        Err(PrintError::RateLimited)
    ));
    assert!(limits.check(first, now + Duration::from_secs(90)).is_ok());
}

#[test]
fn ipv4_mapped_ipv6_cannot_bypass_the_source_limit() {
    let mut limits = PairingLimits::default();
    let now = Instant::now();
    let ipv4 = std::net::Ipv4Addr::new(192, 0, 2, 1);
    for _ in 0..5 {
        assert!(limits.check(IpAddr::V4(ipv4), now).is_ok());
    }
    assert!(matches!(
        limits.check(IpAddr::V6(ipv4.to_ipv6_mapped()), now),
        Err(PrintError::RateLimited)
    ));
}

#[test]
fn global_limit_is_shared_and_has_a_sixty_second_cooldown() {
    let mut limits = PairingLimits::default();
    let now = Instant::now();
    for source in 1..=12 {
        for _ in 0..5 {
            assert!(limits.check(IpAddr::from([192, 0, 2, source]), now).is_ok());
        }
    }
    let next = IpAddr::from([192, 0, 2, 13]);
    assert!(matches!(
        limits.check(next, now + Duration::from_secs(10)),
        Err(PrintError::RateLimited)
    ));
    assert!(matches!(
        limits.check(next, now + Duration::from_secs(69)),
        Err(PrintError::RateLimited)
    ));
    assert!(limits.check(next, now + Duration::from_secs(70)).is_ok());
}
