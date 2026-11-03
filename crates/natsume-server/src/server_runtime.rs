use std::{
    future::{Future, IntoFuture},
    sync::Arc,
    time::Duration,
};

use snafu::{ResultExt as _, Snafu};
use tokio_util::task::AbortOnDropHandle;
use tracing::instrument::WithSubscriber as _;

use crate::{
    background::{BackgroundError, BackgroundTasks},
    device_control::DeviceControl,
    journal::JournalComponent,
    lifecycle::ServerLifecycle,
    task::TaskFailure,
    tls::{PeerAddress, TlsListener},
};

const PRODUCER_WAIT_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) struct ServerRuntime {
    /// Shared admission and producer tracking, closed before waiting for producers.
    lifecycle: ServerLifecycle,
    /// Shared coordinator whose actors stop before background tasks and the journal.
    device_control: Arc<DeviceControl>,
    /// Owns the journal writer until all event producers have stopped.
    journal: JournalComponent,
}

#[derive(Debug, Snafu)]
pub(crate) enum RuntimeError {
    #[snafu(display("HTTP serving failed"))]
    Http { source: std::io::Error },
    #[snafu(display("HTTP serving task failed"))]
    ServingTask {
        #[snafu(source(from(tokio::task::JoinError, TaskFailure::from)))]
        source: TaskFailure,
    },
    #[snafu(
        visibility(pub(crate)),
        display("server background task shutdown failed")
    )]
    Background { source: BackgroundError },
}

impl ServerRuntime {
    pub(crate) fn new(
        lifecycle: ServerLifecycle,
        device_control: Arc<DeviceControl>,
        journal: JournalComponent,
    ) -> Self {
        Self {
            lifecycle,
            device_control,
            journal,
        }
    }

    pub(crate) async fn serve_until<F>(
        self,
        listener: TlsListener,
        router: axum::Router,
        background: BackgroundTasks,
        shutdown: F,
    ) -> Result<(), RuntimeError>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let http_stop = tokio_util::sync::CancellationToken::new();
        let stopped = http_stop.clone();
        let mut serving = spawn_serving(
            axum::serve(
                listener,
                router.into_make_service_with_connect_info::<PeerAddress>(),
            )
            .with_graceful_shutdown(stopped.cancelled_owned())
            .into_future()
            .with_subscriber(tracing::dispatcher::get_default(Clone::clone)),
        );
        tokio::pin!(shutdown);
        let completed = tokio::select! {
            () = &mut shutdown => None,
            result = &mut serving => Some(result),
        };
        self.lifecycle.begin_shutdown();
        http_stop.cancel();
        tracing::info!("graceful shutdown initiated");
        if tokio::time::timeout(PRODUCER_WAIT_TIMEOUT, self.lifecycle.wait())
            .await
            .is_err()
        {
            self.journal.record_producer_timeout();
            tracing::warn!("in-flight mutation shutdown timed out");
        }
        self.device_control.shutdown().await;
        let background_result = background.shutdown().await.context(BackgroundSnafu);
        self.journal.shutdown().await;
        let serving_result = match completed {
            Some(result) => result,
            None => serving.await,
        };
        let serving_result = match serving_result {
            Ok(result) => result.context(HttpSnafu),
            Err(source) => Err(source).context(ServingTaskSnafu),
        };
        let result = combine_shutdown_results(serving_result, background_result);
        if result.is_ok() {
            tracing::info!("graceful shutdown completed");
        }
        result
    }

    #[cfg(test)]
    pub(crate) const fn device_control(&self) -> &Arc<DeviceControl> {
        &self.device_control
    }

    #[cfg(test)]
    pub(crate) const fn journal(&self) -> &JournalComponent {
        &self.journal
    }

    #[cfg(test)]
    pub(crate) async fn shutdown_journal(&self) {
        self.journal.shutdown().await;
    }
}

fn spawn_serving(
    serving: impl Future<Output = std::io::Result<()>> + Send + 'static,
) -> AbortOnDropHandle<std::io::Result<()>> {
    AbortOnDropHandle::new(tokio::spawn(serving))
}

