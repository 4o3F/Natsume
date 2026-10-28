use std::{
    future::Future,
    sync::{Arc, Mutex},
};

use tokio::task::JoinHandle;
use tokio_util::{
    sync::CancellationToken,
    task::{TaskTracker, task_tracker::TaskTrackerToken},
};

#[derive(Clone)]
pub(crate) struct ServerLifecycle {
    /// Process-wide admission state and task ownership, shared by composition-root clones.
    shared: Arc<Shared>,
}

struct Shared {
    /// Serializes admission with shutdown; the cancellation token owns the sole stopping state.
    admission_lock: Mutex<()>,
    /// Counts admitted permits and spawned children; `begin_shutdown` closes it before waiting.
    tracker: TaskTracker,
    /// Initially uncancelled; `begin_shutdown` broadcasts cancellation under the admission lock.
    shutdown_signal: CancellationToken,
}

impl ServerLifecycle {
    pub(crate) fn new() -> Self {
        Self {
            shared: Arc::new(Shared {
                admission_lock: Mutex::new(()),
                tracker: TaskTracker::new(),
                shutdown_signal: CancellationToken::new(),
            }),
        }
    }

    pub(crate) fn admit(&self) -> Option<Admission> {
        let _guard = self
            .shared
            .admission_lock
            .lock()
            // The lock protects no mutable data; token cancellation is the only stopping state.
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.shared.shutdown_signal.is_cancelled() {
            return None;
        }
        Some(Admission {
            token: self.shared.tracker.token(),
            shutdown_signal: self.shared.shutdown_signal.clone(),
        })
    }

    pub(crate) fn begin_shutdown(&self) {
        let _guard = self
            .shared
            .admission_lock
            .lock()
            // The lock protects no mutable data; token cancellation is the only stopping state.
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.shared.tracker.close();
        self.shared.shutdown_signal.cancel();
    }

    pub(crate) async fn wait(&self) {
        self.shared.tracker.wait().await;
    }
}

#[derive(Clone)]
pub(crate) struct Admission {
    /// Keeps producer wait pending until this permit and its clones are dropped.
    token: TaskTrackerToken,
    /// Shared step-one shutdown signal; permits observe it without creating another stop state.
    shutdown_signal: CancellationToken,
}

impl Admission {
    pub(crate) async fn stopping(&self) {
        self.shutdown_signal.cancelled().await;
    }

    pub(crate) fn spawn<F>(&self, future: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.token.task_tracker().spawn(future)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_error::TestError;
    use std::time::Duration;
    use tokio::{sync::Notify, time::timeout};

    #[tokio::test]
    async fn shutdown_rejects_admission_across_clones_and_is_idempotent() {
        let lifecycle = ServerLifecycle::new();
        let shared = lifecycle.clone();
        assert!(lifecycle.admit().is_some());
        shared.begin_shutdown();
        lifecycle.begin_shutdown();
        assert!(shared.admit().is_none());
        assert!(lifecycle.admit().is_none());
        lifecycle.wait().await;
    }

    #[tokio::test]
    async fn wait_includes_held_admissions_and_their_clones() {
        let lifecycle = ServerLifecycle::new();
        let admission = lifecycle.admit().unwrap_or_else(|| panic!("admission"));
        let child = admission.clone();
        {
            let stopping = admission.stopping();
            tokio::pin!(stopping);
            assert!(futures_util::poll!(&mut stopping).is_pending());
            lifecycle.begin_shutdown();
            stopping.await;
        }
        child.stopping().await;
        let waiting = lifecycle.wait();
        tokio::pin!(waiting);
        assert!(futures_util::poll!(&mut waiting).is_pending());
        drop(admission);
        assert!(futures_util::poll!(&mut waiting).is_pending());
        drop(child);
        waiting.await;
    }

    #[tokio::test]
    async fn accepted_children_remain_tracked_after_the_admission_is_dropped()
    -> Result<(), TestError> {
        let lifecycle = ServerLifecycle::new();
        let admission = lifecycle.admit().ok_or(TestError::missing("admission"))?;
        let release = Arc::new(Notify::new());
        let released = Arc::clone(&release);
        lifecycle.begin_shutdown();
        let child = admission.spawn(async move {
            released.notified().await;
        });
        drop(admission);
        let waiting = lifecycle.wait();
        tokio::pin!(waiting);
        assert!(futures_util::poll!(&mut waiting).is_pending());
        release.notify_one();
        timeout(Duration::from_secs(2), waiting).await?;
        child.await?;
        Ok(())
    }
}
