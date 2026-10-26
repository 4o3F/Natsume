use diesel::{
    Connection as _, RunQueryDsl as _, connection::SimpleConnection as _, dsl::sql,
    sql_types::BigInt, sqlite::SqliteConnection,
};
use diesel_migrations::{FileBasedMigrations, MigrationHarness as _};

use super::{Fixture, claim, client, job, must, ready, report, store, write};
use crate::component::print::{
    ClientIdentity, PrintError,
    execution::{AttemptOutcome, AttemptReport, AttemptState, PrintAttempt, PrintRequeue},
};
use uuid::Uuid;

async fn exhaust(f: &Fixture, c: &ClientIdentity, now: i64) -> Vec<(String, PrintAttempt)> {
    let mut attempts = Vec::new();
    for number in 0..3 {
        let request = Uuid::now_v7().to_string();
        let time = now + number * 30_001;
        let attempt = claim(f, c, &request, time)
            .await
            .unwrap_or_else(|| panic!("claim"));
        assert_eq!(i64::from(attempt.attempt_number), number + 1);
        let ended = must(report(f, c, &attempt, AttemptReport::Unknown, time + 1).await);
        attempts.push((request, ended));
    }
    attempts
}

#[tokio::test]
async fn requeue_starts_a_new_three_attempt_round_without_replacing_history_or_admission() {
    let f = Fixture::new().await;
    let c = client(&f).await;
    let sequence = ready(&f).await;
    let now = must(write(&f, super::super::super::db::printers::now).await);
    let old = exhaust(&f, &c, now).await;
    let admission = must(serde_json::to_value(must(f.print.list_queue().await)));
    must(
        f.print
            .requeue(
                sequence,
                PrintRequeue {
                    expected_requeue_count: 0,
                },
            )
            .await,
    );
    let requeued = must(f.print.list_executions().await);
    assert_eq!(requeued[0].state.as_str(), "queued");
    assert_eq!(requeued[0].attempts_used, 0);
    assert_eq!(requeued[0].requeue_count, 1);
    assert_eq!(requeued[0].last_outcome, Some(AttemptOutcome::Unknown));
    let new = exhaust(&f, &c, now + 120_000).await;
    let exhausted = must(f.print.list_executions().await);
    assert_eq!(exhausted[0].state.as_str(), "exhausted");
    assert_eq!(exhausted[0].attempts_used, 3);
    assert_eq!(exhausted[0].requeue_count, 1);
    for ((request, before), (_, after)) in old.iter().zip(&new) {
        assert_ne!(before.attempt_id, after.attempt_id);
        assert_eq!(before.ready_sequence, after.ready_sequence);
        assert_eq!(before.document_sha256, after.document_sha256);
        assert_eq!(
            claim(&f, &c, request, now + 240_000).await.as_ref(),
            Some(before)
        );
        assert_eq!(
            must(
                f.print
                    .get_attempt(c.clone(), before.attempt_id.clone())
                    .await
            ),
            *before
        );
        assert_eq!(
            must(report(&f, &c, before, AttemptReport::Failed, now + 240_000).await),
            *before
        );
    }
    assert_eq!(
        must(serde_json::to_value(must(f.print.list_queue().await))),
        admission
    );
    assert_eq!(
        f.print
            .requeue(
                sequence,
                PrintRequeue {
                    expected_requeue_count: 0
                }
            )
            .await,
        Err(PrintError::Conflict)
    );
    must(
        f.print
            .requeue(
                sequence,
                PrintRequeue {
                    expected_requeue_count: 1,
                },
            )
            .await,
    );
    let reloaded = must(
        crate::component::print::PrintComponent::load(
            must(
                crate::db::Database::connect_and_migrate(&crate::db::DatabaseConfig::new(
                    &f.path, false,
                ))
                .await,
            ),
            f.documents_path(),
        )
        .await,
    );
    let saved = must(reloaded.list_executions().await);
    assert_eq!(saved[0].requeue_count, 2);
    assert_eq!(saved[0].state.as_str(), "queued");
}

