use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use opentelemetry::trace::TraceContextExt as _;
use tokio::{
    sync::{Mutex, mpsc, watch},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;
use tracing::instrument::WithSubscriber as _;
use tracing_opentelemetry::OpenTelemetrySpanExt as _;
use uuid::Uuid;

use crate::{component::device::DeviceId, config::JournalConfig};
use event::{
    Event,
    payload::{ServerStarted, ServerStopping},
    types::{Category, OperationId, OperatorId, PauseReason, RequestId, SeatCode, SessionId},
};

mod budget;
mod db;
pub(crate) mod event;
mod writer;

/// Maximum queued events before synchronous producers start counting drops.
pub(crate) const CAPACITY: usize = 8192;

/// Maximum extra shutdown time spent attempting the terminal loss marker.
const LOSS_MARKER_TIMEOUT: Duration = Duration::from_millis(250);

#[derive(Clone, Default)]
pub(crate) struct JournalContext {
    /// Request UUID supplied explicitly by the producer; None for non-request events.
    request_id: Option<RequestId>,
    /// Authenticated actor supplied by the producer; None when no operator is known.
    actor_operator_id: Option<OperatorId>,
    /// Operation UUID supplied by the producer; None when no operation applies.
    operation_id: Option<OperationId>,
    /// Set only by `with_current_trace` from a valid current `OTel` context; otherwise None.
    trace_id: Option<opentelemetry::trace::TraceId>,
    /// Device identity supplied by the producer; None for events without a device.
    device_id: Option<DeviceId>,
    /// Lease UUID supplied by the producer; None for events without a session.
    session_id: Option<SessionId>,
    /// Committed binding snapshot supplied by the producer; None when unavailable.
    seat_code: Option<SeatCode>,
}

impl JournalContext {
    #[cfg_attr(not(test), expect(dead_code, reason = "request contexts in S4/S5"))]
    pub(crate) fn with_request(mut self, value: RequestId) -> Self {
        self.request_id = Some(value);
        self
    }

    #[cfg_attr(not(test), expect(dead_code, reason = "request contexts in S4/S5"))]
    pub(crate) fn with_actor(mut self, value: OperatorId) -> Self {
        self.actor_operator_id = Some(value);
        self
    }

    #[cfg_attr(not(test), expect(dead_code, reason = "request contexts in S4/S5"))]
    pub(crate) fn with_operation(mut self, value: OperationId) -> Self {
        self.operation_id = Some(value);
        self
    }

    #[cfg_attr(not(test), expect(dead_code, reason = "request contexts in S4/S5"))]
    pub(crate) fn with_device(mut self, value: DeviceId) -> Self {
        self.device_id = Some(value);
        self
    }

    #[cfg_attr(not(test), expect(dead_code, reason = "request contexts in S4/S5"))]
    pub(crate) fn with_session(mut self, value: SessionId) -> Self {
        self.session_id = Some(value);
        self
    }

    #[cfg_attr(not(test), expect(dead_code, reason = "request contexts in S4/S5"))]
    pub(crate) fn with_seat(mut self, value: SeatCode) -> Self {
        self.seat_code = Some(value);
        self
    }

    pub(crate) fn with_current_trace(mut self) -> Self {
        let context = tracing::Span::current().context();
        let span = context.span();
        let span_context = span.span_context();
        if span_context.is_valid() {
            self.trace_id = Some(span_context.trace_id());
        }
        self
    }
}

pub(crate) struct Envelope {
    /// Typed event moved in by emit or created by writer/runtime for journal-health rows.
    event: Event,
    /// Explicit producer facts, immutable after enqueue; absent facts use None.
    context: JournalContext,
    /// Unix milliseconds captured at emission; pre-epoch clock errors use epoch zero.
    occurred_at_ms: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ErrorClass {
    Open,
    Write,
    Serialization,
    Filesystem,
    FlushTimeout,
    ProducerTimeout,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct PerCategory<T> {
    /// Device-category state, maintained by the owner of this grouping.
    pub(crate) device: T,
    /// Operator-audit state, maintained by the owner of this grouping.
    pub(crate) audit: T,
    /// Authentication-category state, maintained by the owner of this grouping.
    pub(crate) auth: T,
    /// Journal-health state, maintained by the owner of this grouping.
    pub(crate) journal: T,
}

impl<T> PerCategory<T> {
    fn get(&self, category: &Category) -> &T {
        match category {
            Category::Device => &self.device,
            Category::Audit => &self.audit,
            Category::Auth => &self.auth,
            Category::Journal => &self.journal,
        }
    }

    fn get_mut(&mut self, category: &Category) -> &mut T {
        match category {
            Category::Device => &mut self.device,
            Category::Audit => &mut self.audit,
            Category::Auth => &mut self.auth,
            Category::Journal => &mut self.journal,
        }
    }

    fn iter(&self) -> impl Iterator<Item = CategoryEntry<'_, T>> {
        std::iter::once(CategoryEntry {
            category: Category::Device,
            value: &self.device,
        })
        .chain(std::iter::once(CategoryEntry {
            category: Category::Audit,
            value: &self.audit,
        }))
        .chain(std::iter::once(CategoryEntry {
            category: Category::Auth,
            value: &self.auth,
        }))
        .chain(std::iter::once(CategoryEntry {
            category: Category::Journal,
            value: &self.journal,
        }))
    }
}

struct CategoryEntry<'a, T> {
    /// Named category associated with this borrowed value.
    category: Category,
    /// Borrowed state belonging to the category, never indexed by a numeric position.
    value: &'a T,
}

struct AtomicErrorClass {
    /// Private error encoding, updated by writer/runtime stores; 0 means no error.
    inner: AtomicU8,
}

impl AtomicErrorClass {
    const fn new(value: Option<ErrorClass>) -> Self {
        Self {
            inner: AtomicU8::new(Self::encode(value)),
        }
    }

    const fn encode(value: Option<ErrorClass>) -> u8 {
        match value {
            None => 0,
            Some(ErrorClass::Open) => 1,
            Some(ErrorClass::Write) => 2,
            Some(ErrorClass::Serialization) => 3,
            Some(ErrorClass::Filesystem) => 4,
            Some(ErrorClass::FlushTimeout) => 5,
            Some(ErrorClass::ProducerTimeout) => 6,
        }
    }

    fn load(&self) -> Option<ErrorClass> {
        match self.inner.load(Ordering::Relaxed) {
            0 => None,
            1 => Some(ErrorClass::Open),
            2 => Some(ErrorClass::Write),
            3 => Some(ErrorClass::Serialization),
            4 => Some(ErrorClass::Filesystem),
            5 => Some(ErrorClass::FlushTimeout),
            6 => Some(ErrorClass::ProducerTimeout),
            _ => unreachable!("invalid private journal error encoding"),
        }
    }

    fn store(&self, value: Option<ErrorClass>) {
        self.inner.store(Self::encode(value), Ordering::Relaxed);
    }
}

struct AtomicPauseReason {
    /// Private pause encoding, updated by writer stores; 0 is None, 1 is `DiskBudget`.
    inner: AtomicU8,
}

impl AtomicPauseReason {
    const fn new(value: Option<&PauseReason>) -> Self {
        Self {
            inner: AtomicU8::new(Self::encode(value)),
        }
    }

    const fn encode(value: Option<&PauseReason>) -> u8 {
        match value {
            None => 0,
            Some(PauseReason::DiskBudget) => 1,
        }
    }

    fn load(&self) -> Option<PauseReason> {
        match self.inner.load(Ordering::Relaxed) {
            0 => None,
            1 => Some(PauseReason::DiskBudget),
            _ => unreachable!("invalid private journal pause encoding"),
        }
    }

    fn store(&self, value: Option<&PauseReason>) {
        self.inner.store(Self::encode(value), Ordering::Relaxed);
    }
}

#[derive(Default)]
struct FirstDropTimestamp {
    /// Earliest Unix-ms drop bound, recorded once; None until the first dropped event.
    inner: std::sync::Mutex<Option<i64>>,
}

impl FirstDropTimestamp {
    fn load(&self) -> Option<i64> {
        // Only copying or inserting an i64 occurs under this lock, so recovery preserves validity.
        *self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn record_first(&self, timestamp_ms: i64) {
        // Inserting a Copy value cannot leave partially updated state if the lock is poisoned.
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_or_insert(timestamp_ms);
    }
}

#[derive(Default)]
struct CategoryCounters {
    /// Events lost in this boot; producers/writer increment this count, initially zero.
    dropped: AtomicU64,
    /// Conservative loss bound in Unix ms, set once by producers/writer; None before loss.
    /// Retained after reports to preserve the existing conservative gap interval.
    first_unreported_drop_ms: FirstDropTimestamp,
    /// Events awaiting a final outcome, including the in-flight batch; initially zero.
    /// Emit increments, writer completion/discard decrements, shutdown reconciles any remainder.
    queued: AtomicU64,
}

impl CategoryCounters {
    fn snapshot(&self) -> CategorySnapshot {
        CategorySnapshot {
            dropped: self.dropped.load(Ordering::Acquire),
            first_unreported_drop_ms: self.first_unreported_drop_ms.load(),
            queued: self.queued.load(Ordering::Relaxed),
        }
    }
}

struct JournalStats {
    /// Per-category event counts and loss bounds, updated by producers, writer and shutdown.
    categories: PerCategory<CategoryCounters>,
    /// Events rejected by serialization or exhausted writes; writer increments from zero.
    write_failed: AtomicU64,
    /// Channel occupancy estimate in events; emit increments/rolls back, `recv_many` decrements.
    /// Excludes the in-flight batch, which remains included in categories.*.queued.
    /// Shutdown resets it to zero; concurrent `try_send` attempts may briefly contribute.
    queue_depth: AtomicUsize,
    /// Writer's typed pause cause; None initially and after recovery.
    pause_reason: AtomicPauseReason,
    /// True initially because the journal is not yet open; writer clears after a successful open.
    degraded: AtomicBool,
    /// Last classified failure, updated by writer/runtime; None initially or after successful open.
    last_error: AtomicErrorClass,
    /// Producer-wait and flush timeouts in this boot; shutdown increments this count from zero.
    shutdown_timeouts: AtomicU64,
}

impl Default for JournalStats {
    fn default() -> Self {
        Self {
            categories: PerCategory::default(),
            write_failed: AtomicU64::new(0),
            queue_depth: AtomicUsize::new(0),
            pause_reason: AtomicPauseReason::new(None),
            degraded: AtomicBool::new(true),
            last_error: AtomicErrorClass::new(None),
            shutdown_timeouts: AtomicU64::new(0),
        }
    }
}

impl JournalStats {
    fn drop_events(&self, category: &Category, count: u64, since: i64) {
        if count == 0 {
            return;
        }
        let counters = self.categories.get(category);
        counters.first_unreported_drop_ms.record_first(since);
        counters.dropped.fetch_add(count, Ordering::Release);
    }

    fn producer_timeout(&self) {
        self.shutdown_timeouts.fetch_add(1, Ordering::Relaxed);
        self.last_error.store(Some(ErrorClass::ProducerTimeout));
    }
}

pub(super) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
        })
}

