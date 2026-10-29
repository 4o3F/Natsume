//! Device-specific application coordination and connection-bound protocol handling.

use std::{net::IpAddr, sync::Arc, time::Duration};

use axum::extract::ws::{Message as WebSocketMessage, WebSocket};
use natsume_workstation_protocol::generated::{
    ClientActiveEnvelope, ClientHandshakeEnvelope, EnrollmentReviewState, EnrollmentReviewStatus,
    ServerHandshakeEnvelope, SessionReady, client_active_envelope, server_handshake_envelope,
};
use prost::Message as _;
use tokio::{
    sync::{Mutex, OwnedSemaphorePermit, Semaphore, mpsc},
    time::{Instant, Sleep, sleep_until, timeout},
};
use uuid::Uuid;

mod actor;
pub(crate) use actor::LiveLeaseView;
mod admission;
mod application;
mod change;
mod convergence;
mod state;

pub(crate) use change::TargetChange;

pub(crate) use convergence::{
    BindingActual, BindingArtifactState, BindingContext, BindingConvergence, BindingEvaluation,
    BindingEvaluationCode, BindingTarget, ConnectionState, ConvergenceStatus, DeviceConvergence,
    DeviceConvergenceError, DeviceStatus, GatewayActual, GatewayConvergence, GatewayState,
    GatewayTarget, HomeActual, HomeConvergence, HomeState, RuntimeConfigActual,
    RuntimeConfigConvergence, RuntimeConfigState, SessionActual, SessionConvergence,
    SessionForeground, SessionState,
};

use crate::component::{
    binding::BindingComponent,
    device::{
        ControlAuthority, DeviceComponent, DeviceId, EnrollmentApprovalError,
        EnrollmentReviewDecision, EnrollmentReviewId, EnrollmentStartOutcome, MachineHardwareId,
    },
    gateway::GatewayComponent,
    home::HomeComponent,
    power::PowerControlComponent,
    provisioning::ProvisioningComponent,
    runtime::RuntimeConfigComponent,
    session::SessionControlComponent,
};

use crate::journal::{
    JournalContext,
    event::{
        Event,
        payload::{ActivationCommitted, AdmissionCompleted, AdmissionRejected},
        types::{AdmissionStage, DisconnectReason, ReviewCancelCause, SessionId},
    },
};

#[derive(Clone, Copy, Debug)]
pub(super) enum ConnectionEnd {
    PingSilence,
    ClientClose,
    TransportError,
    ProtocolViolation,
    ActorInboxUnavailable,
    SendFailed,
    SendTimeout,
    OutgoingClosed,
}

impl ConnectionEnd {
    fn reason(self) -> DisconnectReason {
        match self {
            Self::PingSilence => DisconnectReason::PingSilence,
            Self::ClientClose => DisconnectReason::ClientClose,
            Self::TransportError => DisconnectReason::TransportError,
            Self::ProtocolViolation => DisconnectReason::ProtocolViolation,
            Self::ActorInboxUnavailable => DisconnectReason::ActorInboxUnavailable,
            Self::SendFailed => DisconnectReason::SendFailed,
            Self::SendTimeout => DisconnectReason::SendTimeout,
            Self::OutgoingClosed => DisconnectReason::OutgoingClosed,
        }
    }
}

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
    /// Composition-root provisioning policy; shared for the coordinator lifetime.
    provisioning: Arc<ProvisioningComponent>,
    /// Composition-root durable authority and enrollment operations; shared handle.
    device: Arc<DeviceComponent>,
    /// Composition-root gateway materialization dependency; immutable handle.
    gateway: GatewayComponent,
    /// Composition-root committed binding dependency; shared handle.
    binding: Arc<BindingComponent>,
    /// Composition-root runtime configuration dependency; immutable handle.
    runtime: RuntimeConfigComponent,
    /// Composition-root session target dependency; shared handle.
    session: Arc<SessionControlComponent>,
    /// Composition-root home target dependency; shared handle.
    home: Arc<HomeComponent>,
    /// Composition-root power target dependency; shared handle.
    power: Arc<PowerControlComponent>,
    /// Actor directory; owns insertion identity checks and permanent closure on shutdown.
    registry: DeviceRegistry,
    /// Prevents concurrent approvals from acting on the same stale authority read.
    enrollment_approval: Mutex<()>,
    /// Capacity-only limit of `MAX_HANDSHAKES` concurrent short exchanges; never closed for shutdown.
    handshakes: Arc<Semaphore>,
    /// Shared process admission and shutdown notification, supplied by the composition root.
    lifecycle: crate::lifecycle::ServerLifecycle,
    /// Emit-only journal handle supplied by the composition root for device events.
    journal: crate::journal::Journal,
}