#[tokio::test]
async fn requeue_only_accepts_an_exhausted_current_round() {
    let f = Fixture::new().await;
    let c = client(&f).await;
    let sequence = ready(&f).await;
    let now = must(write(&f, super::super::super::db::printers::now).await);
    for state in ["queued", "active", "waiting", "completed"] {
        let current = must(f.print.list_executions().await);
        assert_eq!(current[0].state.as_str(), state);
        assert_eq!(
            f.print
                .requeue(
                    sequence,
                    PrintRequeue {
                        expected_requeue_count: 0
                    }
                )
                .await,
            Err(PrintError::Conflict)
        );
        if state == "queued" {
            assert!(
                claim(&f, &c, &Uuid::now_v7().to_string(), now)
                    .await
                    .is_some()
            );
        } else if state == "active" {
            let attempt =
                must(f.print.current_attempt(c.clone()).await).unwrap_or_else(|| panic!("active"));
            must(report(&f, &c, &attempt, AttemptReport::Failed, now + 1).await);
        } else if state == "waiting" {
            let attempt = claim(&f, &c, &Uuid::now_v7().to_string(), now + 30_002)
                .await
                .unwrap_or_else(|| panic!("retry"));
            let owner = c.clone();
            let id = attempt.attempt_id.clone();
            let identity = job(&attempt);
            must(
                write(&f, move |tx| {
                    store::register_job(tx, &owner, &id, &identity, now + 30_003)
                })
                .await,
            );
            must(
                report(
                    &f,
                    &c,
                    &attempt,
                    AttemptReport::Printed(job(&attempt)),
                    now + 30_004,
                )
                .await,
            );
        }
    }
    assert_eq!(
        f.print
            .requeue(
                999,
                PrintRequeue {
                    expected_requeue_count: 0
                }
            )
            .await,
        Err(PrintError::NotFound)
    );
    assert_eq!(
        f.print
            .requeue(
                0,
                PrintRequeue {
                    expected_requeue_count: 0
                }
            )
            .await,
        Err(PrintError::Invalid)
    );
    assert_eq!(
        f.print
            .requeue(
                sequence,
                PrintRequeue {
                    expected_requeue_count: -1
                }
            )
            .await,
        Err(PrintError::Invalid)
    );
}

#[tokio::test]
async fn concurrent_requeues_grant_one_round_and_join_the_eligible_tail() {
    let f = Fixture::new().await;
    let c = client(&f).await;
    let sequence = ready(&f).await;
    let now = must(write(&f, super::super::super::db::printers::now).await);
    exhaust(&f, &c, now).await;
    let ahead = ready(&f).await;
    let (one, two) = tokio::join!(
        f.print.requeue(
            sequence,
            PrintRequeue {
                expected_requeue_count: 0
            }
        ),
        f.print.requeue(
            sequence,
            PrintRequeue {
                expected_requeue_count: 0
            }
        ),
    );
    assert_ne!(one.is_ok(), two.is_ok());
    assert!(matches!(
        (one, two),
        (Ok(()), Err(PrintError::Conflict)) | (Err(PrintError::Conflict), Ok(()))
    ));
    let first = claim(&f, &c, &Uuid::now_v7().to_string(), now + 120_000)
        .await
        .unwrap_or_else(|| panic!("ahead"));
    assert_eq!(first.ready_sequence, ahead);
    must(report(&f, &c, &first, AttemptReport::Failed, now + 120_001).await);
    let next = claim(&f, &c, &Uuid::now_v7().to_string(), now + 120_002)
        .await
        .unwrap_or_else(|| panic!("requeued"));
    assert_eq!(next.ready_sequence, sequence);
    assert_eq!(next.attempt_number, 1);
    assert_eq!(
        must(
            f.print
                .attempt_document(c.clone(), next.attempt_id.clone())
                .await
        ),
        super::super::pdf(1)
    );
}

