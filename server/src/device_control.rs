//! Device Control connection-bound protocol coordination.

use std::{
    sync::{Arc, Weak},
    time::Duration,
};

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
mod convergence;
mod state;

pub(crate) use convergence::{DeviceConvergenceError, DeviceConvergenceResponse, DeviceStatus};

use crate::{
    component::device::{
        ControlAuthority, DeviceError, DeviceId, EnrollmentApprovalError, EnrollmentReviewDecision,
        EnrollmentReviewId, EnrollmentStartOutcome, LifecycleOutcome, MachineHardwareId,
    },
    server_state::ServerState,
};

use self::{
    actor::{DeviceHandle, DeviceRegistry},
    admission::{EnrollmentPreAuth, ProofSubmission, ProofWindow},
};

/// Process-wide coordinator for active Device connections.
///
/// It owns the actor registry and the ordering needed by WP8 target refresh,
/// authority mutations, Enrollment approval, and current connection observation.
pub(crate) struct DeviceControl {
    registry: DeviceRegistry,
    /// Prevents concurrent approvals from acting on the same stale authority read.
    enrollment_approval: Mutex<()>,
}

impl DeviceControl {
    pub(crate) fn new() -> Self {
        Self {
            registry: DeviceRegistry::new(),
            enrollment_approval: Mutex::new(()),
        }
    }

    async fn replace_current_lease(
        &self,
        device_id: crate::component::device::DeviceId,
        state: Weak<ServerState>,
        outbound: mpsc::Sender<natsume_device_protocol::generated::ServerActiveEnvelope>,
    ) -> Option<(Uuid, DeviceHandle)> {
        let handle = self.registry.get_or_spawn(device_id).await;
        let session_id = handle.replace_current_lease(state, outbound).await?;
        Some((session_id, handle))
    }

    pub(crate) async fn dirty_one(&self, device_id: DeviceId) {
        self.registry.dirty_one(device_id).await;
    }

    pub(crate) async fn dirty_all(&self) {
        self.registry.dirty_all().await;
    }

    pub(crate) async fn evict_current_lease(&self, device_id: DeviceId) {
        let Some(handle) = self.registry.get(device_id).await else {
            return;
        };
        *handle.authority_fence.lock().await = true;
        handle.evict_current_lease().await;
    }

    /// Completes disable, fencing, and eviction independently of request cancellation.
    pub(crate) async fn disable_device(
        &self,
        state: Arc<ServerState>,
        device_id: DeviceId,
    ) -> Result<LifecycleOutcome, DeviceError> {
        tokio::spawn(async move {
            state
                .device_control()
                .disable_device_inner(&state, device_id)
                .await
        })
        .await
        .unwrap_or(Err(DeviceError::PersistenceFailed))
    }

    async fn disable_device_inner(
        &self,
        state: &ServerState,
        device_id: DeviceId,
    ) -> Result<LifecycleOutcome, DeviceError> {
        let handle = if let Some(handle) = self.registry.get(device_id).await {
            handle
        } else {
            if state.device().find_device(device_id).await?.is_none() {
                return Err(DeviceError::DeviceNotFound);
            }
            self.registry.get_or_spawn(device_id).await
        };
        let authority_fence = Arc::clone(&handle.authority_fence);
        let mut fenced = authority_fence.lock().await;
        let outcome = state.device().disable(device_id).await?;
        if matches!(
            outcome,
            LifecycleOutcome::Changed | LifecycleOutcome::Unchanged
        ) {
            *fenced = true;
        }
        drop(fenced);
        if matches!(
            outcome,
            LifecycleOutcome::Changed | LifecycleOutcome::Unchanged
        ) {
            handle.evict_current_lease().await;
        }
        Ok(outcome)
    }

    /// Completes revoke, fencing, and eviction independently of request cancellation.
    pub(crate) async fn revoke_device(
        &self,
        state: Arc<ServerState>,
        device_id: DeviceId,
    ) -> Result<LifecycleOutcome, DeviceError> {
        tokio::spawn(async move {
            state
                .device_control()
                .revoke_device_inner(&state, device_id)
                .await
        })
        .await
        .unwrap_or(Err(DeviceError::PersistenceFailed))
    }