impl DeviceControl {
    pub(crate) fn live_lease_view(&self) -> LiveLeaseView {
        self.registry.live_view()
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        provisioning: Arc<ProvisioningComponent>,
        device: Arc<DeviceComponent>,
        gateway: GatewayComponent,
        binding: Arc<BindingComponent>,
        runtime: RuntimeConfigComponent,
        session: Arc<SessionControlComponent>,
        home: Arc<HomeComponent>,
        power: Arc<PowerControlComponent>,
        journal: crate::journal::Journal,
        lifecycle: crate::lifecycle::ServerLifecycle,
    ) -> Self {
        Self {
            provisioning,
            device,
            gateway,
            binding,
            runtime,
            session,
            home,
            power,
            registry: DeviceRegistry::new(journal.clone()),
            enrollment_approval: Mutex::new(()),
            handshakes: Arc::new(Semaphore::new(MAX_HANDSHAKES)),
            lifecycle,
            journal,
        }
    }

    /// Reserves short-lived protocol work before HTTP upgrades the connection.
    pub(crate) fn try_reserve_handshake(
        &self,
    ) -> Result<OwnedSemaphorePermit, AdmissionUnavailable> {
        let _admission = self
            .lifecycle
            .admit()
            .ok_or(AdmissionUnavailable::ServerStopping)?;
        Arc::clone(&self.handshakes)
            .try_acquire_owned()
            .map_err(|source| AdmissionUnavailable::Capacity { source })
    }

    pub(crate) async fn shutdown(&self) {
        self.registry.shutdown().await;
    }

    async fn evict_current_lease(&self, device_id: DeviceId, reason: actor::FenceReason) {
        let Some(handle) = self.registry.get(device_id).await else {
            return;
        };
        *handle.authority_fence.lock().await = Some(reason.clone());
        handle
            .evict_current_lease(actor::LeaseEnd {
                reason: reason.kind.reason(),
                context: reason.context,
            })
            .await;
    }
}

#[derive(Debug, snafu::Snafu)]
pub(crate) enum AdmissionUnavailable {
    #[snafu(display("server is stopping"))]
    ServerStopping,
    #[snafu(display("device handshake capacity is exhausted"))]
    Capacity {
        source: tokio::sync::TryAcquireError,
    },
}

struct AdmissionTrace {
    /// Peer socket address supplied explicitly by HTTP, never by a Client field.
    peer_ip: IpAddr,
    /// Current handshake stage, updated before every potentially rejecting exchange.
    stage: AdmissionStage,
    /// Server-selected authority identity when known; never inferred from untrusted text.
    device_id: Option<crate::component::device::DeviceId>,
    /// Review owned by this connection until claimed or explicitly cancelled.
    pending_review: Option<EnrollmentReviewId>,
}

impl DeviceControl {
    pub(crate) fn reject_admission(
        &self,
        stage: AdmissionStage,
        peer_ip: IpAddr,
        device_id: Option<crate::component::device::DeviceId>,
    ) {
        let mut context = JournalContext::default();
        if let Some(device_id) = device_id {
            context = context.with_device(device_id);
        }
        self.journal.emit(
            Event::AdmissionRejected(AdmissionRejected {
                stage,
                peer_ip,
                device_id,
            }),
            context,
        );
    }
}

