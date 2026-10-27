use std::{fmt::Debug, future::Future, time::Duration};

use snafu::ResultExt as _;
use tokio::{sync::oneshot, task::JoinSet};
use tokio_util::sync::CancellationToken;

use super::{BackgroundError, BackgroundTasks, PrintExpirySnafu};
use crate::component::print::ExpiryError;

fn must<T, E: Debug>(result: Result<T, E>) -> T {
    result.unwrap_or_else(|error| panic!("background fixture failed: {error:?}"))
}

fn tasks(
    stop: CancellationToken,
    worker: impl Future<Output = ()> + Send + 'static,
) -> BackgroundTasks {
    let mut tasks = JoinSet::new();
    tasks.spawn(async move {
        worker.await;
        Ok(())
    });
    BackgroundTasks {
        shutdown: stop.drop_guard(),
        tasks,
    }
}

#[tokio::test]
async fn shutdown_signals_cancellation_and_waits_for_every_task() {
    let stop = CancellationToken::new();
    let worker_stop = stop.clone();
    let (observed, observation) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let mut background = tasks(stop.clone(), async move {
        worker_stop.cancelled().await;
        must(observed.send(()));
        must(released.await);
    });
    let (second_observed, second_observation) = oneshot::channel();
    let (second_release, second_released) = oneshot::channel();
    background.tasks.spawn(async move {
        stop.cancelled().await;
        must(second_observed.send(()));
        must(second_released.await);
        Ok(())
    });
    let mut closing = Box::pin(background.shutdown());
    assert!(futures_util::poll!(closing.as_mut()).is_pending());
    must(must(
        tokio::time::timeout(Duration::from_secs(5), observation).await,
    ));
    must(must(
        tokio::time::timeout(Duration::from_secs(5), second_observation).await,
    ));
    assert!(futures_util::poll!(closing.as_mut()).is_pending());
    must(release.send(()));
    assert!(futures_util::poll!(closing.as_mut()).is_pending());
    must(second_release.send(()));
    must(closing.await);
}

#[tokio::test]
async fn dropping_owner_cancels_and_aborts_every_uncooperative_task() {
    let stop = CancellationToken::new();
    let (mut sender, receiver) = oneshot::channel::<()>();
    let mut background = tasks(stop.clone(), async move {
        let _receiver = receiver;
        std::future::pending::<()>().await;
    });
    let (mut second_sender, second_receiver) = oneshot::channel::<()>();
    background.tasks.spawn(async move {
        let _receiver = second_receiver;
        std::future::pending::<Result<(), BackgroundError>>().await
    });
    drop(background);
    assert!(stop.is_cancelled());
    must(tokio::time::timeout(Duration::from_secs(5), sender.closed()).await);
    must(tokio::time::timeout(Duration::from_secs(5), second_sender.closed()).await);
}

#[tokio::test]
async fn cancelling_shutdown_does_not_detach_any_task() {
    let stop = CancellationToken::new();
    let (mut sender, receiver) = oneshot::channel::<()>();
    let mut background = tasks(stop.clone(), async move {
        let _receiver = receiver;
        std::future::pending::<()>().await;
    });
    let (mut second_sender, second_receiver) = oneshot::channel::<()>();
    background.tasks.spawn(async move {
        let _receiver = second_receiver;
        std::future::pending::<Result<(), BackgroundError>>().await
    });
    let mut closing = Box::pin(background.shutdown());
    assert!(futures_util::poll!(closing.as_mut()).is_pending());
    assert!(stop.is_cancelled());
    drop(closing);
    must(tokio::time::timeout(Duration::from_secs(5), sender.closed()).await);
    must(tokio::time::timeout(Duration::from_secs(5), second_sender.closed()).await);
}

#[tokio::test]
async fn shutdown_preserves_task_panic_as_a_typed_error() {
    let background = tasks(CancellationToken::new(), async {
        panic!("injected background task failure");
    });
    let Err(BackgroundError::Task { source }) = background.shutdown().await else {
        panic!("expected background task failure");
    };
    assert_eq!(source, crate::task::TaskFailure::Panicked);
}

#[tokio::test]
async fn shutdown_waits_for_other_tasks_after_an_operation_error_and_keeps_the_first_error() {
    let stop = CancellationToken::new();
    let worker_stop = stop.clone();
    let (release, released) = oneshot::channel();
    let mut background = tasks(stop, async move {
        worker_stop.cancelled().await;
        must(released.await);
        panic!("injected later background task failure");
    });
    let (failed, failure) = oneshot::channel();
    background.tasks.spawn(async move {
        must(failed.send(()));
        Err(ExpiryError::InboxUnavailable {
            source: std::sync::PoisonError::new(()),
        })
        .context(PrintExpirySnafu)
    });
    must(must(
        tokio::time::timeout(Duration::from_secs(5), failure).await,
    ));
    let mut closing = Box::pin(background.shutdown());
    assert!(futures_util::poll!(closing.as_mut()).is_pending());
    must(release.send(()));
    assert!(matches!(
        closing.await,
        Err(BackgroundError::PrintExpiry {
            source: ExpiryError::InboxUnavailable { .. }
        })
    ));
}

#[tokio::test]
async fn shutdown_waits_for_other_tasks_after_a_panic() {
    let stop = CancellationToken::new();
    let worker_stop = stop.clone();
    let (release, released) = oneshot::channel();
    let (finished, completion) = oneshot::channel();
    let mut background = tasks(stop, async move {
        worker_stop.cancelled().await;
        must(released.await);
        must(finished.send(()));
    });
    let (failed, failure) = oneshot::channel();
    background.tasks.spawn(async move {
        must(failed.send(()));
        panic!("injected background task failure");
    });
    must(must(
        tokio::time::timeout(Duration::from_secs(5), failure).await,
    ));
    let mut closing = Box::pin(background.shutdown());
    assert!(futures_util::poll!(closing.as_mut()).is_pending());
    must(release.send(()));
    let Err(BackgroundError::Task { source }) = closing.await else {
        panic!("expected background task failure");
    };
    assert_eq!(source, crate::task::TaskFailure::Panicked);
    must(completion.await);
}

#[tokio::test]
async fn shutdown_accepts_an_empty_task_set() {
    let stop = CancellationToken::new();
    let background = BackgroundTasks {
        shutdown: stop.clone().drop_guard(),
        tasks: JoinSet::new(),
    };
    must(background.shutdown().await);
    assert!(stop.is_cancelled());
}