#[derive(Clone)]
pub(crate) struct Journal {
    /// Bounded channel endpoint, created by the component and cloned for producers.
    sender: mpsc::Sender<Envelope>,
    /// Shared boot counters/status, updated by emit, writer and shutdown.
    stats: Arc<JournalStats>,
    /// Emission closure flag, initially false; runtime sets true before draining or aborting.
    closed: Arc<AtomicBool>,
    /// Immutable `UUIDv7` generated by channel creation, identifying this process boot.
    boot_id: Uuid,
}

impl Journal {
    fn channel(capacity: usize) -> (Self, mpsc::Receiver<Envelope>) {
        let (sender, receiver) = mpsc::channel(capacity);
        (
            Self {
                sender,
                stats: Arc::new(JournalStats::default()),
                closed: Arc::new(AtomicBool::new(false)),
                boot_id: Uuid::now_v7(),
            },
            receiver,
        )
    }

    pub(crate) fn emit(&self, event: Event, context: JournalContext) {
        let occurred_at_ms = now_ms();
        let category = event.category();
        if self.closed.load(Ordering::Acquire) {
            self.stats.drop_events(&category, 1, occurred_at_ms);
            return;
        }
        let counters = self.stats.categories.get(&category);
        self.stats.queue_depth.fetch_add(1, Ordering::Relaxed);
        counters.queued.fetch_add(1, Ordering::Relaxed);
        if self
            .sender
            .try_send(Envelope {
                event,
                context,
                occurred_at_ms,
            })
            .is_err()
        {
            let _ = self.stats.queue_depth.fetch_update(
                Ordering::Relaxed,
                Ordering::Relaxed,
                |value| Some(value.saturating_sub(1)),
            );
            let _ = counters
                .queued
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                    Some(value.saturating_sub(1))
                });
            self.stats.drop_events(&category, 1, occurred_at_ms);
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
#[cfg_attr(not(test), expect(dead_code, reason = "read API in S6"))]
pub(crate) struct CategorySnapshot {
    /// Captured boot-total lost events, maintained by producers/writer; zero before loss.
    pub(crate) dropped: u64,
    /// Captured Unix-ms loss bound, retained across reports; None before the first drop.
    pub(crate) first_unreported_drop_ms: Option<i64>,
    /// Captured events awaiting a final writer outcome, including any in-flight batch.
    pub(crate) queued: u64,
}

#[derive(Debug, PartialEq, Eq)]
#[cfg_attr(not(test), expect(dead_code, reason = "read API in S6"))]
pub(crate) struct JournalStatsSnapshot {
    /// Independently sampled category counters; producers/writer update their atomic sources.
    pub(crate) categories: PerCategory<CategorySnapshot>,
    /// Captured events rejected by serialization or exhausted writes; writer-maintained total.
    pub(crate) write_failed: u64,
    /// Captured channel occupancy estimate in events; excludes the in-flight writer batch.
    pub(crate) queue_depth: usize,
    /// Derived from the captured pause reason; initially false and cleared after recovery.
    pub(crate) paused: bool,
    /// Captured writer pause cause; None initially or after recovery.
    pub(crate) pause_reason: Option<PauseReason>,
    /// Captured open status; initially true because the journal is not yet open.
    pub(crate) degraded: bool,
    /// Captured last failure class, maintained by writer/runtime; None initially or after open.
    pub(crate) last_error: Option<ErrorClass>,
    /// Captured boot-total producer-wait and flush timeouts, maintained by shutdown; initially zero.
    pub(crate) shutdown_timeouts: u64,
}

pub(crate) struct JournalComponent {
    /// Shared emit-only handle; constructed once here and cloned for producers.
    emitter: Journal,
    /// Writer ownership, taken by shutdown; None for inert channels or after shutdown starts.
    runtime: Mutex<Option<WriterRuntime>>,
}

impl JournalComponent {
    #[cfg(test)]
    pub(crate) fn channel(capacity: usize) -> (Self, mpsc::Receiver<Envelope>) {
        let (emitter, receiver) = Journal::channel(capacity);
        (
            Self {
                emitter,
                runtime: Mutex::new(None),
            },
            receiver,
        )
    }