/// Maximum accepted Device Control WebSocket frame and protobuf message size.
pub(crate) const MAX_MESSAGE_BYTES: usize = 65_536;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_HANDSHAKES: usize = 64;
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
pub(crate) async fn serve_connection(
    mut socket: WebSocket,
    control: Arc<DeviceControl>,
    permit: OwnedSemaphorePermit,
    peer_ip: IpAddr,
) {
    let Some(admission) = control.lifecycle.admit() else {
        control.reject_admission(AdmissionStage::ServerStopping, peer_ip, None);
        return;
    };
    let mut trace = AdmissionTrace {
        peer_ip,
        stage: AdmissionStage::Challenge,
        device_id: None,
        pending_review: None,
    };
    let mut permit = Some(permit);
    let admitted = tokio::select! {
        biased;
        () = admission.stopping() => { trace.stage = AdmissionStage::ServerStopping; None },
        admitted = admit(&mut socket, &control, &admission, &mut permit, &mut trace) => admitted,
    };
    let Some((machine_hardware_id, authority, client_ip)) = admitted else {
        if let Some(review_id) = trace.pending_review.take() {
            let cause = if trace.stage == AdmissionStage::ServerStopping {
                ReviewCancelCause::ServerStopping
            } else {
                ReviewCancelCause::TransportError
            };
            control
                .device
                .remove_enrollment_review(
                    review_id,
                    cause,
                    crate::journal::JournalContext::default(),
                )
                .await;
        }
        control.reject_admission(trace.stage, trace.peer_ip, trace.device_id);
        return;
    };
    trace.stage = AdmissionStage::Attach;
    trace.device_id = Some(authority.device_id());
    let (outbound, mut outgoing) = mpsc::channel(OUTBOUND_CAPACITY);
    let attached = tokio::select! {
        biased;
        () = admission.stopping() => { trace.stage = AdmissionStage::ServerStopping; None },
        attached = control.attach_device_lease(&admission, machine_hardware_id, authority, outbound) => attached.inspect_err(|error| {
            if matches!(error, AdmissionUnavailable::ServerStopping) { trace.stage = AdmissionStage::ServerStopping; }
        }).ok().flatten(),
    };
    let Some((session_id, handle)) = attached else {
        control.reject_admission(trace.stage, trace.peer_ip, trace.device_id);
        return;
    };
    if !finish_ready_exchange(
        &control,
        admission,
        authority.device_id(),
        session_id,
        peer_ip,
        &handle,
        send_handshake_result(
            &mut socket,
            ServerHandshakeEnvelope {
                body: Some(server_handshake_envelope::Body::SessionReady(
                    SessionReady {
                        session_id: session_id.as_bytes().to_vec(),
                    },
                )),
            },
        ),
    )
    .await
    {
        return;
    }

    handle
        .record_network_addresses(session_id, authority, peer_ip, client_ip)
        .await;
    drop(permit);
    let cause = run_active(socket, session_id, &handle, &mut outgoing).await;
    handle.clear_lease_if_current(session_id, cause).await;
}

