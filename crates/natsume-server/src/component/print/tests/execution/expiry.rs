use std::{
    sync::{
        Arc, Condvar, Mutex, PoisonError,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use diesel::{ExpressionMethods as _, OptionalExtension as _, QueryDsl as _, RunQueryDsl as _};
use tokio::{
    sync::{Notify, oneshot},
    time::Instant,
};
use tokio_util::{
    sync::{CancellationToken, DropGuard},
    task::AbortOnDropHandle,
};
use tracing::{
    Subscriber,
    field::{Field, Visit},
    instrument::WithSubscriber as _,
    span::{Attributes, Id},
};
use tracing_subscriber::{
    Layer, Registry,
    layer::{Context, SubscriberExt as _},
    registry::LookupSpan,
};

use super::*;
use crate::{
    component::print::{
        execution::PrintClaim,
        expiry::{CheckRequest, ExpiryError, Inbox, check_at},
    },
    diesel_schema::{print_attempts as a, print_jobs as j},
};

struct Running {
    stop: DropGuard,
    task: AbortOnDropHandle<Result<(), ExpiryError>>,
}

impl Running {
    async fn start(f: &Fixture) -> Self {
        let stop = CancellationToken::new();
        let (ready, initialized) = oneshot::channel();
        let running = Self {
            task: AbortOnDropHandle::new(tokio::spawn(
                f.print
                    .maintain_attempts(stop.clone(), ready)
                    .with_current_subscriber(),
            )),
            stop: stop.drop_guard(),
        };
        must(must(
            tokio::time::timeout(Duration::from_secs(5), initialized).await,
        ));
        running
    }

    async fn shutdown(self) {
        self.stop.token().cancel();
        must(must(self.task.await));
    }
}

async fn public_claim(f: &Fixture, client: &ClientIdentity) -> PrintAttempt {
    must(
        f.print
            .claim(
                client.clone(),
                PrintClaim {
                    request_id: Uuid::now_v7().to_string(),
                    queue_name: "queue".to_owned(),
                },
            )
            .await,
    )
    .unwrap_or_else(|| panic!("missing claim"))
}

async fn outcome(f: &Fixture, id: &str) -> Option<String> {
    let id = id.to_owned();
    must(
        f.database
            .read(move |tx| {
                Ok::<_, PrintError>(
                    a::table
                        .find(id)
                        .select(a::outcome)
                        .first::<Option<String>>(tx.connection())
                        .optional()?
                        .flatten(),
                )
            })
            .await,
    )
}

async fn wait_for_outcome(f: &Fixture, id: &str, expected: &str) {
    must(
        tokio::time::timeout(Duration::from_secs(5), async {
            while outcome(f, id).await.as_deref() != Some(expected) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await,
    );
}

fn immediately(f: &Fixture, id: &str) {
    must(f.print.expiry.schedule(CheckRequest {
        attempt_id: id.to_owned(),
        check_at: Instant::now(),
    }));
}

#[tokio::test]
async fn check_times_are_advisory_bounded_and_overflow_safe() {
    let now = Instant::now();
    assert_eq!(check_at(now, 10, 10), now);
    assert_eq!(check_at(now, 10, 9), now);
    assert_eq!(check_at(now, 10, 11), now + Duration::from_millis(1));
    assert_eq!(
        check_at(now, i64::MIN, i64::MAX),
        now + Duration::from_mins(1)
    );
    assert_eq!(check_at(now, i64::MAX, i64::MIN), now);
    assert_eq!(check_at(now, 100, 60_100), now + Duration::from_mins(1));
}

#[tokio::test]
async fn inbox_coalesces_earliest_checks_and_preserves_remaining_batches() {
    let inbox = Inbox::default();
    let now = Instant::now();
    for offset in [10, 20, 5] {
        must(inbox.schedule(CheckRequest {
            attempt_id: "same".to_owned(),
            check_at: now + Duration::from_secs(offset),
        }));
    }
    let entries = must(inbox.take());
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].check_at, now + Duration::from_secs(5));
    for index in 0..300 {
        must(inbox.schedule(CheckRequest {
            attempt_id: index.to_string(),
            check_at: now,
        }));
    }
    assert_eq!(must(inbox.take()).len(), 128);
    assert_eq!(must(inbox.take()).len(), 128);
    assert_eq!(must(inbox.take()).len(), 44);
    assert!(must(inbox.take()).is_empty());
}

#[tokio::test]
async fn only_active_claim_results_publish_checks_and_renewals_need_no_notification() {
    let f = Fixture::new().await;
    let client = client(&f).await;
    let empty = PrintClaim {
        request_id: Uuid::now_v7().to_string(),
        queue_name: "queue".to_owned(),
    };
    assert!(must(f.print.claim(client.clone(), empty).await).is_none());
    assert!(must(f.print.expiry.take()).is_empty());
    ready(&f).await;
    let attempt = public_claim(&f, &client).await;
    let entries = must(f.print.expiry.take());
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].attempt_id, attempt.attempt_id);
    must(
        f.print
            .report_attempt(
                client.clone(),
                attempt.attempt_id.clone(),
                AttemptReport::Contact,
            )
            .await,
    );
    must(
        f.print
            .register_job(client.clone(), attempt.attempt_id.clone(), job(&attempt))
            .await,
    );
    must(
        f.print
            .report_attempt(
                client.clone(),
                attempt.attempt_id.clone(),
                AttemptReport::Observed(job(&attempt)),
            )
            .await,
    );
    must(
        f.print
            .report_attempt(client, attempt.attempt_id, AttemptReport::Failed)
            .await,
    );
    assert!(must(f.print.expiry.take()).is_empty());
}