    pub(crate) fn start(path: PathBuf, config: JournalConfig) -> Self {
        let flush_timeout = Duration::from_secs(config.flush_timeout_seconds);
        let (emitter, receiver) = Journal::channel(CAPACITY);
        let journal = &emitter;
        let stop = CancellationToken::new();
        let (database, current_database) = watch::channel(None);
        let task = tokio::spawn(
            writer::run(
                path,
                config,
                receiver,
                Arc::clone(&journal.stats),
                journal.boot_id,
                stop.clone(),
                database,
            )
            .with_current_subscriber(),
        );
        let runtime = WriterRuntime {
            task,
            stop,
            journal: journal.clone(),
            flush_timeout,
            database: current_database,
        };
        let component = Self {
            emitter,
            runtime: Mutex::new(Some(runtime)),
        };
        component.emitter.emit(
            Event::ServerStarted(ServerStarted {
                boot_id: component.boot_id(),
            }),
            JournalContext::default().with_current_trace(),
        );
        component
    }

    pub(crate) fn emitter(&self) -> Journal {
        self.emitter.clone()
    }

    pub(crate) const fn boot_id(&self) -> Uuid {
        self.emitter.boot_id
    }

    #[cfg_attr(not(test), expect(dead_code, reason = "read API in S6"))]
    pub(crate) fn stats(&self) -> JournalStatsSnapshot {
        let stats = &self.emitter.stats;
        let pause_reason = stats.pause_reason.load();
        JournalStatsSnapshot {
            categories: PerCategory {
                device: stats.categories.device.snapshot(),
                audit: stats.categories.audit.snapshot(),
                auth: stats.categories.auth.snapshot(),
                journal: stats.categories.journal.snapshot(),
            },
            write_failed: stats.write_failed.load(Ordering::Relaxed),
            queue_depth: stats.queue_depth.load(Ordering::Relaxed),
            paused: pause_reason.is_some(),
            pause_reason,
            degraded: stats.degraded.load(Ordering::Acquire),
            last_error: stats.last_error.load(),
            shutdown_timeouts: stats.shutdown_timeouts.load(Ordering::Relaxed),
        }
    }