/// Finishes the attached handshake before handing the socket to the active loop.
async fn finish_ready_exchange(
    control: &DeviceControl,
    admission: crate::lifecycle::Admission,
    device_id: DeviceId,
    session_id: Uuid,
    peer_ip: IpAddr,
    handle: &DeviceHandle,
    ready: impl std::future::Future<Output = Result<(), ConnectionEnd>>,
) -> bool {
    let result = tokio::select! {
        biased;
        () = admission.stopping() => {
            control.reject_admission(AdmissionStage::ServerStopping, peer_ip, Some(device_id));
            handle.end_lease_if_current(session_id, actor::LeaseEnd::local(DisconnectReason::ServerShutdown)).await;
            return false;
        }
        result = ready => result,
    };
    if let Err(cause) = result {
        control.reject_admission(AdmissionStage::ReadyExchange, peer_ip, Some(device_id));
        handle.clear_lease_if_current(session_id, cause).await;
        return false;
    }
    control.journal.emit(
        Event::AdmissionCompleted(AdmissionCompleted {}),
        JournalContext::default()
            .with_device(device_id)
            .with_session(SessionId::from_lease(session_id)),
    );
    // Track the final handshake outcome, but never the long-lived active lease.
    drop(admission);
    true
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
    admission: &crate::lifecycle::Admission,
    permit: &mut Option<OwnedSemaphorePermit>,
    trace: &mut AdmissionTrace,
) -> Option<(MachineHardwareId, ControlAuthority, Option<IpAddr>)> {
    let mut proof_window = ProofWindow::new().inspect_err(|error| tracing::warn!(error = %snafu::Report::from_error(error), "Device admission failed")).ok()?;
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
    trace.stage = AdmissionStage::Proof;
    let proof = receive_handshake(socket).await?;
    let submission = proof_window
        .submit(proof)
        .inspect_err(|error| {
            trace.stage = error.journal_stage();
            tracing::warn!(error = %snafu::Report::from_error(error), "Device admission failed");
        })
        .ok()?;
    let client_ip = submission.client_ip();
    let (machine_hardware_id, authority) = match submission {
        ProofSubmission::Resume(resume) => {
            trace.stage = AdmissionStage::ResumeAuthority;
            let machine_hardware_id = resume.machine_hardware_id();
            let authority = control
                .device
                .find_current_authority(machine_hardware_id)
                .await
                .inspect_err(|error| tracing::warn!(error = %snafu::Report::from_error(error), "Device admission authority lookup failed"))
                .ok()?;
            trace.device_id = authority.map(ControlAuthority::device_id);
            Some((
                machine_hardware_id,
                resume.verify_authority(authority).inspect_err(|error| { trace.stage = error.journal_stage(); tracing::warn!(error = %snafu::Report::from_error(error), "Device admission failed"); }).ok()?,
            ))
        }
        ProofSubmission::Enrollment(enrollment) => {
            admit_enrollment(socket, control, admission, enrollment, permit, trace).await
        }
    }?;
    Some((machine_hardware_id, authority, client_ip))
}

