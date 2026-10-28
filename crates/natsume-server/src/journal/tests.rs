use super::*;
use crate::{
    db::{Database, DatabaseConfig},
    test_error::TestError,
};
use diesel::{ExpressionMethods as _, RunQueryDsl as _};
use event::payload::{ReconcileRetry, ReviewCreated};
use tokio::time::{sleep, timeout};

fn config() -> JournalConfig {
    JournalConfig {
        min_free_bytes: 1,
        ..JournalConfig::default()
    }
}

pub(crate) async fn ready(journal: &JournalComponent) -> Result<(), TestError> {
    timeout(Duration::from_secs(5), async {
        while journal.stats().degraded
            || journal
                .stats()
                .categories
                .iter()
                .any(|entry| entry.value.queued != 0)
        {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    Ok(())
}

pub(crate) async fn fast_busy(component: &JournalComponent) -> Result<(), TestError> {
    let runtime = component.runtime.lock().await;
    let database = runtime
        .as_ref()
        .ok_or(TestError::missing("journal runtime"))?
        .database
        .borrow()
        .clone()
        .ok_or(TestError::missing("journal database"))?;
    db::tests::fast_busy_timeout(&database)
}

#[tokio::test]
async fn full_and_closed_channels_count_drops_without_work_or_serialization() {
    let (component, receiver) = JournalComponent::channel(1);
    let journal = component.emitter();
    assert_eq!(component.stats().pause_reason, None);
    assert!(!component.stats().paused);
    journal.emit(
        Event::ReviewCreated(ReviewCreated {}),
        JournalContext::default(),
    );
    for _ in 0..7 {
        journal.emit(
            Event::ReviewCreated(ReviewCreated {}),
            JournalContext::default(),
        );
    }
    assert_eq!(component.stats().categories.device.dropped, 7);
    assert_eq!(component.stats().queue_depth, 1);
    assert_eq!(component.stats().write_failed, 0);
    drop(receiver);
    journal.emit(
        Event::ReviewCreated(ReviewCreated {}),
        JournalContext::default(),
    );
    assert_eq!(component.stats().categories.device.dropped, 8);
    component.record_producer_timeout();
    assert_eq!(component.stats().shutdown_timeouts, 1);
    assert_eq!(
        component.stats().last_error,
        Some(ErrorClass::ProducerTimeout)
    );
}

#[tokio::test]
async fn shutdown_drains_batches_with_live_sender_clones_and_one_terminal_marker()
-> Result<(), TestError> {
    let root = tempfile::tempdir()?;
    let path = root.path().join("journal.db");
    let component = JournalComponent::start(path.clone(), config());
    let journal = component.emitter();
    let retained = journal.clone();
    for attempt in 0..500 {
        journal.emit(
            Event::ReconcileRetry(ReconcileRetry { attempt }),
            JournalContext::default(),
        );
    }
    component.shutdown().await;
    let stopped = component.stats();
    component.shutdown().await;
    assert_eq!(component.stats(), stopped);
    assert_eq!(component.stats().queue_depth, 0);
    assert_eq!(
        component
            .stats()
            .categories
            .iter()
            .map(|entry| entry.value.queued)
            .sum::<u64>(),
        0
    );
    let database = db::JournalDatabase::open(&path).await?;
    let rows = database.read_cursor(None, None, 1000).await?;
    assert_eq!(rows.len(), 502);
    assert_eq!(
        rows.iter()
            .filter(|(_, row)| row.kind == "server_started")
            .count(),
        1
    );
    assert_eq!(rows[0].1.kind, "server_stopping");
    assert_eq!(
        rows.iter()
            .filter(|(_, row)| row.kind == "server_stopping")
            .count(),
        1
    );
    assert!(
        rows.iter()
            .all(|(_, row)| row.boot_id == component.boot_id().to_string())
    );
    retained.emit(
        Event::ReviewCreated(ReviewCreated {}),
        JournalContext::default(),
    );
    assert_eq!(component.stats().categories.device.dropped, 1);
    Ok(())
}

#[tokio::test]
async fn unavailable_journal_does_not_change_http_statuses_or_business_writes()
-> Result<(), TestError> {
    use axum::{body::Body, http::Request};
    use tower::ServiceExt as _;
    let root = tempfile::tempdir()?;
    let database =
        Database::connect_and_migrate(&DatabaseConfig::new(root.path().join("business.db"), true))
            .await?;
    let component = JournalComponent::start(root.path().join("absent/journal.db"), config());
    let journal = component.emitter();
    let degraded = Arc::new(crate::server_state::tests::for_test_with_journal(
        database.clone(),
        root.path().join("documents"),
        root.path().join("logos"),
        component,
    )?);
    let normal = Arc::new(crate::server_state::tests::for_test(
        database.clone(),
        root.path().join("normal-documents"),
    )?);
    let first = crate::http::router(Arc::clone(&degraded), root.path());
    let second = crate::http::router(normal, root.path());
    for path in [
        "/api/v2/health",
        "/api/v2/operator/session",
        "/api/v2/missing",
        "/api/v2/journal/status",
    ] {
        let degraded_response = first
            .clone()
            .oneshot(Request::get(path).body(Body::empty())?)
            .await?;
        let normal_response = second
            .clone()
            .oneshot(Request::get(path).body(Body::empty())?)
            .await?;
        assert_eq!(degraded_response.status(), normal_response.status());
    }
    for _ in 0..5 {
        journal.emit(
            Event::ReviewCreated(ReviewCreated {}),
            JournalContext::default(),
        );
    }
    timeout(Duration::from_secs(2), async {
        while degraded.journal().stats().categories.device.dropped < 5 {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    assert!(degraded.journal().stats().degraded);
    database
        .write(|transaction| {
            diesel::insert_into(crate::diesel_schema::seats::table)
                .values((
                    crate::diesel_schema::seats::seat_id.eq("journal-isolation"),
                    crate::diesel_schema::seats::seat_code.eq("A-01"),
                ))
                .execute(transaction.connection())
                .map_err(crate::db::PersistenceError::from)
        })
        .await?;
    degraded.journal().shutdown().await;
    Ok(())
}

#[tokio::test]
async fn failed_batch_is_counted_and_drop_marker_is_written_when_backlog_clears()
-> Result<(), TestError> {
    use diesel::{Connection as _, connection::SimpleConnection as _};
    let root = tempfile::tempdir()?;
    let path = root.path().join("journal.db");
    let component = JournalComponent::start(path.clone(), config());
    ready(&component).await?;
    fast_busy(&component).await?;
    let journal = component.emitter();
    let mut locked = diesel::sqlite::SqliteConnection::establish(
        path.to_str().ok_or(TestError::missing("UTF-8 path"))?,
    )?;
    locked.batch_execute("BEGIN EXCLUSIVE")?;
    journal.emit(
        Event::ReviewCreated(ReviewCreated {}),
        JournalContext::default(),
    );
    timeout(Duration::from_secs(3), async {
        while component.stats().write_failed == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    assert_eq!(component.stats().categories.device.dropped, 1);
    drop(locked);
    journal.emit(
        Event::ReconcileRetry(ReconcileRetry { attempt: 1 }),
        JournalContext::default(),
    );
    component.shutdown().await;
    let rows = db::JournalDatabase::open(&path)
        .await?
        .read_cursor(None, None, 1000)
        .await?;
    assert!(
        rows.iter()
            .any(|(_, row)| row.kind == "journal_dropped" && row.details.contains("\"count\":1"))
    );
    assert!(rows.iter().any(|(_, row)| row.kind == "reconcile_retry"));
    Ok(())
}

#[tokio::test]
async fn timed_out_flush_counts_lost_work_and_persists_best_effort_terminal_count()
-> Result<(), TestError> {
    use diesel::{Connection as _, connection::SimpleConnection as _};
    let root = tempfile::tempdir()?;
    let path = root.path().join("journal.db");
    let component = JournalComponent::start(path.clone(), config());
    ready(&component).await?;
    fast_busy(&component).await?;
    let journal = component.emitter();
    let mut locked = diesel::sqlite::SqliteConnection::establish(
        path.to_str().ok_or(TestError::missing("UTF-8 path"))?,
    )?;
    locked.batch_execute("BEGIN EXCLUSIVE")?;
    journal.emit(
        Event::ReviewCreated(ReviewCreated {}),
        JournalContext::default(),
    );
    timeout(Duration::from_secs(2), async {
        while component.stats().last_error != Some(ErrorClass::Write) {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    component
        .runtime
        .lock()
        .await
        .as_mut()
        .ok_or(TestError::missing("journal runtime"))?
        .flush_timeout = Duration::from_millis(20);
    let release = tokio::spawn(async move {
        sleep(Duration::from_millis(10)).await;
        drop(locked);
    });
    component.shutdown().await;
    release.await?;
    assert_eq!(component.stats().shutdown_timeouts, 1);
    assert!(
        component
            .stats()
            .categories
            .iter()
            .any(|entry| entry.value.dropped > 0)
    );
    let rows = db::JournalDatabase::open(&path)
        .await?
        .read_cursor(None, None, 1000)
        .await?;
    assert!(
        rows.iter().any(|(_, row)| row.kind == "server_stopping"
            && !row.details.contains("\"lost_on_flush\":null"))
    );
    Ok(())
}

#[tokio::test]
async fn disk_pause_prunes_then_records_pause_and_resume_without_blocking_business()
-> Result<(), TestError> {
    let root = tempfile::tempdir()?;
    let path = root.path().join("journal.db");
    let seeded = db::JournalDatabase::open(&path).await?;
    let mut row = writer::record(
        Uuid::now_v7(),
        Envelope {
            event: Event::ReviewCreated(ReviewCreated {}),
            context: JournalContext::default(),
            occurred_at_ms: now_ms(),
        },
    )
    .unwrap_or_else(|error| panic!("fixture event: {error}"));
    row.details = format!("{{\"v\":1,\"padding\":\"{}\"}}", "x".repeat(1024));
    seeded
        .insert_batch(&std::iter::repeat_n(row, 4000).collect::<Vec<_>>())
        .await?;
    drop(seeded);
    let component = JournalComponent::start(
        path.clone(),
        JournalConfig {
            max_bytes: 1024 * 1024,
            ..config()
        },
    );
    let journal = component.emitter();
    journal.emit(
        Event::ReviewCreated(ReviewCreated {}),
        JournalContext::default(),
    );
    timeout(Duration::from_secs(5), async {
        while component.stats().categories.device.dropped == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    let business =
        Database::connect_and_migrate(&DatabaseConfig::new(root.path().join("business.db"), true))
            .await?;
    business
        .write(|transaction| {
            diesel::insert_into(crate::diesel_schema::seats::table)
                .values((
                    crate::diesel_schema::seats::seat_id.eq("during-pause"),
                    crate::diesel_schema::seats::seat_code.eq("A-01"),
                ))
                .execute(transaction.connection())
                .map_err(crate::db::PersistenceError::from)
        })
        .await?;
    timeout(Duration::from_secs(10), async {
        while component.stats().paused {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    component.shutdown().await;
    let rows = db::JournalDatabase::open(&path)
        .await?
        .read_cursor(None, None, 1000)
        .await?;
    assert!(rows.iter().any(|(_, row)| row.kind == "journal_paused"));
    assert!(rows.iter().any(|(_, row)| row.kind == "journal_resumed"));
    assert!(!rows.iter().any(|(_, row)| row.kind == "review_created"));
    Ok(())
}

#[tokio::test]
async fn degraded_writer_reopens_after_sixty_seconds_and_reports_prior_drops()
-> Result<(), TestError> {
    let root = tempfile::tempdir()?;
    let directory = root.path().join("later");
    let path = directory.join("journal.db");
    let component = JournalComponent::start(path.clone(), config());
    let journal = component.emitter();
    journal.emit(
        Event::ReviewCreated(ReviewCreated {}),
        JournalContext::default(),
    );
    timeout(Duration::from_secs(2), async {
        while component.stats().categories.device.dropped == 0
            || component.stats().last_error != Some(ErrorClass::Open)
        {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    std::fs::create_dir(&directory)?;
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(59)).await;
    assert!(component.stats().degraded);
    tokio::time::advance(Duration::from_secs(2)).await;
    tokio::time::resume();
    ready(&component).await?;
    component.shutdown().await;
    let rows = db::JournalDatabase::open(&path)
        .await?
        .read_cursor(None, None, 1000)
        .await?;
    assert!(rows.iter().any(|(_, row)| row.kind == "journal_dropped"));
    Ok(())
}

#[test]
fn trace_context_is_only_captured_from_a_valid_otel_span() {
    use opentelemetry::trace::TracerProvider as _;
    use tracing_subscriber::prelude::*;
    assert!(
        JournalContext::default()
            .with_current_trace()
            .trace_id
            .is_none()
    );
    let request = RequestId::from_request(Uuid::now_v7());
    let operation = OperationId::from_operation(Uuid::now_v7());
    let session = SessionId::from_lease(Uuid::now_v7());
    let context = JournalContext::default()
        .with_request(request)
        .with_operation(operation)
        .with_session(session)
        .with_actor(OperatorId::from_authenticated_operator(Uuid::nil()))
        .with_device(
            crate::component::device::DeviceId::parse("019a1000-0000-7000-8000-000000000002")
                .unwrap_or_else(|| panic!("fixture device")),
        )
        .with_seat(
            SeatCode::from_committed_binding("A-01")
                .unwrap_or_else(|error| panic!("fixture seat: {error}")),
        );
    assert_eq!(context.request_id, Some(request));
    assert_eq!(context.operation_id, Some(operation));
    assert_eq!(context.session_id, Some(session));
    assert!(context.trace_id.is_none());
    let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder().build();
    let tracer = provider.tracer("journal-test");
    let subscriber =
        tracing_subscriber::registry().with(tracing_opentelemetry::layer().with_tracer(tracer));
    tracing::subscriber::with_default(subscriber, || {
        let span = tracing::info_span!("journal_trace");
        span.in_scope(|| {
            assert!(
                JournalContext::default()
                    .with_current_trace()
                    .trace_id
                    .is_some()
            );
        });
    });
}

#[test]
fn typed_atomic_states_round_trip_without_exposing_encodings() {
    fn check_error(state: &AtomicErrorClass, error: Option<ErrorClass>) {
        state.store(error);
        assert_eq!(state.load(), error);
    }
    let error = AtomicErrorClass::new(None);
    assert_eq!(error.load(), None);
    check_error(&error, Some(ErrorClass::Open));
    check_error(&error, Some(ErrorClass::Write));
    check_error(&error, Some(ErrorClass::Serialization));
    check_error(&error, Some(ErrorClass::Filesystem));
    check_error(&error, Some(ErrorClass::FlushTimeout));
    check_error(&error, Some(ErrorClass::ProducerTimeout));
    check_error(&error, None);
    let reason = AtomicPauseReason::new(None);
    assert_eq!(reason.load(), None);
    reason.store(Some(&PauseReason::DiskBudget));
    assert_eq!(reason.load(), Some(PauseReason::DiskBudget));
    reason.store(None);
    assert_eq!(reason.load(), None);
    let first_drop = FirstDropTimestamp::default();
    assert_eq!(first_drop.load(), None);
    first_drop.record_first(0);
    first_drop.record_first(10);
    assert_eq!(first_drop.load(), Some(0));
    let latest = FirstDropTimestamp::default();
    latest.record_first(i64::MAX);
    assert_eq!(latest.load(), Some(i64::MAX));
}

#[test]
fn category_snapshots_keep_independent_counts_and_conservative_loss_bounds() {
    let (component, _receiver) = JournalComponent::channel(1);
    let stats = &component.emitter.stats;
    stats.drop_events(&Category::Device, 2, 100);
    stats.drop_events(&Category::Device, 3, 200);
    stats.drop_events(&Category::Audit, 4, 300);
    stats.drop_events(&Category::Auth, 6, 400);
    stats.drop_events(&Category::Journal, 8, 500);
    let snapshot = component.stats();
    assert!(snapshot.degraded);
    assert_eq!(snapshot.categories.device.dropped, 5);
    assert_eq!(
        snapshot.categories.device.first_unreported_drop_ms,
        Some(100)
    );
    assert_eq!(snapshot.categories.audit.dropped, 4);
    assert_eq!(
        snapshot.categories.audit.first_unreported_drop_ms,
        Some(300)
    );
    assert_eq!(snapshot.categories.auth.dropped, 6);
    assert_eq!(snapshot.categories.auth.first_unreported_drop_ms, Some(400));
    assert_eq!(snapshot.categories.journal.dropped, 8);
    assert_eq!(
        snapshot.categories.journal.first_unreported_drop_ms,
        Some(500)
    );
    assert!(
        snapshot
            .categories
            .iter()
            .all(|entry| entry.value.queued == 0)
    );
    let mut reported = PerCategory::<u64>::default();
    for CategoryEntry {
        category,
        value: counters,
    } in snapshot.categories.iter()
    {
        *reported.get_mut(&category) = counters.dropped;
    }
    assert_eq!(
        reported,
        PerCategory {
            device: 5,
            audit: 4,
            auth: 6,
            journal: 8
        }
    );
}

async fn verify_finished_writer_reconciles_queue(task: JoinHandle<()>) {
    let (component, receiver) = JournalComponent::channel(4);
    let journal = component.emitter();
    journal.emit(
        Event::ReviewCreated(ReviewCreated {}),
        JournalContext::default(),
    );
    let (_database, current_database) = watch::channel(None);
    let runtime = WriterRuntime {
        task,
        stop: CancellationToken::new(),
        journal,
        flush_timeout: Duration::from_secs(1),
        database: current_database,
    };
    runtime.shutdown().await;
    drop(receiver);
    let stats = component.stats();
    assert_eq!(stats.queue_depth, 0);
    assert!(stats.categories.iter().all(|entry| entry.value.queued == 0));
    assert_eq!(stats.categories.device.dropped, 1);
    assert_eq!(stats.categories.journal.dropped, 1);
    assert_eq!(stats.shutdown_timeouts, 0);
}

#[tokio::test]
async fn normally_finished_writer_reconciles_unconsumed_events() {
    verify_finished_writer_reconciles_queue(tokio::spawn(async {})).await;
}

#[tokio::test]
async fn failed_writer_reconciles_unconsumed_events() {
    let task = tokio::spawn(std::future::pending::<()>());
    task.abort();
    verify_finished_writer_reconciles_queue(task).await;
}