    pub(crate) fn record_producer_timeout(&self) {
        self.emitter.stats.producer_timeout();
    }

    pub(crate) async fn shutdown(&self) {
        let runtime = self.runtime.lock().await.take();
        if let Some(runtime) = runtime {
            runtime.shutdown().await;
        }
    }
}

struct WriterRuntime {
    /// Writer task started by the component; shutdown joins or aborts it at the deadline.
    task: JoinHandle<()>,
    /// Explicit drain signal, cancelled by shutdown/Drop independently of producer handles.
    stop: CancellationToken,
    /// Runtime-owned producer handle for stopping emission and shared loss accounting.
    journal: Journal,
    /// Maximum drain duration, configured in seconds; the default is ten seconds.
    flush_timeout: Duration,
    /// Writer-published open database; None until open succeeds, used for the best-effort loss marker.
    database: watch::Receiver<Option<db::JournalDatabase>>,
}

impl Drop for WriterRuntime {
    fn drop(&mut self) {
        self.journal.closed.store(true, Ordering::Release);
        self.stop.cancel();
        self.task.abort();
    }
}

impl WriterRuntime {
    fn reconcile_queued(&self) -> u64 {
        let mut lost = 0;
        for CategoryEntry {
            category,
            value: counters,
        } in self.journal.stats.categories.iter()
        {
            let count = counters.queued.swap(0, Ordering::Relaxed);
            self.journal.stats.drop_events(&category, count, now_ms());
            lost += count;
        }
        self.journal.stats.queue_depth.store(0, Ordering::Relaxed);
        lost
    }

