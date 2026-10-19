use futures_util::{SinkExt as _, StreamExt as _};
use natsume_workstation_ipc::diagnostics::{
    DiagnosticCode, DiagnosticHandshakeStep, DiagnosticStage, DiagnosticStatus,
};
use natsume_workstation_protocol::{
    generated::{
        ClientHandshakeEnvelope, EnrollmentAuthority, EnrollmentEvidenceQuality,
        EnrollmentReviewState, ServerChallenge, ServerHandshakeEnvelope, client_handshake_envelope,
        server_handshake_envelope,
    },
    is_valid_error_code_token,
};
use prost::Message as _;
use tokio::time::{Instant, sleep_until, timeout};
use uuid::{Uuid, Variant, Version};

use crate::diagnostics::Diagnostics;

use super::{
    ControlIdentity, ControlLoopError,
    connection::{
        HANDSHAKE_TIMEOUT, MAX_MESSAGE_BYTES, SEND_TIMEOUT, SERVER_SILENCE_TIMEOUT, Socket,
        heartbeat_interval,
    },
};

/// Result of the connection-local Enrollment or Resume exchange.
pub(super) enum HandshakeOutcome {
    /// The Server established this exact active lease.
    Active([u8; 16]),
    /// The connection ended and should use the ordinary reconnect delay.
    Retry,
}

pub(super) async fn handshake(
    socket: &mut Socket,
    identity: &mut ControlIdentity,
    machine_hardware_id: Uuid,
    evidence_quality: EnrollmentEvidenceQuality,
    diagnostics: &mut Diagnostics,
) -> Result<HandshakeOutcome, ControlLoopError> {
    let challenge = match receive_challenge(socket, diagnostics).await {
        Ok(challenge) => challenge,
        Err(code) => {
            diagnostics.failure(code, DiagnosticStatus::Retrying);
            return Ok(HandshakeOutcome::Retry);
        }
    };
    if let Err(code) = send_proof(
        socket,
        identity,
        &challenge,
        machine_hardware_id,
        evidence_quality,
        diagnostics,
    )
    .await
    {
        diagnostics.failure(code, DiagnosticStatus::Retrying);
        return Ok(HandshakeOutcome::Retry);
    }

    // Resume already has durable authority and skips enrollment-only phases.
    if identity.is_enrolling() {
        let authority = match await_enrollment_authority(socket, diagnostics).await {
            Ok(EnrollmentReviewOutcome::Approved(authority)) => authority,
            Ok(EnrollmentReviewOutcome::Denied) => return Ok(HandshakeOutcome::Retry),
            Err(code) => {
                diagnostics.failure(code, DiagnosticStatus::Retrying);
                return Ok(HandshakeOutcome::Retry);
            }
        };
        install_enrollment_authority(identity, &authority, diagnostics)?;
        if !send_enrollment_ready(socket, authority, diagnostics).await {
            diagnostics.failure(
                DiagnosticCode::HandshakeWriteFailed,
                DiagnosticStatus::Retrying,
            );
            return Ok(HandshakeOutcome::Retry);
        }
    }

    Ok(match await_session_ready(socket, diagnostics).await {
        Ok(session_id) => HandshakeOutcome::Active(session_id),
        Err(code) => {
            diagnostics.failure(code, DiagnosticStatus::Retrying);
            HandshakeOutcome::Retry
        }
    })
}

async fn receive_challenge(
    socket: &mut Socket,
    diagnostics: &mut Diagnostics,
) -> Result<ServerChallenge, DiagnosticCode> {
    diagnostics.set_stage(DiagnosticStage::Handshake, DiagnosticStatus::Retrying);
    diagnostics.update(|report| {
        report.connection.handshake_step = Some(DiagnosticHandshakeStep::AwaitingChallenge);
    });
    let envelope = receive(socket).await?;
    let Some(server_handshake_envelope::Body::ServerChallenge(challenge)) = envelope.body else {
        return Err(DiagnosticCode::HandshakeInvalidMessage);
    };
    if challenge.challenge_nonce.len() != 32 {
        return Err(DiagnosticCode::HandshakeInvalidMessage);
    }
    diagnostics.update(|report| {
        report.connection.handshake_step = Some(DiagnosticHandshakeStep::ChallengeReceived);
    });
    Ok(challenge)
}