/// Completes the connection-local Enrollment path after candidate-key proof.
///
/// An exact committed replay skips review. A new candidate sends `PendingReview` and
/// waits for automatic approval while the window is open or administrator approval
/// while closed. Connection failure attempts to remove the review if approval has
/// not already claimed it. Both paths send the
/// activated Device ID and require its exact `EnrollmentReady` echo before returning
/// the enabled authority.
async fn admit_enrollment(
    socket: &mut WebSocket,
    control: &Arc<DeviceControl>,
    admission: &crate::lifecycle::Admission,
    enrollment: EnrollmentPreAuth,
    permit: &mut Option<OwnedSemaphorePermit>,
    trace: &mut AdmissionTrace,
) -> Option<(MachineHardwareId, ControlAuthority)> {
    trace.stage = AdmissionStage::Enrollment;
    let evidence = enrollment.review_evidence();
    let machine_hardware_id = evidence.machine_hardware_id();
    let authority = match control.device.start_enrollment(evidence, trace.peer_ip, JournalContext::default()).await.inspect_err(|error| { if matches!(error, crate::component::device::EnrollmentStartError::ReviewCapacityReached) { trace.stage = AdmissionStage::Capacity; } tracing::warn!(error = %snafu::Report::from_error(error), "Device enrollment start failed"); }).ok()? {
        EnrollmentStartOutcome::Replay(authority) => authority,
        EnrollmentStartOutcome::Pending(review, mut activation) => {
            trace.pending_review = Some(review.review_id());
            // The bounded review registry now owns this connection's admission
            // capacity. Manual review must not occupy a short handshake slot.
            drop(permit.take());
            if !send_review_status(socket, ReviewStatus::Pending).await
            {
                // A pending review belongs to this proved connection. If the Client
                // cannot receive its pending control, retaining the review would leave
                // an operator-visible request with no connection able to complete
                // the activation/ready exchange.
                control.device.remove_enrollment_review(review.review_id(), ReviewCancelCause::TransportError, crate::journal::JournalContext::default()).await;
                trace.pending_review = None;
                return None;
            }
            let client_deadline = sleep_until(Instant::now() + CLIENT_SILENCE_TIMEOUT);
            tokio::pin!(client_deadline);
            let mut window = control.provisioning.subscribe();
            let automatic_approval = async {
                window.wait_for(|window| window.is_open()).await.ok()?;
                Some(control.automatically_approve_enrollment(admission, review.review_id(), JournalContext::default()).await)
            };
            tokio::pin!(automatic_approval);
            let mut automatic_approval_finished = false;
            let decision = loop {
                tokio::select! {
                    result = &mut activation => match result.ok()? {
                        Ok(decision) => break decision,
                        Err(error) => { trace.stage = approval_rejection_stage(&error); return None; },
                    },
                    result = &mut automatic_approval, if !automatic_approval_finished => {
                        automatic_approval_finished = true;
                        if !matches!(result, Some(Ok(_) | Err(EnrollmentApprovalError::ReviewNotFound))) {
                            if let Some(Err(error)) = &result { trace.stage = approval_rejection_stage(error); }
                            control.device.remove_enrollment_review(review.review_id(), ReviewCancelCause::TransportError, crate::journal::JournalContext::default()).await;
                            trace.pending_review = None;
                            return None;
                        }
                    },
                    message = socket.recv() => match message {
                        Some(Ok(WebSocketMessage::Ping(payload))) if payload.is_empty() => {
                            client_deadline
                                .as_mut()
                                .reset(Instant::now() + CLIENT_SILENCE_TIMEOUT);
                        }
                        message => {
                            let cause = match message {
                                Some(Err(_)) => ReviewCancelCause::TransportError,
                                _ => ReviewCancelCause::ClientClose,
                            };
                            control.device.remove_enrollment_review(review.review_id(), cause, crate::journal::JournalContext::default()).await;
                            trace.pending_review = None;
                            return None;
                        }
                    },
                    () = &mut client_deadline => {
                        control.device.remove_enrollment_review(review.review_id(), ReviewCancelCause::Timeout, crate::journal::JournalContext::default()).await;
                        trace.pending_review = None;
                        return None;
                    }
                }
            };
            trace.pending_review = None;
            match decision {
                EnrollmentReviewDecision::Activated(authority) => authority,
                EnrollmentReviewDecision::Denied => {
                    let _ = send_review_status(socket, ReviewStatus::Denied).await;
                    return None;
                }
            }
        }
    };
    trace.stage = AdmissionStage::Activation;
    trace.device_id = Some(authority.device_id());
    let barrier = enrollment.activated(authority).inspect_err(|error| tracing::warn!(error = %snafu::Report::from_error(error), "Device admission failed")).ok()?;
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
    control.journal.emit(
        Event::ActivationCommitted(ActivationCommitted {}),
        JournalContext::default().with_device(authority.device_id()),
    );
    trace.stage = AdmissionStage::ReadyExchange;
    let authority = barrier.receive(receive_handshake(socket).await?).inspect_err(|error| tracing::warn!(error = %snafu::Report::from_error(error), "Device admission failed")).ok()?;
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
    handle: &DeviceHandle,
    outgoing: &mut mpsc::Receiver<natsume_workstation_protocol::generated::ServerActiveEnvelope>,
) -> ConnectionEnd {
    let client_deadline = sleep_until(Instant::now() + CLIENT_SILENCE_TIMEOUT);
    tokio::pin!(client_deadline);
    let cause = loop {
        tokio::select! {
            biased;
            () = &mut client_deadline => break ConnectionEnd::PingSilence,
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
                    Some(Ok(WebSocketMessage::Close(_))) | None => break ConnectionEnd::ClientClose,
                    Some(Err(_)) => break ConnectionEnd::TransportError,
                    Some(Ok(_)) => break ConnectionEnd::ProtocolViolation,
                };
                let Ok(envelope) = ClientActiveEnvelope::decode(bytes) else {
                    break ConnectionEnd::ProtocolViolation;
                };
                let Ok(received_session_id) = Uuid::from_slice(&envelope.session_id) else {
                    break ConnectionEnd::ProtocolViolation;
                };
                match envelope.body {
                    Some(client_active_envelope::Body::ClientState(snapshot)) => {
                        tokio::select! {
                            biased;
                            () = &mut client_deadline => break ConnectionEnd::PingSilence,
                            result = handle.enqueue_client_state(received_session_id, snapshot) => {
                                if let Err(error) = result {
                                    tracing::warn!(error = %snafu::Report::from_error(error), "Device actor inbox unavailable");
                                    break ConnectionEnd::ActorInboxUnavailable;
                                }
                            }
                        }
                    }
                    _ => break ConnectionEnd::ProtocolViolation,
                }
            }
            envelope = outgoing.recv() => {
                let Some(envelope) = envelope else {
                    break ConnectionEnd::OutgoingClosed;
                };
                let bytes = envelope.encode_to_vec();
                match timeout(SEND_TIMEOUT, socket.send(WebSocketMessage::Binary(bytes.into()))).await {
                    Ok(Ok(())) => {},
                    Ok(Err(_)) => break ConnectionEnd::SendFailed,
                    Err(_) => break ConnectionEnd::SendTimeout,
                }
            }
        }
    };
    drop(socket);
    cause
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

