use futures_util::{SinkExt as _, StreamExt as _};
use natsume_device_protocol::{
    generated::{
        ClientHandshakeEnvelope, EnrollmentEvidenceQuality, EnrollmentReviewState,
        ServerHandshakeEnvelope, client_handshake_envelope, server_handshake_envelope,
    },
    is_valid_error_code_token,
};
use prost::Message as _;
use tokio::time::{Instant, sleep_until, timeout};
use uuid::{Uuid, Variant, Version};

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
) -> Result<HandshakeOutcome, ControlLoopError> {
    let Some(first) = receive(socket).await else {
        return Ok(HandshakeOutcome::Retry);
    };
    let challenge = match first.body {
        Some(server_handshake_envelope::Body::ServerChallenge(challenge))
            if challenge.challenge_nonce.len() == 32 =>
        {
            challenge
        }
        _ => return Ok(HandshakeOutcome::Retry),
    };

    let Ok(local_address) = socket.get_ref().get_ref().local_addr() else {
        return Ok(HandshakeOutcome::Retry);
    };
    let mut proof = identity.proof(&challenge, machine_hardware_id, evidence_quality);
    proof.client_ip = Some(local_address.ip().to_canonical().to_string());
    if !send(
        socket,
        ClientHandshakeEnvelope {
            body: Some(client_handshake_envelope::Body::ClientProof(proof)),
        },
    )
    .await
    {
        return Ok(HandshakeOutcome::Retry);
    }

    let mut review_seen = false;
    loop {
        let envelope = if review_seen && identity.is_enrolling() {
            receive_pending(socket).await
        } else {
            receive(socket).await
        };
        let Some(envelope) = envelope else {
            return Ok(HandshakeOutcome::Retry);
        };
        match envelope.body {
            Some(server_handshake_envelope::Body::EnrollmentReviewStatus(status))
                if identity.is_enrolling() =>
            {
                match EnrollmentReviewState::try_from(status.state) {
                    Ok(EnrollmentReviewState::PendingReview)
                        if !review_seen && status.error_code.is_empty() =>
                    {
                        review_seen = true;
                    }
                    Ok(EnrollmentReviewState::Denied)
                        if is_valid_error_code_token(&status.error_code) =>
                    {
                        return Ok(HandshakeOutcome::Retry);
                    }
                    Ok(
                        EnrollmentReviewState::Unspecified
                        | EnrollmentReviewState::PendingReview
                        | EnrollmentReviewState::Denied,
                    )
                    | Err(_) => return Ok(HandshakeOutcome::Retry),
                }
            }
            Some(server_handshake_envelope::Body::EnrollmentActivated(authority))
                if identity.is_enrolling() =>
            {
                identity
                    .install_authority(&authority)
                    .map_err(|source| ControlLoopError::AuthorityPersistence { source })?;
                if !send(
                    socket,
                    ClientHandshakeEnvelope {
                        body: Some(client_handshake_envelope::Body::EnrollmentReady(authority)),
                    },
                )
                .await
                {
                    return Ok(HandshakeOutcome::Retry);
                }
            }
            Some(server_handshake_envelope::Body::SessionReady(ready))
                if !identity.is_enrolling() =>
            {
                return Ok(match parse_session_id(&ready.session_id) {
                    Some(session_id) => HandshakeOutcome::Active(session_id),
                    None => HandshakeOutcome::Retry,
                });
            }
            _ => return Ok(HandshakeOutcome::Retry),
        }
    }
}

fn parse_session_id(value: &[u8]) -> Option<[u8; 16]> {
    let bytes: [u8; 16] = value.try_into().ok()?;
    let session_id = Uuid::from_bytes(bytes);
    (session_id.get_version() == Some(Version::SortRand)
        && session_id.get_variant() == Variant::RFC4122)
        .then_some(bytes)
}

async fn receive(socket: &mut Socket) -> Option<ServerHandshakeEnvelope> {
    match timeout(HANDSHAKE_TIMEOUT, socket.next()).await {
        Ok(Some(Ok(tokio_tungstenite::tungstenite::Message::Binary(bytes))))
            if bytes.len() <= MAX_MESSAGE_BYTES =>
        {
            ServerHandshakeEnvelope::decode(bytes).ok()
        }
        _ => None,
    }
}

async fn receive_pending(socket: &mut Socket) -> Option<ServerHandshakeEnvelope> {
    let mut heartbeat = heartbeat_interval();
    let server_deadline = sleep_until(Instant::now() + SERVER_SILENCE_TIMEOUT);
    tokio::pin!(server_deadline);
    loop {
        tokio::select! {
            message = socket.next() => {
                match message {
                    Some(Ok(tokio_tungstenite::tungstenite::Message::Binary(bytes)))
                        if bytes.len() <= MAX_MESSAGE_BYTES =>
                    {
                        return ServerHandshakeEnvelope::decode(bytes).ok();
                    }
                    Some(Ok(tokio_tungstenite::tungstenite::Message::Pong(payload)))
                        if payload.is_empty() =>
                    {
                        server_deadline
                            .as_mut()
                            .reset(Instant::now() + SERVER_SILENCE_TIMEOUT);
                    }
                    _ => return None,
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
                    return None;
                }
            }
            () = &mut server_deadline => return None,
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

    #[tokio::test]
    async fn handshake_reports_the_local_ip_of_its_actual_socket()
    -> Result<(), Box<dyn std::error::Error>> {
        use natsume_device_protocol::generated::{ServerChallenge, client_proof};
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
            handshake(
                &mut client,
                &mut identity,
                machine,
                EnrollmentEvidenceQuality::Strong,
            )
            .await
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
            natsume_device_protocol::verify_client_proof(&public_key, &challenge, &proof),
            Ok(())
        );
        server.close(None).await?;
        assert!(matches!(task.await??, HandshakeOutcome::Retry));
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