#[tokio::test]
async fn newly_claimed_attempt_is_checked_without_any_further_client_requests() {
    let f = Fixture::new().await;
    let client = client(&f).await;
    ready(&f).await;
    let running = Running::start(&f).await;
    let attempt = public_claim(&f, &client).await;
    must(
        write(&f, |tx| {
            diesel::update(a::table)
                .set(a::lease_expires_at_unix_ms.eq(0))
                .execute(tx.connection())?;
            Ok(())
        })
        .await,
    );
    immediately(&f, &attempt.attempt_id);
    wait_for_outcome(&f, &attempt.attempt_id, "unknown").await;
    running.shutdown().await;
    let execution = must(f.print.list_executions().await);
    assert_eq!(execution[0].attempts_used, 1);
    assert_eq!(execution[0].state.as_str(), "waiting");
}

#[tokio::test]
async fn check_transaction_uses_current_deadline_and_ignores_old_attempts() {
    let f = Fixture::new().await;
    let client = client(&f).await;
    ready(&f).await;
    let now = must(write(&f, super::super::super::db::printers::now).await);
    let first = claim(&f, &client, &Uuid::now_v7().to_string(), now)
        .await
        .unwrap_or_else(|| panic!("claim"));
    let id = first.attempt_id.clone();
    let current = must(write(&f, move |tx| store::check_attempts(tx, &[id], now + 59_999)).await);
    assert_eq!(current.len(), 1);
    assert_eq!(current[0].expires_at_unix_ms, now + 60_000);
    must(report(&f, &client, &first, AttemptReport::Contact, now + 59_999).await);
    let id = first.attempt_id.clone();
    let renewed = must(write(&f, move |tx| store::check_attempts(tx, &[id], now + 60_000)).await);
    assert_eq!(renewed[0].expires_at_unix_ms, now + 119_999);
    let id = first.attempt_id.clone();
    assert!(
        must(
            write(&f, move |tx| store::check_attempts(
                tx,
                &[id],
                now + 119_999
            ))
            .await
        )
        .is_empty()
    );
    let second = claim(&f, &client, &Uuid::now_v7().to_string(), now + 149_999)
        .await
        .unwrap_or_else(|| panic!("retry"));
    let id = first.attempt_id;
    assert!(
        must(
            write(&f, move |tx| store::check_attempts(
                tx,
                &[id],
                now + 999_999
            ))
            .await
        )
        .is_empty()
    );
    assert_eq!(outcome(&f, &second.attempt_id).await, None);
    let active = must(write(&f, store::load_active_deadlines).await);
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].attempt_id, second.attempt_id);
    let used = must(
        f.database
            .read(move |tx| {
                Ok::<_, PrintError>(
                    j::table
                        .select(j::attempts_used)
                        .first::<i32>(tx.connection())?,
                )
            })
            .await,
    );
    assert_eq!(used, 2);
}

#[derive(Clone, Default)]
struct Probe {
    queries: Arc<AtomicUsize>,
    checks: Arc<AtomicUsize>,
    checked_ids: Arc<AtomicUsize>,
    started: Arc<Notify>,
    finished: Arc<Notify>,
}