async fn send_proof(
    socket: &mut Socket,
    identity: &ControlIdentity,
    challenge: &ServerChallenge,
    machine_hardware_id: Uuid,
    evidence_quality: EnrollmentEvidenceQuality,
    diagnostics: &mut Diagnostics,
) -> Result<(), DiagnosticCode> {
    let local_address = socket
        .get_ref()
        .get_ref()
        .local_addr()
        .map_err(|_| DiagnosticCode::LocalAddressUnavailable)?;
    diagnostics.update(|report| {
        report.connection.local_ip = Some(local_address.ip().to_canonical().to_string());
    });
    let mut proof = identity.proof(challenge, machine_hardware_id, evidence_quality);
    proof.client_ip = Some(local_address.ip().to_canonical().to_string());
    if !send(
        socket,
        ClientHandshakeEnvelope {
            body: Some(client_handshake_envelope::Body::ClientProof(proof)),
        },
    )
    .await
    {
        return Err(DiagnosticCode::HandshakeWriteFailed);
    }
    diagnostics.update(|report| {
        report.connection.handshake_step = Some(DiagnosticHandshakeStep::ProofSent);
    });
    Ok(())
}

enum EnrollmentReviewOutcome {
    Approved(EnrollmentAuthority),
    Denied,
}

async fn await_enrollment_authority(
    socket: &mut Socket,
    diagnostics: &mut Diagnostics,
) -> Result<EnrollmentReviewOutcome, DiagnosticCode> {
    let mut review_seen = false;
    loop {
        let envelope = if review_seen {
            receive_pending(socket).await?
        } else {
            receive(socket).await?
        };
        match envelope.body {
            Some(server_handshake_envelope::Body::EnrollmentReviewStatus(status)) => {
                match EnrollmentReviewState::try_from(status.state) {
                    Ok(EnrollmentReviewState::PendingReview)
                        if !review_seen && status.error_code.is_empty() =>
                    {
                        review_seen = true;
                        diagnostics.update(|report| {
                            report.connection.handshake_step =
                                Some(DiagnosticHandshakeStep::PendingReview);
                        });
                        diagnostics.clear_failure();
                        diagnostics.set_stage(
                            DiagnosticStage::PendingReview,
                            DiagnosticStatus::WaitingForReview,
                        );
                    }
                    Ok(EnrollmentReviewState::Denied)
                        if is_valid_error_code_token(&status.error_code) =>
                    {
                        diagnostics.update(|report| {
                            report.connection.handshake_step =
                                Some(DiagnosticHandshakeStep::Denied);
                        });
                        diagnostics.update(|report| {
                            report.connection.peer_error_code = Some(status.error_code);
                        });
                        diagnostics
                            .failure(DiagnosticCode::EnrollmentDenied, DiagnosticStatus::Denied);
                        return Ok(EnrollmentReviewOutcome::Denied);
                    }
                    _ => return Err(DiagnosticCode::HandshakeInvalidMessage),
                }
            }
            Some(server_handshake_envelope::Body::EnrollmentActivated(authority)) => {
                return Ok(EnrollmentReviewOutcome::Approved(authority));
            }
            _ => return Err(DiagnosticCode::HandshakeInvalidMessage),
        }
    }
}

fn install_enrollment_authority(
    identity: &mut ControlIdentity,
    authority: &EnrollmentAuthority,
    diagnostics: &mut Diagnostics,
) -> Result<(), ControlLoopError> {
    diagnostics.update(|report| {
        report.connection.handshake_step = Some(DiagnosticHandshakeStep::AuthorityReceived);
    });
    diagnostics.set_stage(
        DiagnosticStage::AuthorityInstallation,
        DiagnosticStatus::Retrying,
    );
    identity.install_authority(authority).map_err(|source| {
        diagnostics.failure(
            DiagnosticCode::AuthorityPersistenceFailed,
            DiagnosticStatus::Stopped,
        );
        ControlLoopError::AuthorityPersistence { source }
    })?;
    diagnostics.update(|report| {
        report.connection.device_id = identity.manifest.device_id().map(|id| id.to_string());
    });
    diagnostics.update(|report| {
        report.connection.handshake_step = Some(DiagnosticHandshakeStep::AuthorityInstalled);
    });
    diagnostics.complete(DiagnosticStage::AuthorityInstallation);
    Ok(())
}

async fn send_enrollment_ready(
    socket: &mut Socket,
    authority: EnrollmentAuthority,
    diagnostics: &mut Diagnostics,
) -> bool {
    if !send(
        socket,
        ClientHandshakeEnvelope {
            body: Some(client_handshake_envelope::Body::EnrollmentReady(authority)),
        },
    )
    .await
    {
        return false;
    }
    diagnostics.update(|report| {
        report.connection.handshake_step = Some(DiagnosticHandshakeStep::ReadySent);
    });
    diagnostics.set_stage(DiagnosticStage::Handshake, DiagnosticStatus::Retrying);
    true
}

