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

    let proof = identity.proof(&challenge, machine_hardware_id, evidence_quality);
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
