//! Device-specific application coordination and connection-bound protocol handling.

use std::{sync::Arc, time::Duration};

use axum::extract::ws::{Message as WebSocketMessage, WebSocket};
use natsume_device_protocol::generated::{
    ClientActiveEnvelope, ClientHandshakeEnvelope, EnrollmentReviewState, EnrollmentReviewStatus,
    ServerHandshakeEnvelope, SessionReady, client_active_envelope, server_handshake_envelope,
};
use prost::Message as _;
use tokio::{
    sync::{Mutex, mpsc},
    time::{Instant, Sleep, sleep_until, timeout},
};
use uuid::Uuid;

mod actor;
mod admission;
mod application;
mod convergence;
mod state;

pub(crate) use convergence::{
    BindingActual, BindingArtifactState, BindingContext, BindingConvergence, BindingEvaluation,
    BindingEvaluationCode, BindingTarget, ConnectionState, ConvergenceStatus, DeviceConvergence,
    DeviceConvergenceError, DeviceStatus, GatewayActual, GatewayConvergence, GatewayState,
    GatewayTarget, HomeActual, HomeConvergence, HomeState, RuntimeConfigActual,
    RuntimeConfigConvergence, RuntimeConfigState, SessionActual, SessionConvergence, SessionState,
};

use crate::component::{
    binding::BindingComponent,
    device::{
        ControlAuthority, DeviceComponent, DeviceId, EnrollmentReviewDecision,
        EnrollmentStartOutcome, MachineHardwareId,
    },
    gateway::GatewayComponent,
    home::HomeComponent,
    provisioning::ProvisioningComponent,
    runtime::RuntimeConfigComponent,
    session::SessionControlComponent,
};

use self::{
    actor::{DeviceHandle, DeviceRegistry},
    admission::{EnrollmentPreAuth, ProofSubmission, ProofWindow},
};

/// Device application use cases and their process-local runtime.
///
/// Components own business rules and transactions. This coordinator owns the
/// ordering between those operations and current leases, with explicit dependencies
/// instead of a reference back to the process composition object.
pub(crate) struct DeviceControl {
    provisioning: Arc<ProvisioningComponent>,
    device: Arc<DeviceComponent>,
    gateway: GatewayComponent,
    binding: Arc<BindingComponent>,
    runtime: RuntimeConfigComponent,
    session: Arc<SessionControlComponent>,
    home: Arc<HomeComponent>,
    registry: DeviceRegistry,
    /// Prevents concurrent approvals from acting on the same stale authority read.
    enrollment_approval: Mutex<()>,
}

impl DeviceControl {
    pub(crate) fn new(
        provisioning: Arc<ProvisioningComponent>,
        device: Arc<DeviceComponent>,
        gateway: GatewayComponent,
        binding: Arc<BindingComponent>,
        runtime: RuntimeConfigComponent,
        session: Arc<SessionControlComponent>,
        home: Arc<HomeComponent>,
    ) -> Self {
        Self {
            provisioning,
            device,
            gateway,
            binding,
            runtime,
            session,
            home,
            registry: DeviceRegistry::new(),
            enrollment_approval: Mutex::new(()),
        }
    }

    async fn evict_current_lease(&self, device_id: DeviceId) {
        let Some(handle) = self.registry.get(device_id).await else {
            return;
        };
        *handle.authority_fence.lock().await = true;
        handle.evict_current_lease().await;
    }
}

/// Maximum accepted Device Control WebSocket frame and protobuf message size.
pub(crate) const MAX_MESSAGE_BYTES: usize = 65_536;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const SEND_TIMEOUT: Duration = Duration::from_secs(5);
const CLIENT_SILENCE_TIMEOUT: Duration = Duration::from_mins(1);
const OUTBOUND_CAPACITY: usize = 1;

/// Drives one upgraded WebSocket through admission, lease attachment, and the active
/// Device protocol.
///
/// Admission returns an authenticated authority, which is re-read from the Device
/// Component before and immediately after attachment. These checks reject authority
/// changes during admission and close the precheck-to-attach eviction race. Every
/// failed stage returns and drops the socket. Once a lease is attached, all exits
/// either notify the Device actor directly or pass through [`run_active`], which does
/// so before returning.
pub(crate) async fn serve_connection(mut socket: WebSocket, control: Arc<DeviceControl>) {
    let Some((machine_hardware_id, authority)) = admit(&mut socket, &control).await else {
        return;
    };
    let (outbound, mut outgoing) = mpsc::channel(OUTBOUND_CAPACITY);
    let Some((session_id, handle)) = control
        .attach_device_lease(machine_hardware_id, authority, outbound)
        .await
    else {
        return;
    };
    if !send_handshake(
        &mut socket,
        ServerHandshakeEnvelope {
            body: Some(server_handshake_envelope::Body::SessionReady(
                SessionReady {
                    session_id: session_id.as_bytes().to_vec(),
                },
            )),
        },
    )
    .await
    {
        handle.clear_lease_if_current(session_id).await;
        return;
    }

    run_active(socket, session_id, handle, &mut outgoing).await;
}