fn combine_shutdown_results(
    serving: Result<(), RuntimeError>,
    background: Result<(), RuntimeError>,
) -> Result<(), RuntimeError> {
    match (serving, background) {
        (Err(serving), Err(background)) => {
            tracing::error!(error = %snafu::Report::from_error(background), "server background task shutdown failed");
            Err(serving)
        }
        (serving, background) => serving.and(background),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, Mutex},
        time::Duration,
    };
    use tokio::sync::oneshot;
    use tracing::{
        Subscriber,
        field::{Field, Visit},
        instrument::WithSubscriber as _,
    };
    use tracing_subscriber::{
        Layer, Registry,
        layer::{Context, SubscriberExt as _},
    };

    use crate::{background::BackgroundError, task::TaskFailure};

    use super::{RuntimeError, combine_shutdown_results, spawn_serving};

    #[derive(Clone, Default)]
    struct ShutdownProbe(Arc<Mutex<Vec<ShutdownEvent>>>);

    #[derive(Debug, Default)]
    struct ShutdownEvent {
        message: String,
        error: String,
    }

    impl<S: Subscriber> Layer<S> for ShutdownProbe {
        fn on_event(&self, event: &tracing::Event<'_>, _context: Context<'_, S>) {
            if event.metadata().level() == &tracing::Level::ERROR {
                let mut fields = ShutdownEvent::default();
                event.record(&mut fields);
                self.0
                    .lock()
                    .unwrap_or_else(|error| panic!("shutdown probe lock poisoned: {error}"))
                    .push(fields);
            }
        }
    }

    impl Visit for ShutdownEvent {
        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            match field.name() {
                "message" => self.message = format!("{value:?}"),
                "error" => self.error = format!("{value:?}"),
                _ => {}
            }
        }
    }

    #[tokio::test]
    async fn shutdown_results_preserve_http_error_and_log_background_error() {
        let _guard = crate::logging::tests::SubscriberTestGuard::acquire();
        let probe = ShutdownProbe::default();
        let http_error = || RuntimeError::Http {
            source: std::io::Error::new(
                std::io::ErrorKind::ConnectionAborted,
                "injected HTTP serving failure",
            ),
        };
        let background_error = || RuntimeError::Background {
            source: BackgroundError::Task {
                source: TaskFailure::Cancelled,
            },
        };
        async {
            assert!(combine_shutdown_results(Ok(()), Ok(())).is_ok());
            assert!(matches!(
                combine_shutdown_results(Err(http_error()), Ok(())),
                Err(RuntimeError::Http { source })
                    if source.kind() == std::io::ErrorKind::ConnectionAborted
                        && source.to_string() == "injected HTTP serving failure"
            ));
            assert!(matches!(
                combine_shutdown_results(Ok(()), Err(background_error())),
                Err(RuntimeError::Background {
                    source: BackgroundError::Task {
                        source: TaskFailure::Cancelled,
                    },
                })
            ));
            assert!(
                probe
                    .0
                    .lock()
                    .unwrap_or_else(|error| panic!("shutdown probe lock poisoned: {error}"))
                    .is_empty()
            );

            assert!(matches!(
                combine_shutdown_results(Err(http_error()), Err(background_error())),
                Err(RuntimeError::Http { source })
                    if source.kind() == std::io::ErrorKind::ConnectionAborted
                        && source.to_string() == "injected HTTP serving failure"
            ));
        }
        .with_subscriber(Registry::default().with(probe.clone()))
        .await;

        let events = probe
            .0
            .lock()
            .unwrap_or_else(|error| panic!("shutdown probe lock poisoned: {error}"));
        assert_eq!(
            events.len(),
            1,
            "the background failure was not logged once"
        );
        assert_eq!(events[0].message, "server background task shutdown failed");
        assert_eq!(
            events[0].error,
            snafu::Report::from_error(background_error()).to_string()
        );
    }

    #[tokio::test]
    async fn dropping_serving_owner_aborts_the_task() {
        let (held, released) = oneshot::channel::<()>();
        let (started, ready) = oneshot::channel();
        let serving = spawn_serving(async move {
            let _held = held;
            started
                .send(())
                .unwrap_or_else(|()| panic!("serving readiness receiver dropped"));
            std::future::pending::<std::io::Result<()>>().await
        });
        tokio::time::timeout(Duration::from_secs(5), ready)
            .await
            .unwrap_or_else(|error| panic!("serving task did not start: {error}"))
            .unwrap_or_else(|error| panic!("serving readiness sender dropped: {error}"));

        drop(serving);

        assert!(
            matches!(
                tokio::time::timeout(Duration::from_secs(5), released).await,
                Ok(Err(_))
            ),
            "dropping the serving owner did not release the task's sender"
        );
    }
}
