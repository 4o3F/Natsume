use std::{
    collections::{BTreeSet, HashMap},
    future::{Future, poll_fn},
    sync::{Arc, Mutex},
    task::Poll,
    time::Duration,
};

use snafu::{ResultExt as _, Snafu};
use tokio::{
    sync::{Notify, oneshot},
    time::Instant,
};
use tokio_util::{
    sync::CancellationToken,
    time::{DelayQueue, delay_queue::Key},
};
use tracing::Instrument as _;

use super::{PrintComponent, PrintError, db};
use crate::db::{Database, TransactionError};

#[cfg(test)]
mod tests;

const BATCH_SIZE: usize = 128;
const RETRY_DELAY: Duration = Duration::from_secs(1);

#[derive(Debug, Snafu)]
pub(crate) enum ExpiryError {
    #[snafu(display("print expiry inbox is unavailable"))]
    InboxUnavailable,
    #[snafu(display("print expiry initialization failed"))]
    Initialize { source: PrintError },
}

pub(super) struct CheckRequest {
    pub attempt_id: String,
    pub check_at: Instant,
}

pub(super) struct ActiveDeadline {
    pub attempt_id: String,
    pub expires_at_unix_ms: i64,
}

#[derive(Default)]
pub(super) struct Inbox {
    pending: Mutex<HashMap<String, Instant>>,
    changed: Notify,
}

impl Inbox {
    pub(super) fn schedule(&self, request: CheckRequest) -> Result<(), ExpiryError> {
        {
            let mut pending = self
                .pending
                .lock()
                .map_err(|_| ExpiryError::InboxUnavailable)?;
            pending
                .entry(request.attempt_id)
                .and_modify(|at| *at = (*at).min(request.check_at))
                .or_insert(request.check_at);
        }
        self.changed.notify_one();
        Ok(())
    }

    pub(super) fn take(&self) -> Result<Vec<CheckRequest>, ExpiryError> {
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| ExpiryError::InboxUnavailable)?;
        Ok(pending
            .extract_if(|_, _| true)
            .take(BATCH_SIZE)
            .map(|(attempt_id, check_at)| CheckRequest {
                attempt_id,
                check_at,
            })
            .collect())
    }
}

pub(super) fn check_at(sampled_at: Instant, now: i64, deadline: i64) -> Instant {
    sampled_at + Duration::from_millis(deadline.saturating_sub(now).clamp(0, 60_000).unsigned_abs())
}

struct Scheduled {
    at: Instant,
    key: Key,
}

struct Driver {
    database: Database,
    inbox: Arc<Inbox>,
    wheel: DelayQueue<String>,
    scheduled: HashMap<String, Scheduled>,
    pending: BTreeSet<String>,
}

impl Driver {
    fn new(database: Database, inbox: Arc<Inbox>) -> Self {
        Self {
            database,
            inbox,
            wheel: DelayQueue::new(),
            scheduled: HashMap::new(),
            pending: BTreeSet::new(),
        }
    }

    fn install(&mut self, request: CheckRequest) {
        if self.pending.contains(&request.attempt_id) {
            return;
        }
        if let Some(existing) = self.scheduled.get_mut(&request.attempt_id) {
            if request.check_at < existing.at {
                self.wheel.reset_at(&existing.key, request.check_at);
                existing.at = request.check_at;
            }
        } else {
            let key = self
                .wheel
                .insert_at(request.attempt_id.clone(), request.check_at);
            self.scheduled.insert(
                request.attempt_id,
                Scheduled {
                    at: request.check_at,
                    key,
                },
            );
        }
    }

    fn enqueue(&mut self, expired: tokio_util::time::delay_queue::Expired<String>) {
        let id = expired.into_inner();
        // Remove the mapping before the queue can reuse its expired key.
        self.scheduled.remove(&id);
        self.pending.insert(id);
    }