async fn await_session_ready(
    socket: &mut Socket,
    diagnostics: &mut Diagnostics,
) -> Result<[u8; 16], DiagnosticCode> {
    let envelope = receive(socket).await?;
    let Some(server_handshake_envelope::Body::SessionReady(ready)) = envelope.body else {
        return Err(DiagnosticCode::HandshakeInvalidMessage);
    };
    let session_id =
        parse_session_id(&ready.session_id).ok_or(DiagnosticCode::HandshakeInvalidMessage)?;
    diagnostics.update(|report| {
        report.connection.handshake_step = Some(DiagnosticHandshakeStep::SessionReady);
    });
    diagnostics.complete(DiagnosticStage::Handshake);
    Ok(session_id)
}

fn parse_session_id(value: &[u8]) -> Option<[u8; 16]> {
    let bytes: [u8; 16] = value.try_into().ok()?;
    let session_id = Uuid::from_bytes(bytes);
    (session_id.get_version() == Some(Version::SortRand)
        && session_id.get_variant() == Variant::RFC4122)
        .then_some(bytes)
}

fn decode_handshake(
    message: Option<
        Result<tokio_tungstenite::tungstenite::Message, tokio_tungstenite::tungstenite::Error>,
    >,
) -> Result<ServerHandshakeEnvelope, DiagnosticCode> {
    use tokio_tungstenite::tungstenite::Message;
    match message {
        Some(Ok(Message::Binary(bytes))) if bytes.len() <= MAX_MESSAGE_BYTES => {
            ServerHandshakeEnvelope::decode(bytes)
                .map_err(|_| DiagnosticCode::HandshakeInvalidMessage)
        }
        Some(Err(tokio_tungstenite::tungstenite::Error::Capacity(_))) => {
            Err(DiagnosticCode::HandshakeInvalidMessage)
        }
        None | Some(Err(_) | Ok(Message::Close(_))) => Err(DiagnosticCode::HandshakeDisconnected),
        _ => Err(DiagnosticCode::HandshakeInvalidMessage),
    }
}

async fn receive(socket: &mut Socket) -> Result<ServerHandshakeEnvelope, DiagnosticCode> {
    let message = timeout(HANDSHAKE_TIMEOUT, socket.next())
        .await
        .map_err(|_| DiagnosticCode::HandshakeTimeout)?;
    decode_handshake(message)
}

async fn receive_pending(socket: &mut Socket) -> Result<ServerHandshakeEnvelope, DiagnosticCode> {
    let mut heartbeat = heartbeat_interval();
    let server_deadline = sleep_until(Instant::now() + SERVER_SILENCE_TIMEOUT);
    tokio::pin!(server_deadline);
    loop {
        tokio::select! {
            message = socket.next() => {
                match message {
                    Some(Ok(tokio_tungstenite::tungstenite::Message::Pong(payload)))
                        if payload.is_empty() =>
                    {
                        server_deadline
                            .as_mut()
                            .reset(Instant::now() + SERVER_SILENCE_TIMEOUT);
                    }
                    other => return decode_handshake(other),
                }
            }
            _ = heartbeat.tick() => {
                if !matches!(
                    timeout(
                        SEND_TIMEOUT,
                        socket.send(tokio_tungstenite::tungstenite::Message::Ping(
                            Vec::new().into(),
                        )),
                    )
                    .await,
                    Ok(Ok(()))
                ) {
                    return Err(DiagnosticCode::HandshakeWriteFailed);
                }
            }
            () = &mut server_deadline => return Err(DiagnosticCode::HandshakeTimeout),
        }
    }
}

