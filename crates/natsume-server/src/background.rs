use snafu::{IntoError as _, ResultExt as _, Snafu};
use tokio::{sync::oneshot, task::JoinSet};
use tokio_util::sync::{CancellationToken, DropGuard};
use tracing::{Instrument as _, instrument::WithSubscriber as _};

use crate::{component::print::ExpiryError, server_state::ServerState};

#[derive(Debug, Snafu)]
pub(crate) enum BackgroundError {
    #[snafu(display("background task stopped unexpectedly"))]
    Task { source: tokio::task::JoinError },
    #[snafu(display("print expiry operation failed"))]
    PrintExpiry { source: ExpiryError },
    #[snafu(display("print expiry did not initialize"))]
    PrintInitialization { source: oneshot::error::RecvError },
}

/// Owns Server background tasks separately from shared state.
/// Normal shutdown cancels and joins all tasks; dropping the owner aborts them.
pub(crate) struct BackgroundTasks {
    shutdown: DropGuard,
    tasks: JoinSet<Result<(), BackgroundError>>,
}

pub(crate) async fn start_background_tasks(
    state: &ServerState,
) -> Result<BackgroundTasks, BackgroundError> {
    let stop = CancellationToken::new();
    let mut background = BackgroundTasks {
        shutdown: stop.clone().drop_guard(),
        tasks: JoinSet::new(),
    };
    let (ready, initialized) = oneshot::channel();
    let expiry = state.print().maintain_attempts(stop, ready);
    background.tasks.spawn(
        async move { expiry.await.context(PrintExpirySnafu) }
            .in_current_span()
            .with_subscriber(tracing::dispatcher::get_default(Clone::clone)),
    );
    if let Err(source) = initialized.await {
        background.shutdown().await?;
        return Err(source).context(PrintInitializationSnafu);
    }
    Ok(background)
}

impl BackgroundTasks {
    pub(crate) async fn shutdown(mut self) -> Result<(), BackgroundError> {
        self.shutdown.token().cancel();
        let mut first_error = None;
        while let Some(result) = self.tasks.join_next().await {
            let error = match result {
                Ok(Ok(())) => continue,
                Ok(Err(error)) => {
                    tracing::error!(%error, "background task failed");
                    error
                }
                Err(source) => {
                    tracing::error!(
                        panicked = source.is_panic(),
                        cancelled = source.is_cancelled(),
                        "background task stopped unexpectedly"
                    );
                    TaskSnafu.into_error(source)
                }
            };
            first_error.get_or_insert(error);
        }
        first_error.map_or(Ok(()), Err)
    }
}

#[cfg(test)]
mod tests;
