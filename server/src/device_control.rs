//! Device Control connection-bound protocol coordination.

use std::{sync::Arc, time::Duration};

use axum::extract::ws::{Message as WebSocketMessage, WebSocket};
use natsume_device_protocol::generated::{
    ClientActiveEnvelope, ClientHandshakeEnvelope, EnrollmentReviewState, EnrollmentReviewStatus,
    ServerHandshakeEnvelope, SessionReady, client_active_envelope, server_handshake_envelope,
};
use prost::Message as _;
use tokio::{sync::mpsc, time::timeout};

mod actor;
mod admission;
mod state;

pub(crate) use actor::DeviceRegistry;

use crate::{
    component::device::{ControlAuthority, EnrollmentStartOutcome, MachineHardwareId},
    server_state::ServerState,
};

use self::{
    actor::DeviceHandle,
    admission::{EnrollmentPreAuth, ProofSubmission, ProofWindow},
};

/// Maximum accepted Device Control WebSocket frame and protobuf message size.
pub(crate) const MAX_MESSAGE_BYTES: usize = 65_536;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const SEND_TIMEOUT: Duration = Duration::from_secs(5);
const OUTBOUND_CAPACITY: usize = 1;

/// Drives one upgraded WebSocket through admission, lease attachment, and the active
/// Device protocol.
///
/// Admission returns an authenticated authority, which is re-read from the Device
/// Component immediately before attachment. This rejects an authority changed while
/// admission was in progress. Every failed stage returns and drops the socket. Once
/// a lease is attached, all exits either notify the Device actor directly or pass
/// through [`run_active`], which does so before returning.
pub(crate) async fn serve_connection(mut socket: WebSocket, state: Arc<ServerState>) {
    let Some((machine_hardware_id, authority)) = admit(&mut socket, &state).await else {
        return;
    };
    if state
        .device()
        .find_current_authority(machine_hardware_id)
        .await
        .ok()
        != Some(Some(authority))
    {
        return;
    }

    let (outbound, mut outgoing) = mpsc::channel(OUTBOUND_CAPACITY);
    let Some((session_id, handle)) = state
        .device_registry()
        .attach(authority.device_id(), outbound)
        .await
    else {
        return;
    };
    if !send_handshake(
        &mut socket,
        ServerHandshakeEnvelope {
            body: Some(server_handshake_envelope::Body::SessionReady(
                SessionReady {
                    session_id: session_id.to_vec(),
                },
            )),
        },
    )
    .await
    {
        handle.disconnected(session_id).await;
        return;
    }

    run_active(socket, state, session_id, handle, &mut outgoing).await;
}

/// Performs the single proof exchange and returns the exact authority authenticated
/// for this connection.
///
/// Resume verifies against the database-selected current key. Enrollment delegates
/// replay/manual-review handling to [`admit_enrollment`]. Any malformed, timed-out,
/// rejected, or inactive path returns `None` and closes the connection.
async fn admit(
    socket: &mut WebSocket,
    state: &Arc<ServerState>,
) -> Option<(MachineHardwareId, ControlAuthority)> {
    let mut proof_window = ProofWindow::new().ok()?;
    let challenge = proof_window.server_challenge()?.clone();
    if !send_handshake(
        socket,
        ServerHandshakeEnvelope {
            body: Some(server_handshake_envelope::Body::ServerChallenge(challenge)),
        },
    )
    .await
    {
        return None;
    }
    let proof = receive_handshake(socket).await?;
    match proof_window.submit(proof).ok()? {
        ProofSubmission::Resume(resume) => {
            let machine_hardware_id = resume.machine_hardware_id();
            let authority = state
                .device()
                .find_current_authority(machine_hardware_id)
                .await
                .ok()?;
            Some((
                machine_hardware_id,
                resume.verify_authority(authority).ok()?,
            ))
        }
        ProofSubmission::Enrollment(enrollment) => {
            admit_enrollment(socket, state, enrollment).await
        }
    }
}