#[tokio::test]
async fn failed_requeue_preserves_the_exhausted_round() {
    let f = Fixture::new().await;
    let c = client(&f).await;
    let sequence = ready(&f).await;
    let now = must(write(&f, super::super::super::db::printers::now).await);
    exhaust(&f, &c, now).await;
    must(write(&f, |tx| {
        tx.connection().batch_execute("CREATE TRIGGER reject_requeue BEFORE UPDATE ON print_jobs BEGIN SELECT RAISE(ABORT, 'fixture'); END")?;
        Ok(())
    }).await);
    assert_eq!(
        f.print
            .requeue(
                sequence,
                PrintRequeue {
                    expected_requeue_count: 0
                }
            )
            .await,
        Err(PrintError::Persistence)
    );
    let saved = must(f.print.list_executions().await);
    assert_eq!(saved[0].state.as_str(), "exhausted");
    assert_eq!(saved[0].attempts_used, 3);
    assert_eq!(saved[0].requeue_count, 0);
    must(
        write(&f, |tx| {
            tx.connection()
                .batch_execute("DROP TRIGGER reject_requeue")?;
            Ok(())
        })
        .await,
    );
    must(
        f.print
            .requeue(
                sequence,
                PrintRequeue {
                    expected_requeue_count: 0,
                },
            )
            .await,
    );
    assert_eq!(must(f.print.list_executions().await)[0].requeue_count, 1);
}

#[tokio::test]
async fn requeue_migration_preserves_live_attempts_and_receipts_and_refuses_lossy_downgrades() {
    let f = Fixture::new().await;
    let c = client(&f).await;
    let sequence = ready(&f).await;
    let now = must(write(&f, super::super::super::db::printers::now).await);
    let old = exhaust(&f, &c, now).await;
    let ahead = ready(&f).await;
    let live_request = Uuid::now_v7().to_string();
    let live = claim(&f, &c, &live_request, now + 120_000)
        .await
        .unwrap_or_else(|| panic!("live claim"));
    assert_eq!(live.ready_sequence, ahead);
    let migrations = || {
        must(FileBasedMigrations::from_path(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations"),
        ))
    };
    let mut connection = must(SqliteConnection::establish(
        f.path.to_str().unwrap_or_else(|| panic!("database path")),
    ));
    must(connection.batch_execute("PRAGMA foreign_keys = ON"));
    must(connection.revert_last_migration(migrations()));
    must(connection.run_next_migration(migrations()));
    assert_eq!(
        must(
            sql::<BigInt>("SELECT count(*) FROM pragma_foreign_key_check")
                .get_result::<i64>(&mut connection)
        ),
        0
    );
    for (request, ended) in &old {
        assert_eq!(
            claim(&f, &c, request, now + 120_000).await.as_ref(),
            Some(ended)
        );
    }
    assert_eq!(
        claim(&f, &c, &live_request, now + 120_001).await,
        Some(live.clone())
    );
    must(report(&f, &c, &live, AttemptReport::Failed, now + 120_001).await);
    must(
        f.print
            .requeue(
                sequence,
                PrintRequeue {
                    expected_requeue_count: 0,
                },
            )
            .await,
    );
    let active = claim(&f, &c, &Uuid::now_v7().to_string(), now + 120_002)
        .await
        .unwrap_or_else(|| panic!("active"));
    assert!(connection.revert_last_migration(migrations()).is_err());
    assert_eq!(
        must(f.print.get_attempt(c, active.attempt_id).await).state,
        AttemptState::Preparing
    );
    assert_eq!(
        must(
            sql::<BigInt>("SELECT count(*) FROM pragma_foreign_key_check")
                .get_result::<i64>(&mut connection)
        ),
        0
    );
}