async fn send(socket: &mut Socket, envelope: ClientHandshakeEnvelope) -> bool {
    let bytes = envelope.encode_to_vec();
    bytes.len() <= MAX_MESSAGE_BYTES
        && matches!(
            timeout(
                SEND_TIMEOUT,
                socket.send(tokio_tungstenite::tungstenite::Message::Binary(
                    bytes.into(),
                )),
            )
            .await,
            Ok(Ok(()))
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    use natsume_workstation_protocol::generated::{
        EnrollmentAuthority, EnrollmentReviewStatus, ServerChallenge, SessionReady,
    };
    use tokio::net::{TcpListener, TcpStream};
    use tokio_tungstenite::{
        MaybeTlsStream, WebSocketStream,
        tungstenite::{Message, protocol::Role},
    };

    struct HandshakeFixture {
        directory: tempfile::TempDir,
        machine: Uuid,
        server: WebSocketStream<TcpStream>,
        task: tokio::task::JoinHandle<(Result<HandshakeOutcome, ControlLoopError>, Diagnostics)>,
    }

    impl HandshakeFixture {
        async fn new(challenge: bool) -> Result<Self, Box<dyn std::error::Error>> {
            Self::with_diagnostics(challenge, Diagnostics::disabled()).await
        }

        async fn with_diagnostics(
            challenge: bool,
            diagnostics: Diagnostics,
        ) -> Result<Self, Box<dyn std::error::Error>> {
            Self::with_authority(challenge, diagnostics, None).await
        }

        async fn with_authority(
            challenge: bool,
            mut diagnostics: Diagnostics,
            device: Option<Uuid>,
        ) -> Result<Self, Box<dyn std::error::Error>> {
            let directory = tempfile::tempdir()?;
            let machine = Uuid::from_u128(0xa9aa_9d04_3ece_5567_8260_9109_30ff_5e03);
            let mut identity = crate::control::load_or_create_identity(directory.path(), machine)?;
            if let Some(device) = device {
                identity.install_authority(&EnrollmentAuthority {
                    device_id: device.to_string(),
                })?;
            }
            let listener = TcpListener::bind("127.0.0.1:0").await?;
            let tcp = TcpStream::connect(listener.local_addr()?).await?;
            let (peer, _) = listener.accept().await?;
            let mut client = WebSocketStream::from_raw_socket(
                MaybeTlsStream::Plain(tcp),
                Role::Client,
                Some(
                    tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
                        .max_message_size(Some(MAX_MESSAGE_BYTES))
                        .max_frame_size(Some(MAX_MESSAGE_BYTES)),
                ),
            )
            .await;
            let server = WebSocketStream::from_raw_socket(peer, Role::Server, None).await;
            let task = tokio::spawn(async move {
                // Mirror the caller's transport-success presentation reset.
                diagnostics.clear_failure();
                let outcome = handshake(
                    &mut client,
                    &mut identity,
                    machine,
                    EnrollmentEvidenceQuality::Strong,
                    &mut diagnostics,
                )
                .await;
                (outcome, diagnostics)
            });
            let mut fixture = Self {
                directory,
                machine,
                server,
                task,
            };
            if challenge {
                fixture
                    .send(server_handshake_envelope::Body::ServerChallenge(
                        ServerChallenge {
                            challenge_nonce: vec![0x12; 32],
                        },
                    ))
                    .await?;
                let _proof = fixture.server.next().await.ok_or("missing proof")??;
            }
            Ok(fixture)
        }

        async fn send(
            &mut self,
            body: server_handshake_envelope::Body,
        ) -> Result<(), Box<dyn std::error::Error>> {
            self.server
                .send(Message::Binary(
                    ServerHandshakeEnvelope { body: Some(body) }
                        .encode_to_vec()
                        .into(),
                ))
                .await?;
            Ok(())
        }
    }

    #[tokio::test]
    async fn pending_review_is_waiting_not_failure_and_denial_is_sanitized()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut fixture = HandshakeFixture::new(true).await?;
        fixture
            .send(server_handshake_envelope::Body::EnrollmentReviewStatus(
                EnrollmentReviewStatus {
                    state: EnrollmentReviewState::PendingReview.into(),
                    error_code: String::new(),
                },
            ))
            .await?;
        fixture
            .send(server_handshake_envelope::Body::EnrollmentReviewStatus(
                EnrollmentReviewStatus {
                    state: EnrollmentReviewState::Denied.into(),
                    error_code: "REVIEW_DENIED".to_owned(),
                },
            ))
            .await?;
        let (outcome, diagnostics) = fixture.task.await?;
        assert!(matches!(outcome?, HandshakeOutcome::Retry));
        let report = diagnostics.report();
        assert!(
            report
                .events
                .iter()
                .any(|event| event.stage == DiagnosticStage::PendingReview
                    && event.status == DiagnosticStatus::WaitingForReview
                    && event.code.is_none())
        );
        assert_eq!(report.status, DiagnosticStatus::Denied);
        assert_eq!(
            report.connection.handshake_step,
            Some(DiagnosticHandshakeStep::Denied)
        );
        assert_eq!(
            report.connection.peer_error_code.as_deref(),
            Some("REVIEW_DENIED")
        );
        assert_eq!(
            report.failure.as_ref().map(|failure| failure.code),
            Some(DiagnosticCode::EnrollmentDenied)
        );
        assert_eq!(report.connection.device_id, None);
        Ok(())
    }

    #[tokio::test]
    async fn enrollment_cannot_skip_authority_installation_or_repeat_pending_review()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut early = HandshakeFixture::new(true).await?;
        early
            .send(server_handshake_envelope::Body::SessionReady(
                SessionReady {
                    session_id: Uuid::now_v7().as_bytes().to_vec(),
                },
            ))
            .await?;
        let (outcome, diagnostics) = early.task.await?;
        assert!(matches!(outcome?, HandshakeOutcome::Retry));
        assert_eq!(
            diagnostics.report().connection.handshake_step,
            Some(DiagnosticHandshakeStep::ProofSent)
        );
        assert_eq!(diagnostics.report().connection.device_id, None);
        assert_eq!(
            diagnostics
                .report()
                .failure
                .as_ref()
                .map(|failure| failure.code),
            Some(DiagnosticCode::HandshakeInvalidMessage)
        );

        let mut repeated = HandshakeFixture::new(true).await?;
        for _ in 0..2 {
            repeated
                .send(server_handshake_envelope::Body::EnrollmentReviewStatus(
                    EnrollmentReviewStatus {
                        state: EnrollmentReviewState::PendingReview.into(),
                        error_code: String::new(),
                    },
                ))
                .await?;
        }
        let (outcome, diagnostics) = repeated.task.await?;
        assert!(matches!(outcome?, HandshakeOutcome::Retry));
        assert_eq!(
            diagnostics.report().connection.handshake_step,
            Some(DiagnosticHandshakeStep::PendingReview)
        );
        assert_eq!(
            diagnostics
                .report()
                .failure
                .as_ref()
                .map(|failure| failure.code),
            Some(DiagnosticCode::HandshakeInvalidMessage)
        );
        Ok(())
    }

    #[tokio::test]
    async fn ready_phase_rejects_a_second_authority_without_replacing_installed_identity()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut fixture = HandshakeFixture::new(true).await?;
        let device = Uuid::now_v7();
        fixture
            .send(server_handshake_envelope::Body::EnrollmentActivated(
                EnrollmentAuthority {
                    device_id: device.to_string(),
                },
            ))
            .await?;
        let _ready = fixture.server.next().await.ok_or("missing ready")??;
        fixture
            .send(server_handshake_envelope::Body::EnrollmentActivated(
                EnrollmentAuthority {
                    device_id: Uuid::now_v7().to_string(),
                },
            ))
            .await?;
        let (outcome, diagnostics) = fixture.task.await?;
        assert!(matches!(outcome?, HandshakeOutcome::Retry));
        assert_eq!(
            diagnostics.report().connection.handshake_step,
            Some(DiagnosticHandshakeStep::ReadySent)
        );
        assert_eq!(
            diagnostics.report().connection.device_id.as_deref(),
            Some(device.to_string().as_str())
        );
        assert_eq!(
            diagnostics
                .report()
                .failure
                .as_ref()
                .map(|failure| failure.code),
            Some(DiagnosticCode::HandshakeInvalidMessage)
        );
        let identity =
            crate::control::load_or_create_identity(fixture.directory.path(), fixture.machine)?;
        assert_eq!(identity.manifest.device_id(), Some(device));
        Ok(())
    }

    #[tokio::test]
    async fn resume_skips_enrollment_and_requires_session_ready()
    -> Result<(), Box<dyn std::error::Error>> {
        let device = Uuid::now_v7();
        let mut fixture =
            HandshakeFixture::with_authority(true, Diagnostics::disabled(), Some(device)).await?;
        let session = Uuid::now_v7();
        fixture
            .send(server_handshake_envelope::Body::SessionReady(
                SessionReady {
                    session_id: session.as_bytes().to_vec(),
                },
            ))
            .await?;
        let (outcome, diagnostics) = fixture.task.await?;
        assert!(matches!(outcome?, HandshakeOutcome::Active(id) if id == *session.as_bytes()));
        assert!(!diagnostics.report().events.iter().any(|event| matches!(
            event.stage,
            DiagnosticStage::PendingReview | DiagnosticStage::AuthorityInstallation
        )));

        for body in [
            server_handshake_envelope::Body::EnrollmentReviewStatus(EnrollmentReviewStatus {
                state: EnrollmentReviewState::PendingReview.into(),
                error_code: String::new(),
            }),
            server_handshake_envelope::Body::EnrollmentActivated(EnrollmentAuthority {
                device_id: Uuid::now_v7().to_string(),
            }),
        ] {
            let mut fixture =
                HandshakeFixture::with_authority(true, Diagnostics::disabled(), Some(device))
                    .await?;
            fixture.send(body).await?;
            let (outcome, diagnostics) = fixture.task.await?;
            assert!(matches!(outcome?, HandshakeOutcome::Retry));
            assert_eq!(
                diagnostics.report().connection.handshake_step,
                Some(DiagnosticHandshakeStep::ProofSent)
            );
            assert_eq!(
                diagnostics
                    .report()
                    .failure
                    .as_ref()
                    .map(|failure| failure.code),
                Some(DiagnosticCode::HandshakeInvalidMessage)
            );
            let identity =
                crate::control::load_or_create_identity(fixture.directory.path(), fixture.machine)?;
            assert_eq!(identity.manifest.device_id(), Some(device));
        }
        Ok(())
    }

    #[tokio::test]
    async fn malformed_peer_code_is_never_published() -> Result<(), Box<dyn std::error::Error>> {
        for token in ["SECRET body".to_owned(), "A".repeat(65)] {
            let mut fixture = HandshakeFixture::new(true).await?;
            fixture
                .send(server_handshake_envelope::Body::EnrollmentReviewStatus(
                    EnrollmentReviewStatus {
                        state: EnrollmentReviewState::Denied.into(),
                        error_code: token.clone(),
                    },
                ))
                .await?;
            let (outcome, diagnostics) = fixture.task.await?;
            assert!(matches!(outcome?, HandshakeOutcome::Retry));
            assert_eq!(diagnostics.report().connection.peer_error_code, None);
            assert_eq!(
                diagnostics.report().connection.handshake_step,
                Some(DiagnosticHandshakeStep::ProofSent)
            );
            assert_eq!(
                diagnostics
                    .report()
                    .failure
                    .as_ref()
                    .map(|failure| failure.code),
                Some(DiagnosticCode::HandshakeInvalidMessage)
            );
            assert!(!serde_json::to_string(diagnostics.report())?.contains(&token));
        }
        Ok(())
    }

    #[tokio::test]
    async fn authority_is_reported_only_after_install_and_session_ready_keeps_resume_identity()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut fixture = HandshakeFixture::new(true).await?;
        let device = Uuid::now_v7();
        fixture
            .send(server_handshake_envelope::Body::EnrollmentActivated(
                EnrollmentAuthority {
                    device_id: device.to_string(),
                },
            ))
            .await?;
        let ready = fixture.server.next().await.ok_or("missing ready")??;
        let Message::Binary(bytes) = ready else {
            return Err("expected ready".into());
        };
        assert!(matches!(
            ClientHandshakeEnvelope::decode(bytes)?.body,
            Some(client_handshake_envelope::Body::EnrollmentReady(_))
        ));
        let session = Uuid::now_v7();
        fixture
            .send(server_handshake_envelope::Body::SessionReady(
                SessionReady {
                    session_id: session.as_bytes().to_vec(),
                },
            ))
            .await?;
        let (outcome, diagnostics) = fixture.task.await?;
        assert!(matches!(outcome?, HandshakeOutcome::Active(id) if id == *session.as_bytes()));
        assert_eq!(
            diagnostics.report().connection.device_id.as_deref(),
            Some(device.to_string().as_str())
        );
        assert_eq!(
            diagnostics.report().connection.handshake_step,
            Some(DiagnosticHandshakeStep::SessionReady)
        );
        assert_eq!(
            diagnostics.report().last_completed_stage,
            Some(DiagnosticStage::Handshake)
        );
        let identity =
            crate::control::load_or_create_identity(fixture.directory.path(), fixture.machine)?;
        assert_eq!(identity.manifest.device_id(), Some(device));
        Ok(())
    }

    #[tokio::test]
    async fn uninstalled_authority_never_becomes_diagnostic_device_id()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut fixture = HandshakeFixture::new(true).await?;
        fixture
            .send(server_handshake_envelope::Body::EnrollmentActivated(
                EnrollmentAuthority {
                    device_id: "SECRET invalid authority".to_owned(),
                },
            ))
            .await?;
        let (outcome, diagnostics) = fixture.task.await?;
        assert!(matches!(
            outcome,
            Err(ControlLoopError::AuthorityPersistence { .. })
        ));
        assert_eq!(diagnostics.report().connection.device_id, None);
        assert_eq!(
            diagnostics.report().connection.handshake_step,
            Some(DiagnosticHandshakeStep::AuthorityReceived)
        );
        assert_eq!(
            diagnostics
                .report()
                .failure
                .as_ref()
                .map(|failure| failure.code),
            Some(DiagnosticCode::AuthorityPersistenceFailed)
        );
        assert!(!serde_json::to_string(diagnostics.report())?.contains("SECRET"));
        Ok(())
    }

    #[tokio::test]
    async fn silent_handshake_preserves_awaiting_challenge_on_timeout()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture = HandshakeFixture::new(false).await?;
        tokio::task::yield_now().await;
        tokio::time::pause();
        tokio::time::advance(HANDSHAKE_TIMEOUT).await;
        let (outcome, diagnostics) = fixture.task.await?;
        tokio::time::resume();
        assert!(matches!(outcome?, HandshakeOutcome::Retry));
        assert_eq!(
            diagnostics.report().connection.handshake_step,
            Some(DiagnosticHandshakeStep::AwaitingChallenge)
        );
        assert_eq!(
            diagnostics
                .report()
                .failure
                .as_ref()
                .map(|failure| failure.code),
            Some(DiagnosticCode::HandshakeTimeout)
        );
        Ok(())
    }

    #[tokio::test]
    async fn repeated_timeouts_survive_transport_success_until_active_resets_accounting()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut diagnostics = Diagnostics::disabled();
        for count in 1..=2 {
            let fixture = HandshakeFixture::with_diagnostics(false, diagnostics).await?;
            tokio::task::yield_now().await;
            tokio::time::pause();
            tokio::time::advance(HANDSHAKE_TIMEOUT).await;
            let (outcome, received) = fixture.task.await?;
            tokio::time::resume();
            assert!(matches!(outcome?, HandshakeOutcome::Retry));
            assert_eq!(
                received
                    .report()
                    .failure
                    .as_ref()
                    .map(|failure| failure.consecutive_count),
                Some(count)
            );
            diagnostics = received;
        }
        diagnostics.reset_failures();
        diagnostics.set_stage(DiagnosticStage::Active, DiagnosticStatus::Active);
        assert_eq!(diagnostics.report().failure, None);
        diagnostics.failure(DiagnosticCode::HandshakeTimeout, DiagnosticStatus::Retrying);
        assert_eq!(
            diagnostics
                .report()
                .failure
                .as_ref()
                .map(|failure| failure.consecutive_count),
            Some(1)
        );
        Ok(())
    }

    #[tokio::test]
    async fn repeated_pending_then_denied_keeps_accounting_but_pending_is_not_failure()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut diagnostics = Diagnostics::disabled();
        for count in 1..=2 {
            let mut fixture = HandshakeFixture::with_diagnostics(true, diagnostics).await?;
            fixture
                .send(server_handshake_envelope::Body::EnrollmentReviewStatus(
                    EnrollmentReviewStatus {
                        state: EnrollmentReviewState::PendingReview.into(),
                        error_code: String::new(),
                    },
                ))
                .await?;
            fixture
                .send(server_handshake_envelope::Body::EnrollmentReviewStatus(
                    EnrollmentReviewStatus {
                        state: EnrollmentReviewState::Denied.into(),
                        error_code: "REVIEW_DENIED".to_owned(),
                    },
                ))
                .await?;
            let (outcome, received) = fixture.task.await?;
            assert!(matches!(outcome?, HandshakeOutcome::Retry));
            assert_eq!(
                received
                    .report()
                    .failure
                    .as_ref()
                    .map(|failure| failure.consecutive_count),
                Some(count)
            );
            diagnostics = received;
        }
        Ok(())
    }

    #[tokio::test]
    async fn production_socket_limits_classify_oversized_input_as_invalid_message()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut fixture = HandshakeFixture::new(false).await?;
        fixture
            .server
            .send(Message::Binary(vec![0; MAX_MESSAGE_BYTES + 1].into()))
            .await?;
        let (outcome, diagnostics) = fixture.task.await?;
        assert!(matches!(outcome?, HandshakeOutcome::Retry));
        assert_eq!(
            diagnostics
                .report()
                .failure
                .as_ref()
                .map(|failure| failure.code),
            Some(DiagnosticCode::HandshakeInvalidMessage)
        );
        Ok(())
    }

    #[test]
    fn malformed_handshake_payloads_are_fixed_invalid_message_codes() {
        for message in [
            Message::Binary(vec![0xff].into()),
            Message::Text("SECRET".into()),
            Message::Binary(vec![0; MAX_MESSAGE_BYTES + 1].into()),
        ] {
            assert_eq!(
                decode_handshake(Some(Ok(message))),
                Err(DiagnosticCode::HandshakeInvalidMessage)
            );
        }
        assert_eq!(
            decode_handshake(None),
            Err(DiagnosticCode::HandshakeDisconnected)
        );
    }

    #[tokio::test]
    async fn handshake_reports_the_local_ip_of_its_actual_socket()
    -> Result<(), Box<dyn std::error::Error>> {
        use natsume_workstation_protocol::generated::{ServerChallenge, client_proof};
        use tokio::net::{TcpListener, TcpStream};
        use tokio_tungstenite::{
            MaybeTlsStream, WebSocketStream,
            tungstenite::{Message, protocol::Role},
        };

        let directory = tempfile::tempdir()?;
        let machine = Uuid::from_u128(0xa9aa_9d04_3ece_5567_8260_9109_30ff_5e03);
        let mut identity = crate::control::load_or_create_identity(directory.path(), machine)?;
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let tcp = TcpStream::connect(listener.local_addr()?).await?;
        let (peer, address) = listener.accept().await?;
        let mut client =
            WebSocketStream::from_raw_socket(MaybeTlsStream::Plain(tcp), Role::Client, None).await;
        let mut server = WebSocketStream::from_raw_socket(peer, Role::Server, None).await;
        let task = tokio::spawn(async move {
            let mut diagnostics = Diagnostics::disabled();
            let outcome = handshake(
                &mut client,
                &mut identity,
                machine,
                EnrollmentEvidenceQuality::Strong,
                &mut diagnostics,
            )
            .await;
            (outcome, diagnostics)
        });
        let challenge = ServerChallenge {
            challenge_nonce: vec![0x12; 32],
        };
        server
            .send(Message::Binary(
                ServerHandshakeEnvelope {
                    body: Some(server_handshake_envelope::Body::ServerChallenge(
                        challenge.clone(),
                    )),
                }
                .encode_to_vec()
                .into(),
            ))
            .await?;
        let Message::Binary(bytes) = timeout(std::time::Duration::from_secs(5), server.next())
            .await?
            .ok_or("closed socket")??
        else {
            return Err("expected ClientProof".into());
        };
        let Some(client_handshake_envelope::Body::ClientProof(proof)) =
            ClientHandshakeEnvelope::decode(bytes)?.body
        else {
            return Err("missing ClientProof".into());
        };
        assert_eq!(
            proof.client_ip.as_deref(),
            Some(address.ip().to_string().as_str())
        );
        let Some(client_proof::Purpose::Enrollment(enrollment)) = &proof.purpose else {
            return Err("missing enrollment".into());
        };
        let public_key: [u8; 32] = enrollment.candidate_public_key.as_slice().try_into()?;
        assert_eq!(
            natsume_workstation_protocol::verify_client_proof(&public_key, &challenge, &proof),
            Ok(())
        );
        server.close(None).await?;
        let (outcome, diagnostics) = task.await?;
        assert!(matches!(outcome?, HandshakeOutcome::Retry));
        assert_eq!(
            diagnostics.report().connection.local_ip.as_deref(),
            Some(address.ip().to_string().as_str())
        );
        assert_eq!(
            diagnostics.report().connection.handshake_step,
            Some(DiagnosticHandshakeStep::ProofSent)
        );
        assert_eq!(
            diagnostics
                .report()
                .failure
                .as_ref()
                .map(|failure| failure.code),
            Some(DiagnosticCode::HandshakeDisconnected)
        );
        Ok(())
    }

    #[test]
    fn session_ready_requires_an_exact_uuid_v7() {
        let valid = Uuid::from_u128(0x0190_0000_0000_7000_8000_0000_0000_0001);
        assert_eq!(parse_session_id(valid.as_bytes()), Some(*valid.as_bytes()));
        assert_eq!(
            parse_session_id(Uuid::new_v5(&Uuid::NAMESPACE_OID, b"lease").as_bytes()),
            None
        );
        assert_eq!(parse_session_id(&[0; 15]), None);
    }
}
