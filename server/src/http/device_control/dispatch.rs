use axum::extract::ws::{Message as WebSocketMessage, WebSocket};
use prost::Message as _;
use tokio::sync::watch;

use crate::{application::command, db::Database};

use super::{
    ConnectionCloseReason, ConnectionFlow,
    render::{self, RenderError},
    session::send_server_drain,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DispatchRetry {
    /// The last pass completed, so heartbeats must not resend durable non-terminal commands.
    Clear,
    /// The last pass could not read durable rows, so a later heartbeat owes one retry attempt.
    Owed,
}

/// Owns command-dispatch retry state for one steady Device connection.
///
/// Session entry and explicit dispatch notifications always execute a pass. Heartbeats only
/// retry a pass whose database read failed, preventing periodic redelivery after success.
pub(super) struct CommandDispatcher {
    retry: DispatchRetry,
}

impl CommandDispatcher {
    pub(super) const fn new() -> Self {
        Self {
            retry: DispatchRetry::Clear,
        }
    }

    /// Immediately executes a pass and replaces retry state with that pass's outcome.
    ///
    /// Steady-session entry and a dispatch notification both use this path because each is an
    /// independent reason to inspect the durable command queue now.
    pub(super) async fn dispatch_now(
        &mut self,
        socket: &mut WebSocket,
        database: &Database,
        device_pk: &str,
        eviction: &mut watch::Receiver<bool>,
    ) -> ConnectionFlow {
        self.run_pass(socket, database, device_pk, eviction).await
    }

    /// Retries one failed-read pass on a heartbeat, or stays idle when no retry is owed.
    ///
    /// A successful retry clears the debt through `run_pass`; another read failure leaves it
    /// owed for the next heartbeat.
    pub(super) async fn retry_if_owed(
        &mut self,
        socket: &mut WebSocket,
        database: &Database,
        device_pk: &str,
        eviction: &mut watch::Receiver<bool>,
    ) -> ConnectionFlow {
        match self.retry {
            DispatchRetry::Clear => ConnectionFlow::Continue,
            DispatchRetry::Owed => self.run_pass(socket, database, device_pk, eviction).await,
        }
    }

    async fn run_pass(
        &mut self,
        socket: &mut WebSocket,
        database: &Database,
        device_pk: &str,
        eviction: &mut watch::Receiver<bool>,
    ) -> ConnectionFlow {
        match dispatch_commands(socket, database, device_pk, eviction).await {
            DispatchOutcome::Completed => {
                self.retry = DispatchRetry::Clear;
                ConnectionFlow::Continue
            }
            DispatchOutcome::RetryOwed => {
                self.retry = DispatchRetry::Owed;
                ConnectionFlow::Continue
            }
            DispatchOutcome::Close(reason) => ConnectionFlow::Close(reason),
        }
    }
}

/// The result of one dispatch pass.
///
/// `RetryOwed` is what lets the heartbeat stay quiet in the common case: a pass is only
/// re-run on the tick when a previous one failed to read its rows, so a device whose commands
/// simply sit in a non-terminal state is not re-sent the whole batch every heartbeat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DispatchOutcome {
    /// Every dispatchable row was rendered and sent.
    Completed,
    /// The rows could not be read; they remain durable and a retry is owed.
    RetryOwed,
    Close(ConnectionCloseReason),
}

async fn dispatch_commands(
    socket: &mut WebSocket,
    database: &Database,
    device_pk: &str,
    eviction: &mut watch::Receiver<bool>,
) -> DispatchOutcome {
    let Ok(rows) = command::list_dispatchable_commands(database, device_pk).await else {
        // A transient read failure must not disconnect a healthy device; the heartbeat retries
        // against the same durable rows.
        tracing::error!("Device command dispatch query failed; identifiers redacted");
        return DispatchOutcome::RetryOwed;
    };
    for row in rows {
        let envelope = match render::render_wire_command(&row) {
            Ok(envelope) => envelope,
            Err(RenderError::HeldByPhasePolicy) => {
                tracing::debug!(
                    "Phase 4 held a sync_secret command from dispatch; identifiers redacted"
                );
                continue;
            }
            Err(
                error @ (RenderError::PayloadVersionUnsupported
                | RenderError::PayloadCorrupt
                | RenderError::TimestampCorrupt),
            ) => {
                tracing::error!(
                    render_error = ?error,
                    "Persisted command rendering invariant failed; command facts redacted"
                );
                continue;
            }
        };
        // Revocation must stay immediate: a device that stalls its receive window cannot be
        // allowed to defer eviction until the whole batch drains.
        let send = tokio::select! {
            biased;
            changed = eviction.changed() => {
                if changed.is_ok() {
                    send_server_drain(socket).await;
                    return DispatchOutcome::Close(ConnectionCloseReason::Evicted);
                }
                return DispatchOutcome::Close(ConnectionCloseReason::TransportFailed);
            }
            send = socket.send(WebSocketMessage::binary(envelope.encode_to_vec())) => send,
        };
        if send.is_err() {
            return DispatchOutcome::Close(ConnectionCloseReason::TransportFailed);
        }
    }
    DispatchOutcome::Completed
}
