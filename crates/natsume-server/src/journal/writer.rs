use std::{
    path::{Path, PathBuf},
    sync::{Arc, atomic::Ordering},
    time::Duration,
};

use snafu::{ResultExt as _, Snafu};
use tokio::{
    sync::{mpsc, watch},
    task::JoinHandle,
    time::{Instant, MissedTickBehavior},
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{
    CategoryEntry, Envelope, ErrorClass, JournalContext, JournalStats, PerCategory,
    budget::{self, BudgetDecision, BudgetLimits, DiskUsage, PauseState},
    db::{JournalDatabase, PRUNE_CHUNK, Prune, StoredEvent},
    event::{
        Event, EventError,
        payload::{JournalDropped, JournalPaused, JournalResumed, ServerStopping},
        types::{Category, PauseReason},
    },
    now_ms,
};
use crate::{config::JournalConfig, db::PersistenceError};

/// Maximum events dequeued together, bounding each writer transaction.
const BATCH_SIZE: usize = 256;
/// Milliseconds in a day, converting configured retention days to Unix milliseconds.
const MS_PER_DAY: i64 = 86_400_000;
/// Transient retries following the initial write attempt.
const WRITE_RETRIES: u32 = 3;
/// Initial retry delay, doubled for each subsequent transient failure.
const RETRY_BACKOFF_BASE: Duration = Duration::from_millis(50);
/// Interval between age-retention passes, in addition to startup retention.
const RETENTION_INTERVAL: Duration = Duration::from_mins(5);
/// Interval between pruning/checkpoint attempts while disk pressure pauses writes.
const PAUSE_RECOVERY_INTERVAL: Duration = Duration::from_secs(1);
/// Minimum interval between filesystem budget samples during normal writing.
const BUDGET_CHECK_INTERVAL: Duration = Duration::from_secs(1);
/// Delay between degraded-mode attempts to reopen the journal.
const REOPEN_INTERVAL: Duration = Duration::from_mins(1);

#[derive(Clone, Copy)]
enum Loss {
    Dropped,
    WriteFailed,
}

struct BudgetSample {
    /// Writer clock at the last filesystem check, including failed checks.
    checked_at: Instant,
    /// Last budget decision; None means filesystem sampling failed and writing is refused.
    decision: Option<BudgetDecision>,
}

pub(super) fn record(boot_id: Uuid, envelope: Envelope) -> Result<StoredEvent, EventError> {
    let event = envelope.event;
    let context = envelope.context;
    Ok(StoredEvent {
        occurred_at_ms: envelope.occurred_at_ms,
        boot_id: boot_id.to_string(),
        kind: event.kind().into(),
        category: event.category().as_str().into(),
        severity: event.severity().as_str().into(),
        device_id: context.device_id.map(|id| id.as_text()),
        session_id: context.session_id.map(|id| id.value().to_string()),
        actor_operator_id: context.actor_operator_id.map(|id| id.value().to_string()),
        request_id: context.request_id.map(|id| id.value().to_string()),
        operation_id: context.operation_id.map(|id| id.value().to_string()),
        trace_id: context
            .trace_id
            .filter(|id| *id != opentelemetry::trace::TraceId::INVALID)
            .map(|id| id.to_string()),
        seat_code: context.seat_code.map(|code| code.as_str().into()),
        details: event.details()?,
    })
}

struct Writer {
    /// Immutable journal file path supplied by the component, also used for budget checks.
    path: PathBuf,
    /// Immutable retention, byte-budget and timeout settings supplied at startup.
    config: JournalConfig,
    /// Shared atomic boot counters/status, updated by producers, this writer and shutdown.
    stats: Arc<JournalStats>,
    /// Immutable process `UUIDv7` stamped onto every stored event.
    boot_id: Uuid,
    /// Open journal pool; None until startup/retry succeeds, set only by the writer.
    database: Option<JournalDatabase>,
    /// Original pause event retained by the writer until recovery; None outside a pause interval.
    pause_marker: Option<Envelope>,
    /// Boot-total drops already reported successfully per category; writer advances from zero.
    reported_drops: PerCategory<u64>,
    /// Last filesystem decision, owned by the writer; None before the first sample.
    budget_sample: Option<BudgetSample>,
}

pub(super) async fn run(
    path: PathBuf,
    config: JournalConfig,
    mut receiver: mpsc::Receiver<Envelope>,
    stats: Arc<JournalStats>,
    boot_id: Uuid,
    stop: CancellationToken,
    current_database: watch::Sender<Option<JournalDatabase>>,
) {
    let mut writer = Writer {
        path,
        config,
        stats,
        boot_id,
        database: None,
        pause_marker: None,
        reported_drops: PerCategory::default(),
        budget_sample: None,
    };
    let open_path = writer.path.clone();
    let mut opening = Opening(Some(tokio::spawn(async move {
        JournalDatabase::open(&open_path).await
    })));
    let mut retry_open = Instant::now();
    let mut retention =
        tokio::time::interval_at(Instant::now() + RETENTION_INTERVAL, RETENTION_INTERVAL);
    retention.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut recovery = tokio::time::interval(PAUSE_RECOVERY_INTERVAL);
    recovery.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut closing = false;
    let mut batch = Vec::with_capacity(BATCH_SIZE);
    loop {
        tokio::select! {
            biased;
            () = stop.cancelled(), if !closing => { closing = true; receiver.close(); },
            result = opening.wait(), if opening.0.is_some() => {
                opening.0.take();
                match result {
                    Ok(Ok(database)) => {
                        writer.database = Some(database.clone());
                        current_database.send_replace(Some(database));
                        writer.stats.degraded.store(false, Ordering::Release);
                        writer.stats.last_error.store(None);
                        writer.retain().await;
                        if receiver.is_empty() { writer.report_drops().await; }
                    },
                    Ok(Err(error)) => {
                        tracing::warn!(error = %snafu::Report::from_error(error), "journal unavailable; serving in degraded mode");
                        writer.stats.last_error.store(Some(ErrorClass::Open));
                    },
                    Err(source) => {
                        tracing::warn!(error = %snafu::Report::from_error(crate::task::TaskFailure::from(source)), "journal open task failed");
                        writer.stats.last_error.store(Some(ErrorClass::Open));
                    },
                }
                retry_open = Instant::now() + REOPEN_INTERVAL;
            },
            () = tokio::time::sleep_until(retry_open), if writer.database.is_none() && opening.0.is_none() && !closing => {
                let path = writer.path.clone();
                opening.0 = Some(tokio::spawn(async move { JournalDatabase::open(&path).await }));
            },
            _ = retention.tick(), if writer.database.is_some() => { writer.retain().await; },
            _ = recovery.tick(), if writer.stats.pause_reason.load().is_some() => {
                writer.check_budget().await;
                if receiver.is_empty() { writer.report_drops().await; }
            },
            count = receiver.recv_many(&mut batch, BATCH_SIZE), if opening.0.is_none() => {
                if count == 0 { break; }
                writer.stats.queue_depth.fetch_sub(count, Ordering::Relaxed);
                writer.batch(std::mem::take(&mut batch)).await;
                if receiver.is_empty() { writer.report_drops().await; }
            },
        }
    }
    if writer.database.is_some() && writer.check_budget().await {
        writer.report_drops().await;
        writer
            .direct(
                Event::ServerStopping(ServerStopping {
                    lost_on_flush: None,
                }),
                now_ms(),
            )
            .await;
    }
}

struct Opening(
    /// Open task owned by the writer; None between attempts, aborted when this owner drops.
    Option<JoinHandle<Result<JournalDatabase, crate::db::DatabaseError>>>,
);
impl Opening {
    async fn wait(
        &mut self,
    ) -> Result<Result<JournalDatabase, crate::db::DatabaseError>, tokio::task::JoinError> {
        match self.0.as_mut() {
            Some(task) => task.await,
            None => std::future::pending().await,
        }
    }
}
impl Drop for Opening {
    fn drop(&mut self) {
        if let Some(task) = &self.0 {
            task.abort();
        }
    }
}

impl Writer {
    fn lost(&self, category: &Category, count: u64, at: i64, loss: Loss) {
        self.stats
            .categories
            .get(category)
            .queued
            .fetch_sub(count, Ordering::Relaxed);
        self.stats.drop_events(category, count, at);
        if matches!(loss, Loss::WriteFailed) {
            self.stats.write_failed.fetch_add(count, Ordering::Relaxed);
        }
    }

    async fn batch(&mut self, events: Vec<Envelope>) {
        let writable = self.database.is_some() && self.check_budget().await;
        let mut rows = Vec::new();
        let mut counts = PerCategory::<u64>::default();
        for envelope in events {
            let category = envelope.event.category();
            let at = envelope.occurred_at_ms;
            if matches!(envelope.event, Event::ServerStopping(_)) {
                self.stats
                    .categories
                    .get(&category)
                    .queued
                    .fetch_sub(1, Ordering::Relaxed);
                continue;
            }
            if !writable {
                self.lost(&category, 1, at, Loss::Dropped);
                continue;
            }
            match record(self.boot_id, envelope) {
                Ok(row) => {
                    rows.push(row);
                    *counts.get_mut(&category) += 1;
                }
                Err(error) => {
                    tracing::warn!(error = %snafu::Report::from_error(error), "journal event rejected");
                    self.stats.last_error.store(Some(ErrorClass::Serialization));
                    self.lost(&category, 1, at, Loss::WriteFailed);
                }
            }
        }
        if rows.is_empty() {
            return;
        }
        let result = self.insert(&rows).await;
        for CategoryEntry {
            category,
            value: count,
        } in counts.iter()
        {
            let count = *count;
            if result {
                self.stats
                    .categories
                    .get(&category)
                    .queued
                    .fetch_sub(count, Ordering::Relaxed);
            } else {
                self.lost(&category, count, now_ms(), Loss::WriteFailed);
            }
        }
        self.retain_rows().await;
    }

    async fn insert(&self, rows: &[StoredEvent]) -> bool {
        let Some(database) = &self.database else {
            return false;
        };
        for attempt in 0..=WRITE_RETRIES {
            match database.insert_batch(rows).await {
                Ok(_) => return true,
                Err(error) => {
                    self.stats.last_error.store(Some(ErrorClass::Write));
                    tracing::warn!(
                        error = %snafu::Report::from_error(&error),
                        transient = error.is_transient(),
                        attempt,
                        "journal batch write failed"
                    );
                    if !error.is_transient() || attempt == WRITE_RETRIES {
                        return false;
                    }
                    tokio::time::sleep(RETRY_BACKOFF_BASE * (1 << attempt)).await;
                }
            }
        }
        false
    }

    async fn direct(&self, event: Event, at: i64) -> bool {
        let category = event.category();
        let envelope = Envelope {
            event,
            context: JournalContext::default(),
            occurred_at_ms: at,
        };
        match record(self.boot_id, envelope) {
            Ok(row) if self.insert(std::slice::from_ref(&row)).await => return true,
            Ok(_) => {}
            Err(error) => {
                self.stats.last_error.store(Some(ErrorClass::Serialization));
                tracing::warn!(error = %snafu::Report::from_error(&error), "journal health event rejected");
            }
        }
        self.stats.drop_events(&category, 1, at);
        self.stats.write_failed.fetch_add(1, Ordering::Relaxed);
        false
    }

    async fn report_drops(&mut self) {
        if self.database.is_none() || !self.check_budget().await {
            return;
        }
        let stats = Arc::clone(&self.stats);
        for CategoryEntry {
            category,
            value: counters,
        } in stats.categories.iter()
        {
            let total = counters.dropped.load(Ordering::Acquire);
            let count = total.saturating_sub(*self.reported_drops.get(&category));
            if let Some(since) = counters.first_unreported_drop_ms.load()
                && count != 0
                && self
                    .direct(
                        Event::JournalDropped(JournalDropped {
                            count,
                            since,
                            category: category.clone(),
                        }),
                        now_ms(),
                    )
                    .await
            {
                *self.reported_drops.get_mut(&category) = total;
            }
        }
    }

    async fn retain(&self) {
        let Some(database) = &self.database else {
            return;
        };
        let cutoff = now_ms().saturating_sub(i64::from(self.config.max_age_days) * MS_PER_DAY);
        loop {
            match database.prune(Prune::OlderThan(cutoff)).await {
                Ok(count) if count == usize::from(PRUNE_CHUNK) => {}
                Ok(_) => break,
                Err(error) => {
                    self.maintenance_failed(&error);
                    break;
                }
            }
        }
        self.retain_rows().await;
    }

    async fn retain_rows(&self) {
        let Some(database) = &self.database else {
            return;
        };
        loop {
            match database
                .prune(Prune::RowsBeyond(self.config.max_rows))
                .await
            {
                Ok(count) if count == usize::from(PRUNE_CHUNK) => {}
                Ok(_) => break,
                Err(error) => {
                    self.maintenance_failed(&error);
                    break;
                }
            }
        }
    }

    fn maintenance_failed(&self, error: &PersistenceError) {
        self.stats.last_error.store(Some(ErrorClass::Write));
        tracing::warn!(
            error = %snafu::Report::from_error(error),
            transient = error.is_transient(),
            "journal maintenance failed"
        );
    }

    async fn check_budget(&mut self) -> bool {
        if let Some(sample) = &self.budget_sample
            && sample.checked_at.elapsed() < BUDGET_CHECK_INTERVAL
        {
            return matches!(
                sample.decision,
                Some(BudgetDecision::Normal | BudgetDecision::PruneSoon | BudgetDecision::Resume)
            );
        }
        self.refresh_budget().await
    }

    async fn refresh_budget(&mut self) -> bool {
        let path = self.path.clone();
        self.budget_sample = Some(BudgetSample {
            checked_at: Instant::now(),
            decision: None,
        });
        let sizes = tokio::task::spawn_blocking(move || disk_sizes(&path)).await
            .map_err(crate::task::TaskFailure::from)
            .inspect_err(|error| tracing::warn!(error = %snafu::Report::from_error(error), "journal budget task failed"))
            .map(|result| result.inspect_err(|error| tracing::warn!(error = %snafu::Report::from_error(error), "journal filesystem budget unavailable")));
        let Ok(Ok(usage)) = sizes else {
            self.stats.last_error.store(Some(ErrorClass::Filesystem));
            return false;
        };
        let paused = self.stats.pause_reason.load().is_some();
        let decision = budget::decide(
            usage,
            BudgetLimits {
                max_bytes: self.config.max_bytes,
                min_free_bytes: self.config.min_free_bytes,
            },
            if paused {
                PauseState::Paused
            } else {
                PauseState::Running
            },
        );
        self.budget_sample = Some(BudgetSample {
            checked_at: Instant::now(),
            decision: Some(decision),
        });
        match decision {
            BudgetDecision::Normal => true,
            BudgetDecision::PruneSoon => {
                self.retain().await;
                if let Some(database) = &self.database {
                    if let Err(error) = database.prune(Prune::OldestBatch).await {
                        self.maintenance_failed(&error);
                    }
                    if let Err(error) = database.maintenance(false).await {
                        self.maintenance_failed(&error);
                    }
                }
                true
            }
            BudgetDecision::Paused => {
                if !paused {
                    self.stats
                        .pause_reason
                        .store(Some(&PauseReason::DiskBudget));
                    self.pause_marker = Some(Envelope {
                        event: Event::JournalPaused(JournalPaused {
                            reason: PauseReason::DiskBudget,
                        }),
                        context: JournalContext::default(),
                        occurred_at_ms: now_ms(),
                    });
                }
                if let Some(database) = &self.database {
                    if let Err(error) = database.prune(Prune::OldestBatch).await {
                        self.maintenance_failed(&error);
                    }
                    if let Err(error) = database.maintenance(true).await {
                        self.maintenance_failed(&error);
                    }
                }
                false
            }
            BudgetDecision::Resume => {
                if let Some(marker) = self.pause_marker.take() {
                    self.direct(marker.event, marker.occurred_at_ms).await;
                }
                self.direct(Event::JournalResumed(JournalResumed {}), now_ms())
                    .await;
                self.stats.pause_reason.store(None);
                true
            }
        }
    }
}

#[derive(Debug, Snafu)]
enum DiskError {
    #[snafu(display("cannot read journal file sizes"))]
    Io {
        /// Original metadata error from sampling the journal or a sidecar.
        source: std::io::Error,
    },
    #[snafu(display("cannot read journal filesystem space"))]
    Space {
        /// Original filesystem-space failure, preserving the underlying statvfs error.
        source: budget::BudgetError,
    },
}

fn disk_sizes(path: &Path) -> Result<DiskUsage, DiskError> {
    fn size(path: &Path) -> Result<u64, DiskError> {
        match std::fs::metadata(path) {
            Ok(metadata) => Ok(metadata.len()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
            Err(source) => Err(source).context(IoSnafu),
        }
    }
    let mut wal = path.as_os_str().to_owned();
    wal.push("-wal");
    let mut shm = path.as_os_str().to_owned();
    shm.push("-shm");
    Ok(DiskUsage {
        db_bytes: size(path)?,
        wal_bytes: size(Path::new(&wal))?,
        shm_bytes: size(Path::new(&shm))?,
        free_bytes: budget::free_bytes(path.parent().unwrap_or(path)).context(SpaceSnafu)?,
    })
}

#[cfg(test)]
mod tests;