    async fn initialize(&mut self) -> Result<(), ExpiryError> {
        let (sampled_at, now, active) = self
            .database
            .read(|tx| {
                let sampled_at = Instant::now();
                let now = db::printers::now(tx)?;
                Ok::<_, PrintError>((sampled_at, now, db::execution::load_active_deadlines(tx)?))
            })
            .await
            .map_err(TransactionError::into_error)
            .context(InitializeSnafu)?;
        for deadline in active {
            self.install(CheckRequest {
                attempt_id: deadline.attempt_id,
                check_at: check_at(sampled_at, now, deadline.expires_at_unix_ms),
            });
        }
        Ok(())
    }

    async fn collect_due(&mut self) {
        poll_fn(|cx| {
            for _ in 0..BATCH_SIZE {
                match self.wheel.poll_expired(cx) {
                    Poll::Ready(Some(expired)) => self.enqueue(expired),
                    Poll::Ready(None) | Poll::Pending => break,
                }
            }
            Poll::Ready(())
        })
        .await;
    }

    async fn check_pending(&mut self) -> Result<(), PrintError> {
        let ids: Vec<_> = self.pending.iter().take(BATCH_SIZE).cloned().collect();
        let batch = ids.clone();
        let (sampled_at, now, active) = self
            .database
            .write(move |tx| {
                let sampled_at = Instant::now();
                let now = db::printers::now(tx)?;
                Ok::<_, PrintError>((
                    sampled_at,
                    now,
                    db::execution::check_attempts(tx, &batch, now)?,
                ))
            })
            .instrument(tracing::debug_span!(
                "print.expiry.check",
                attempts = ids.len()
            ))
            .await
            .map_err(TransactionError::into_error)?;
        // A failed transaction leaves the entire batch owned by pending.
        for id in ids {
            self.pending.remove(&id);
        }
        for deadline in active {
            self.install(CheckRequest {
                attempt_id: deadline.attempt_id,
                check_at: check_at(sampled_at, now, deadline.expires_at_unix_ms),
            });
        }
        Ok(())
    }

    async fn run(&mut self, stop: CancellationToken) -> Result<(), ExpiryError> {
        let mut retry_at = None;
        loop {
            if stop.is_cancelled() {
                return Ok(());
            }
            tokio::task::consume_budget().await;
            let requests = self.inbox.take()?;
            let installed = !requests.is_empty();
            for request in requests {
                self.install(request);
            }
            let retry_ready = retry_at.is_none_or(|at| Instant::now() >= at);
            if retry_ready {
                retry_at = None;
                self.collect_due().await;
                if !self.pending.is_empty() {
                    if let Err(error) = self.check_pending().await {
                        tracing::error!(%error, "print attempt expiry failed");
                        retry_at = Some(Instant::now() + RETRY_DELAY);
                    }
                    continue;
                }
            }
            if installed {
                continue;
            }
            let retry_deadline = retry_at.unwrap_or_else(Instant::now);
            tokio::select! {
                biased;
                () = stop.cancelled() => return Ok(()),
                () = self.inbox.changed.notified() => {},
                expired = poll_fn(|cx| self.wheel.poll_expired(cx)),
                    if retry_ready && !self.wheel.is_empty() => {
                    if let Some(expired) = expired {
                        self.enqueue(expired);
                    }
                },
                () = tokio::time::sleep_until(retry_deadline), if !retry_ready => {},
            }
        }
    }
}

impl PrintComponent {
    pub(crate) fn maintain_attempts(
        &self,
        stop: CancellationToken,
        ready: oneshot::Sender<()>,
    ) -> impl Future<Output = Result<(), ExpiryError>> + Send + 'static {
        let mut driver = Driver::new(self.database.clone(), Arc::clone(&self.expiry));
        async move {
            driver.initialize().await?;
            if ready.send(()).is_err() {
                return Ok(());
            }
            driver.run(stop).await
        }
    }
}