impl<S> Layer<S> for Probe
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    fn on_new_span(&self, attributes: &Attributes<'_>, _id: &Id, _context: Context<'_, S>) {
        match attributes.metadata().name() {
            "db.query" => {
                self.queries.fetch_add(1, Ordering::SeqCst);
            }
            "print.expiry.check" => {
                self.checks.fetch_add(1, Ordering::SeqCst);
                attributes.record(&mut AttemptCount(&self.checked_ids));
                self.started.notify_one();
            }
            _ => {}
        }
    }

    fn on_close(&self, id: Id, context: Context<'_, S>) {
        if context
            .span(&id)
            .is_some_and(|span| span.metadata().name() == "print.expiry.check")
        {
            self.finished.notify_one();
        }
    }
}

struct AttemptCount<'a>(&'a AtomicUsize);

impl Visit for AttemptCount<'_> {
    fn record_u64(&mut self, field: &Field, value: u64) {
        if field.name() == "attempts" {
            self.0.fetch_add(
                usize::try_from(value).unwrap_or(usize::MAX),
                Ordering::SeqCst,
            );
        }
    }

    fn record_debug(&mut self, _field: &Field, _value: &dyn std::fmt::Debug) {}
}

struct Gate(Arc<(Mutex<bool>, Condvar)>);

impl Gate {
    fn new() -> Self {
        Self(Arc::new((Mutex::new(false), Condvar::new())))
    }

    fn wait(state: &Arc<(Mutex<bool>, Condvar)>) {
        let guard = state.0.lock().unwrap_or_else(PoisonError::into_inner);
        drop(
            state
                .1
                .wait_while(guard, |released| !*released)
                .unwrap_or_else(PoisonError::into_inner),
        );
    }
}

impl Drop for Gate {
    fn drop(&mut self) {
        *self.0.0.lock().unwrap_or_else(PoisonError::into_inner) = true;
        self.0.1.notify_all();
    }
}

type ClaimTask = AbortOnDropHandle<Result<String, crate::db::TransactionError<PrintError>>>;

async fn held_claim(
    f: &Fixture,
    client: ClientIdentity,
    rollback: bool,
) -> (ClaimTask, Gate, String) {
    let gate = Gate::new();
    let worker_gate = Arc::clone(&gate.0);
    let database = f.database.clone();
    let inbox = Arc::clone(&f.print.expiry);
    let (entered, started) = oneshot::channel();
    let task = AbortOnDropHandle::new(tokio::spawn(async move {
        database
            .write(move |tx| {
                let now = super::super::super::db::printers::now(tx)?;
                let attempt = store::claim(tx, &client, &Uuid::now_v7().to_string(), "queue", now)?
                    .unwrap_or_else(|| panic!("claim"));
                diesel::update(a::table.find(&attempt.attempt_id))
                    .set(a::lease_expires_at_unix_ms.eq(0))
                    .execute(tx.connection())?;
                must(inbox.schedule(CheckRequest {
                    attempt_id: attempt.attempt_id.clone(),
                    check_at: Instant::now(),
                }));
                must(entered.send(attempt.attempt_id.clone()));
                Gate::wait(&worker_gate);
                if rollback {
                    Err(PrintError::Conflict)
                } else {
                    Ok(attempt.attempt_id)
                }
            })
            .await
    }));
    let id = must(must(
        tokio::time::timeout(Duration::from_secs(5), started).await,
    ));
    (task, gate, id)
}

#[tokio::test]
async fn precommit_check_survives_request_cancellation_and_waits_for_the_writer() {
    let _guard = crate::logging::tests::SubscriberTestGuard::acquire();
    let f = Fixture::new().await;
    let client = client(&f).await;
    ready(&f).await;
    let probe = Probe::default();
    async {
        let running = Running::start(&f).await;
        let (task, gate, id) = held_claim(&f, client, false).await;
        must(tokio::time::timeout(Duration::from_secs(5), probe.started.notified()).await);
        let visible = must(
            f.database
                .read(|tx| {
                    Ok::<_, PrintError>(a::table.count().get_result::<i64>(tx.connection())?)
                })
                .await,
        );
        assert_eq!(
            visible, 0,
            "ordinary WAL reads still see the precommit snapshot"
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(100), probe.finished.notified())
                .await
                .is_err()
        );
        task.abort();
        let Err(error) = task.await else {
            panic!("caller was not cancelled")
        };
        assert!(error.is_cancelled());
        drop(gate);
        wait_for_outcome(&f, &id, "unknown").await;
        running.shutdown().await;
    }
    .with_subscriber(Registry::default().with(probe.clone()))
    .await;
}