/// Completes the connection-local Enrollment path after candidate-key proof.
///
/// An exact committed replay skips review. A new candidate sends `PendingReview` and
/// waits for the one-shot result attached to that review. Connection failure attempts
/// to remove the review if approval has not already claimed it. Both paths send the
/// activated Device ID and require its exact `EnrollmentReady` echo before returning
/// the enabled authority.
async fn admit_enrollment(
    socket: &mut WebSocket,
    state: &Arc<ServerState>,
    enrollment: EnrollmentPreAuth,
) -> Option<(MachineHardwareId, ControlAuthority)> {
    let evidence = enrollment.review_evidence();
    let machine_hardware_id = evidence.machine_hardware_id();
    let authority = match state
        .device()
        .start_enrollment(state.provisioning(), evidence)
        .await
        .ok()?
    {
        EnrollmentStartOutcome::Replay(authority) => authority,
        EnrollmentStartOutcome::Pending(review, activation) => {
            if !send_handshake(
                socket,
                ServerHandshakeEnvelope {
                    body: Some(server_handshake_envelope::Body::EnrollmentReviewStatus(
                        EnrollmentReviewStatus {
                            state: EnrollmentReviewState::PendingReview.into(),
                            error_code: String::new(),
                        },
                    )),
                },
            )
            .await
            {
                // A pending review belongs to this proved connection. If the Client
                // cannot receive its pending state, retaining the review would leave
                // an operator-visible request with no connection able to complete
                // the activation/ready exchange.
                state
                    .device()
                    .remove_enrollment_review(review.review_id())
                    .await;
                return None;
            }
            tokio::select! {
                result = activation => result.ok()?.ok()?,
                _ = socket.recv() => {
                    state.device().remove_enrollment_review(review.review_id()).await;
                    return None;
                }
            }
        }
    };
    let barrier = enrollment.activated(authority).ok()?;
    if !send_handshake(
        socket,
        ServerHandshakeEnvelope {
            body: Some(server_handshake_envelope::Body::EnrollmentActivated(
                barrier.enrollment_activated().clone(),
            )),
        },
    )
    .await
    {
        return None;
    }
    let authority = barrier.receive(receive_handshake(socket).await?).ok()?;
    Some((machine_hardware_id, authority))
}

/// Pumps active Client snapshots to the Device actor and complete Server snapshots
/// back to this WebSocket.
///
/// The received session ID is intentionally forwarded to the actor, which owns stale
/// lease fencing. Invalid frames, closed channels, actor mailbox closure, send
/// failure, and send timeout all terminate the loop. The final disconnect event
/// carries the local attached session ID, so it cannot clear a replacement lease.
async fn run_active(
    mut socket: WebSocket,
    state: Arc<ServerState>,
    session_id: [u8; 16],
    handle: DeviceHandle,
    outgoing: &mut mpsc::Receiver<natsume_device_protocol::generated::ServerActiveEnvelope>,
) {
    loop {
        tokio::select! {
            message = socket.recv() => {
                let Some(envelope) = decode_active(message) else {
                    break;
                };
                let Ok(received_session_id) = envelope.session_id.try_into() else {
                    break;
                };
                match envelope.body {
                    Some(client_active_envelope::Body::ClientState(snapshot)) => {
                        if !handle
                            .client_state(Arc::clone(&state), received_session_id, snapshot)
                            .await
                        {
                            break;
                        }
                    }
                    _ => break,
                }
            }
            envelope = outgoing.recv() => {
                let Some(envelope) = envelope else {
                    break;
                };
                let bytes = envelope.encode_to_vec();
                if !matches!(
                    timeout(
                        SEND_TIMEOUT,
                        socket.send(WebSocketMessage::Binary(bytes.into())),
                    )
                    .await,
                    Ok(Ok(()))
                ) {
                    break;
                }
            }
        }
    }
    handle.disconnected(session_id).await;
}

/// Sends one handshake envelope within the fixed transport timeout.
async fn send_handshake(socket: &mut WebSocket, envelope: ServerHandshakeEnvelope) -> bool {
    let bytes = envelope.encode_to_vec();
    timeout(
        SEND_TIMEOUT,
        socket.send(WebSocketMessage::Binary(bytes.into())),
    )
    .await
    .is_ok_and(|result| result.is_ok())
}

/// Receives one bounded binary handshake envelope within the admission timeout.
/// Text, control, oversized, malformed, closed, and errored messages all terminate
/// the current admission path.
async fn receive_handshake(socket: &mut WebSocket) -> Option<ClientHandshakeEnvelope> {
    match timeout(HANDSHAKE_TIMEOUT, socket.recv()).await {
        Ok(Some(Ok(WebSocketMessage::Binary(bytes)))) if bytes.len() <= MAX_MESSAGE_BYTES => {
            ClientHandshakeEnvelope::decode(bytes).ok()
        }
        _ => None,
    }
}

/// Decodes one bounded binary active envelope; every other WebSocket outcome is
/// terminal for the active connection.
fn decode_active(
    message: Option<Result<WebSocketMessage, axum::Error>>,
) -> Option<ClientActiveEnvelope> {
    match message {
        Some(Ok(WebSocketMessage::Binary(bytes))) if bytes.len() <= MAX_MESSAGE_BYTES => {
            ClientActiveEnvelope::decode(bytes).ok()
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests;