fn approval_rejection_stage(error: &EnrollmentApprovalError) -> AdmissionStage {
    match error {
        EnrollmentApprovalError::ReviewNotFound => AdmissionStage::Enrollment,
        EnrollmentApprovalError::ServerStopping => AdmissionStage::ServerStopping,
        EnrollmentApprovalError::Authority { .. } | EnrollmentApprovalError::Activation { .. } => {
            AdmissionStage::Activation
        }
    }
}

enum ReviewStatus {
    Pending,
    Denied,
}

async fn send_review_status(socket: &mut WebSocket, status: ReviewStatus) -> bool {
    let status = match status {
        ReviewStatus::Pending => EnrollmentReviewStatus {
            state: EnrollmentReviewState::PendingReview.into(),
            error_code: String::new(),
        },
        ReviewStatus::Denied => EnrollmentReviewStatus {
            state: EnrollmentReviewState::Denied.into(),
            error_code: "ENROLLMENT_DENIED".into(),
        },
    };
    send_handshake(
        socket,
        ServerHandshakeEnvelope {
            body: Some(server_handshake_envelope::Body::EnrollmentReviewStatus(
                status,
            )),
        },
    )
    .await
}

/// Sends one handshake envelope within the fixed transport timeout.
async fn send_handshake(socket: &mut WebSocket, envelope: ServerHandshakeEnvelope) -> bool {
    send_handshake_result(socket, envelope).await.is_ok()
}

async fn send_handshake_result(
    socket: &mut WebSocket,
    envelope: ServerHandshakeEnvelope,
) -> Result<(), ConnectionEnd> {
    let bytes = envelope.encode_to_vec();
    match timeout(
        SEND_TIMEOUT,
        socket.send(WebSocketMessage::Binary(bytes.into())),
    )
    .await
    {
        Ok(Ok(())) => Ok(()),
        Ok(Err(_)) => Err(ConnectionEnd::SendFailed),
        Err(_) => Err(ConnectionEnd::SendTimeout),
    }
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