    async fn revoke_device_inner(
        &self,
        state: &ServerState,
        device_id: DeviceId,
    ) -> Result<LifecycleOutcome, DeviceError> {
        let handle = if let Some(handle) = self.registry.get(device_id).await {
            handle
        } else {
            if state.device().find_device(device_id).await?.is_none() {
                return Err(DeviceError::DeviceNotFound);
            }
            self.registry.get_or_spawn(device_id).await
        };
        let authority_fence = Arc::clone(&handle.authority_fence);
        let mut fenced = authority_fence.lock().await;
        let outcome = state.device().revoke(device_id).await?;
        if matches!(
            outcome,
            LifecycleOutcome::Changed | LifecycleOutcome::Unchanged
        ) {
            *fenced = true;
        }
        drop(fenced);
        if matches!(
            outcome,
            LifecycleOutcome::Changed | LifecycleOutcome::Unchanged
        ) {
            handle.evict_current_lease().await;
        }
        Ok(outcome)
    }

    /// Completes approval, fencing, eviction, and notification independently of the request.
    pub(crate) async fn approve_enrollment(
        &self,
        state: Arc<ServerState>,
        review_id: EnrollmentReviewId,
    ) -> Result<ControlAuthority, EnrollmentApprovalError> {
        tokio::spawn(async move {
            state
                .device_control()
                .approve_enrollment_inner(&state, review_id)
                .await
        })
        .await
        .unwrap_or(Err(EnrollmentApprovalError::Activation(
            crate::component::device::ActivationError::AuthorityPersistenceFailed,
        )))
    }

    async fn approve_enrollment_inner(
        &self,
        state: &ServerState,
        review_id: EnrollmentReviewId,
    ) -> Result<ControlAuthority, EnrollmentApprovalError> {
        let _approval = self.enrollment_approval.lock().await;
        let machine_hardware_id = state
            .device()
            .enrollment_review_machine_hardware_id(review_id)
            .await?;
        let current_device_id = state
            .device()
            .find_current_authority(machine_hardware_id)
            .await
            .map_err(EnrollmentApprovalError::Authority)?
            .map(ControlAuthority::device_id);
        let handle = match current_device_id {
            Some(device_id) => Some(self.registry.get_or_spawn(device_id).await),
            None => None,
        };
        let authority_fence = handle
            .as_ref()
            .map(|handle| Arc::clone(&handle.authority_fence));
        let mut fenced = match authority_fence.as_ref() {
            Some(fence) => Some(fence.lock().await),
            None => None,
        };
        let approval = state
            .device()
            .approve_enrollment(state.provisioning(), review_id)
            .await?;
        let authority = approval.authority();
        if let Some(fenced) = fenced.as_mut() {
            **fenced = true;
        }
        drop(fenced);
        match handle {
            Some(handle) => handle.evict_current_lease().await,
            None => self.evict_current_lease(authority.device_id()).await,
        }
        approval.complete();
        Ok(authority)
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
    let Some((session_id, handle)) =
        attach_authority(&state, machine_hardware_id, authority, outbound).await
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

/// Replaces the current lease and then closes the precheck-to-replacement authority race.
async fn attach_authority(
    state: &Arc<ServerState>,
    machine_hardware_id: MachineHardwareId,
    authority: ControlAuthority,
    outbound: mpsc::Sender<natsume_device_protocol::generated::ServerActiveEnvelope>,
) -> Option<(Uuid, DeviceHandle)> {
    let (session_id, handle) = state
        .device_control()
        .replace_current_lease(authority.device_id(), Arc::downgrade(state), outbound)
        .await?;
    if state
        .device()
        .find_current_authority(machine_hardware_id)
        .await
        .ok()
        != Some(Some(authority))
    {
        handle.clear_lease_if_current(session_id).await;
        return None;
    }
    Some((session_id, handle))
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
                // cannot receive its pending state, retaining the review would leave
                // an operator-visible request with no connection able to complete
                // the activation/ready exchange.
                state
                    .device()
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
                            state.device().remove_enrollment_review(review.review_id()).await;
                            return None;
                        }
                    },
                    () = &mut client_deadline => {
                        state.device().remove_enrollment_review(review.review_id()).await;
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