#[tokio::test]
async fn rollback_leaves_only_a_harmless_check_and_does_not_consume_budget() {
    let _guard = crate::logging::tests::SubscriberTestGuard::acquire();
    let f = Fixture::new().await;
    let client = client(&f).await;
    ready(&f).await;
    let probe = Probe::default();
    async {
        let running = Running::start(&f).await;
        let (task, gate, _) = held_claim(&f, client.clone(), true).await;
        must(tokio::time::timeout(Duration::from_secs(5), probe.started.notified()).await);
        drop(gate);
        assert_eq!(
            must(task.await),
            Err(crate::db::TransactionError::Operation(PrintError::Conflict))
        );
        must(tokio::time::timeout(Duration::from_secs(5), probe.finished.notified()).await);
        let attempt = public_claim(&f, &client).await;
        assert_eq!(attempt.attempt_number, 1);
        assert_eq!(outcome(&f, &attempt.attempt_id).await, None);
        running.shutdown().await;
    }
    .with_subscriber(Registry::default().with(probe.clone()))
    .await;
}

#[tokio::test]
async fn failed_check_rolls_back_and_retries_without_notification_busy_loop() {
    use diesel::connection::SimpleConnection as _;
    let _guard = crate::logging::tests::SubscriberTestGuard::acquire();
    let f = Fixture::new().await;
    let client = client(&f).await;
    ready(&f).await;
    let probe = Probe::default();
    async {
        let running = Running::start(&f).await;
        let attempt = public_claim(&f, &client).await;
        must(write(&f, |tx| {
            diesel::update(a::table).set(a::lease_expires_at_unix_ms.eq(0)).execute(tx.connection())?;
            tx.connection().batch_execute("CREATE TRIGGER reject_expiry BEFORE UPDATE ON print_jobs BEGIN SELECT RAISE(ABORT, 'fixture'); END")?;
            Ok(())
        }).await);
        immediately(&f, &attempt.attempt_id);
        must(tokio::time::timeout(Duration::from_secs(5), probe.finished.notified()).await);
        assert_eq!(outcome(&f, &attempt.attempt_id).await, None);
        let ghost = Uuid::now_v7().to_string();
        for _ in 0..300 { immediately(&f, &ghost); }
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(probe.checks.load(Ordering::SeqCst), 1);
        must(write(&f, |tx| {
            tx.connection().batch_execute("DROP TRIGGER reject_expiry")?;
            Ok(())
        }).await);
        wait_for_outcome(&f, &attempt.attempt_id, "unknown").await;
        assert!(probe.checks.load(Ordering::SeqCst) >= 2);
        running.shutdown().await;
    }.with_subscriber(Registry::default().with(probe.clone())).await;
}

#[tokio::test]
async fn idle_scheduler_and_future_hint_installation_do_not_query_the_database() {
    let _guard = crate::logging::tests::SubscriberTestGuard::acquire();
    let f = Fixture::new().await;
    let probe = Probe::default();
    async {
        let running = Running::start(&f).await;
        let initialized_queries = probe.queries.load(Ordering::SeqCst);
        assert!(initialized_queries > 0);
        tokio::time::pause();
        for _ in 0..3 {
            tokio::time::advance(Duration::from_secs(1)).await;
            tokio::task::yield_now().await;
        }
        assert_eq!(probe.queries.load(Ordering::SeqCst), initialized_queries);
        for _ in 0..300 {
            must(f.print.expiry.schedule(CheckRequest {
                attempt_id: Uuid::now_v7().to_string(),
                check_at: Instant::now() + Duration::from_mins(1),
            }));
        }
        for _ in 0..3 {
            tokio::time::advance(Duration::from_secs(1)).await;
            tokio::task::yield_now().await;
        }
        assert_eq!(probe.queries.load(Ordering::SeqCst), initialized_queries);
        assert_eq!(probe.checks.load(Ordering::SeqCst), 0);
        running.shutdown().await;
    }
    .with_subscriber(Registry::default().with(probe.clone()))
    .await;
}