/// Performs the single proof exchange and returns the exact authority authenticated
/// for this connection.
///
/// Resume verifies against the database-selected current key. Enrollment delegates
/// replay/manual-review handling to [`admit_enrollment`]. Any malformed, timed-out,
/// rejected, or inactive path returns `None` and closes the connection.
async fn admit(
    socket: &mut WebSocket,
    control: &Arc<DeviceControl>,
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
            let authority = control
                .device
                .find_current_authority(machine_hardware_id)
                .await
                .ok()?;
            Some((
                machine_hardware_id,
                resume.verify_authority(authority).ok()?,
            ))
        }
        ProofSubmission::Enrollment(enrollment) => {
            admit_enrollment(socket, control, enrollment).await
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
    control: &Arc<DeviceControl>,
    enrollment: EnrollmentPreAuth,
) -> Option<(MachineHardwareId, ControlAuthority)> {
    let evidence = enrollment.review_evidence();
    let machine_hardware_id = evidence.machine_hardware_id();
    let authority = match control
        .device
        .start_enrollment(&control.provisioning, evidence)
        .await
        .ok()?
    {
        EnrollmentStartOutcome::Replay(authority) => authority,
        EnrollmentStartOutcome::Pending(review, mut activation) => {
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
                // cannot receive its pending control, retaining the review would leave
                // an operator-visible request with no connection able to complete
                // the activation/ready exchange.
                control
                    .device
                    .remove_enrollment_review(review.review_id())
                    .await;
                return None;
            }
            let client_deadline = sleep_until(Instant::now() + CLIENT_SILENCE_TIMEOUT);
            tokio::pin!(client_deadline);
            let decision = loop {
                tokio::select! {
                    result = &mut activation => break result.ok()?.ok()?,
                    message = socket.recv() => match message {
                        Some(Ok(WebSocketMessage::Ping(payload))) if payload.is_empty() => {
                            client_deadline
                                .as_mut()
                                .reset(Instant::now() + CLIENT_SILENCE_TIMEOUT);
                        }
                        _ => {
                            control.device.remove_enrollment_review(review.review_id()).await;
                            return None;
                        }
                    },
                    () = &mut client_deadline => {
                        control.device.remove_enrollment_review(review.review_id()).await;
                        return None;
                    }
                }
            };
            match decision {
                EnrollmentReviewDecision::Activated(authority) => authority,
                EnrollmentReviewDecision::Denied => {
                    let _ = send_handshake(
                        socket,
                        ServerHandshakeEnvelope {
                            body: Some(server_handshake_envelope::Body::EnrollmentReviewStatus(
                                EnrollmentReviewStatus {
                                    state: EnrollmentReviewState::Denied.into(),
                                    error_code: "ENROLLMENT_DENIED".to_owned(),
                                },
                            )),
                        },
                    )
                    .await;
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
    session_id: Uuid,
    handle: DeviceHandle,
    outgoing: &mut mpsc::Receiver<natsume_device_protocol::generated::ServerActiveEnvelope>,
) {
    let client_deadline = sleep_until(Instant::now() + CLIENT_SILENCE_TIMEOUT);
    tokio::pin!(client_deadline);
    loop {
        tokio::select! {
            biased;
            () = &mut client_deadline => break,
            message = socket.recv() => {
                let bytes = match message {
                    Some(Ok(WebSocketMessage::Binary(bytes))) if bytes.len() <= MAX_MESSAGE_BYTES => bytes,
                    Some(Ok(WebSocketMessage::Ping(payload))) if refresh_active_client_deadline(
                        client_deadline.as_mut(),
                        payload.as_ref(),
                        &session_id,
                    ) => {
                            continue;
                        }
                    _ => break,
                };
                let Ok(envelope) = ClientActiveEnvelope::decode(bytes) else {
                    break;
                };
                let Ok(received_session_id) = Uuid::from_slice(&envelope.session_id) else {
                    break;
                };
                match envelope.body {
                    Some(client_active_envelope::Body::ClientState(snapshot)) => {
                        tokio::select! {
                            biased;
                            () = &mut client_deadline => break,
                            result = handle.enqueue_client_state(received_session_id, snapshot) => {
                                if result.is_err() {
                                    break;
                                }
                            }
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
    drop(socket);
    handle.clear_lease_if_current(session_id).await;
}

fn refresh_active_client_deadline(
    mut deadline: std::pin::Pin<&mut Sleep>,
    payload: &[u8],
    session_id: &Uuid,
) -> bool {
    if payload != session_id.as_bytes() {
        return false;
    }
    deadline
        .as_mut()
        .reset(Instant::now() + CLIENT_SILENCE_TIMEOUT);
    true
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

#[cfg(test)]
mod tests;