    async fn shutdown(mut self) {
        self.journal.emit(
            Event::ServerStopping(ServerStopping {
                lost_on_flush: None,
            }),
            JournalContext::default().with_current_trace(),
        );
        self.journal.closed.store(true, Ordering::Release);
        self.stop.cancel();
        match tokio::time::timeout(self.flush_timeout, &mut self.task).await {
            Ok(Ok(())) => {
                self.reconcile_queued();
            }
            Ok(Err(source)) => {
                self.reconcile_queued();
                tracing::error!(error = %snafu::Report::from_error(crate::task::TaskFailure::from(source)), "journal writer failed");
            }
            Err(_) => {
                self.task.abort();
                let _ = (&mut self.task).await;
                let lost = self.reconcile_queued();
                self.journal
                    .stats
                    .shutdown_timeouts
                    .fetch_add(1, Ordering::Relaxed);
                self.journal
                    .stats
                    .last_error
                    .store(Some(ErrorClass::FlushTimeout));
                tracing::warn!(lost_on_flush = lost, "journal flush timed out");
                let database = self.database.borrow().clone();
                if let Some(database) =
                    database.filter(|_| self.journal.stats.pause_reason.load().is_none())
                {
                    let record = writer::record(
                        self.journal.boot_id,
                        Envelope {
                            event: Event::ServerStopping(ServerStopping {
                                lost_on_flush: Some(lost),
                            }),
                            context: JournalContext::default(),
                            occurred_at_ms: now_ms(),
                        },
                    );
                    match record {
                        Ok(record) => match tokio::time::timeout(
                            LOSS_MARKER_TIMEOUT,
                            database.insert_batch(&[record]),
                        )
                        .await
                        {
                            Ok(Ok(_)) => {}
                            Ok(Err(error)) => {
                                tracing::warn!(error = %snafu::Report::from_error(&error), "journal loss marker could not be flushed");
                            }
                            Err(error) => {
                                tracing::warn!(error = %snafu::Report::from_error(&error), "journal loss marker timed out");
                            }
                        },
                        Err(error) => {
                            tracing::warn!(error = %snafu::Report::from_error(&error), "journal loss marker rejected");
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests;