#[tokio::test]
async fn all_check_batches_drain_without_new_notifications_or_stale_keys() {
    let _guard = crate::logging::tests::SubscriberTestGuard::acquire();
    let f = Fixture::new().await;
    let probe = Probe::default();
    async {
        let running = Running::start(&f).await;
        for _ in 0..300 {
            immediately(&f, &Uuid::now_v7().to_string());
        }
        must(
            tokio::time::timeout(Duration::from_secs(5), async {
                while probe.checked_ids.load(Ordering::SeqCst) < 300 {
                    probe.finished.notified().await;
                }
            })
            .await,
        );
        for _ in 0..300 {
            immediately(&f, &Uuid::now_v7().to_string());
        }
        must(
            tokio::time::timeout(Duration::from_secs(5), async {
                while probe.checked_ids.load(Ordering::SeqCst) < 600 {
                    probe.finished.notified().await;
                }
            })
            .await,
        );
        running.shutdown().await;
        assert_eq!(probe.checked_ids.load(Ordering::SeqCst), 600);
    }
    .with_subscriber(Registry::default().with(probe.clone()))
    .await;
}

#[tokio::test]
async fn rebuilding_the_scheduler_does_not_issue_another_recovery_grace() {
    let f = Fixture::new().await;
    let client = client(&f).await;
    ready(&f).await;
    let attempt = public_claim(&f, &client).await;
    let registered = must(
        f.print
            .register_job(client.clone(), attempt.attempt_id.clone(), job(&attempt))
            .await,
    );
    let running = Running::start(&f).await;
    running.shutdown().await;
    let running = Running::start(&f).await;
    let saved = must(write(&f, move |tx| store::current(tx, &client)).await);
    assert_eq!(saved, Some(registered));
    running.shutdown().await;
}

#[tokio::test]
async fn background_initialization_reports_snapshot_failure_instead_of_becoming_ready() {
    use diesel::connection::SimpleConnection as _;
    let f = Fixture::new().await;
    let state = must(crate::server_state::tests::for_test(
        f.database.clone(),
        f.documents_path(),
    ));
    must(
        write(&f, |tx| {
            tx.connection().batch_execute("DROP TABLE print_attempts")?;
            Ok(())
        })
        .await,
    );
    let Err(error) = crate::background::start_background_tasks(&state).await else {
        panic!("initialization unexpectedly succeeded");
    };
    assert!(
        std::error::Error::source(&error)
            .and_then(|source| source.downcast_ref::<ExpiryError>())
            .is_some_and(|source| matches!(
                source,
                ExpiryError::Initialize {
                    source: PrintError::Persistence
                }
            ))
    );
}

#[tokio::test]
async fn an_early_check_rearms_and_later_reads_renewal_without_any_notification() {
    let _guard = crate::logging::tests::SubscriberTestGuard::acquire();
    let f = Fixture::new().await;
    let client = client(&f).await;
    ready(&f).await;
    let probe = Probe::default();
    async {
        let running = Running::start(&f).await;
        let attempt = public_claim(&f, &client).await;
        must(
            write(&f, |tx| {
                let now = super::super::super::db::printers::now(tx)?;
                diesel::update(a::table)
                    .set(a::lease_expires_at_unix_ms.eq(now + 1_000))
                    .execute(tx.connection())?;
                Ok(())
            })
            .await,
        );
        immediately(&f, &attempt.attempt_id);
        must(tokio::time::timeout(Duration::from_secs(5), probe.finished.notified()).await);
        let renewed = must(
            f.print
                .report_attempt(client, attempt.attempt_id.clone(), AttemptReport::Contact)
                .await,
        );
        assert_eq!(renewed.state, AttemptState::Preparing);
        assert!(must(f.print.expiry.take()).is_empty());
        must(tokio::time::timeout(Duration::from_secs(5), probe.finished.notified()).await);
        assert!(probe.checks.load(Ordering::SeqCst) >= 2);
        assert_eq!(outcome(&f, &attempt.attempt_id).await, None);
        running.shutdown().await;
    }
    .with_subscriber(Registry::default().with(probe.clone()))
    .await;
}

#[tokio::test]
async fn cancelling_background_initialization_releases_the_owned_task() {
    let f = Fixture::new().await;
    let state = Arc::new(must(crate::server_state::tests::for_test(
        f.database.clone(),
        f.documents_path(),
    )));
    let weak_state = Arc::downgrade(&state);
    let weak_inbox = Arc::downgrade(&state.print().expiry);
    let held_connections = f.database.test_exhaust_pool();
    let start = AbortOnDropHandle::new(tokio::spawn(async move {
        crate::background::start_background_tasks(&state).await
    }));
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!start.is_finished());
    start.abort();
    assert!(start.await.is_err());
    drop(held_connections);
    must(
        tokio::time::timeout(Duration::from_secs(5), async {
            while weak_state.upgrade().is_some() || weak_inbox.upgrade().is_some() {
                tokio::task::yield_now().await;
            }
        })
        .await,
    );
}
