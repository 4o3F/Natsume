use ed25519_dalek::{Signature, VerifyingKey};
use sha2::{Digest as _, Sha256};
use uuid::Version;

use super::{HandshakeError, canonical_uuid, exact, proof_intent_byte, validate_uuid_bytes};
use crate::{
    CONTROL_ROUTE, CONTROL_SUBPROTOCOL, CONTROL_WIRE_VERSION,
    generated::{ClientProof, ServerChallenge},
};

const TRANSCRIPT_DOMAIN: &[u8] = b"NATSUME-WSS-CONTROL-PROOF-v1\0";
const CONTROL_KEY_ID_DOMAIN: &[u8] = b"NATSUME-CONTROL-KEY-ID-v1\0";
const ED25519_ALGORITHM_ID: u8 = 0x01;

/// Stable SHA-256 identifier for an Ed25519 Device control public key.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ControlKeyId([u8; 32]);

impl ControlKeyId {
    /// Derives the versioned control-key identifier from one Ed25519 public key.
    #[must_use]
    pub fn derive(public_key: [u8; 32]) -> Self {
        let mut digest = Sha256::new();
        digest.update(CONTROL_KEY_ID_DOMAIN);
        digest.update([ED25519_ALGORITHM_ID]);
        digest.update(32_u16.to_be_bytes());
        digest.update(public_key);
        Self(digest.finalize().into())
    }

    /// Borrows the identifier bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// Builds the exact control-proof transcript.
///
/// # Errors
///
/// Returns a redacted typed error when a signed field has the wrong fixed size, a UUID has the
/// wrong version, variant, or canonical spelling, the intent is outside the closed vocabulary,
/// or the proof does not name the supplied challenge.
pub fn proof_transcript(
    challenge: &ServerChallenge,
    proof: &ClientProof,
) -> Result<Vec<u8>, HandshakeError> {
    if challenge.protocol_version != CONTROL_WIRE_VERSION {
        return Err(HandshakeError::ProtocolVersion);
    }

    let challenge_id = exact::<16>(&challenge.challenge_id, HandshakeError::ChallengeId)?;
    validate_uuid_bytes(challenge_id, Version::SortRand, HandshakeError::ChallengeId)?;
    let proof_challenge_id = exact::<16>(&proof.challenge_id, HandshakeError::ChallengeId)?;
    validate_uuid_bytes(
        proof_challenge_id,
        Version::SortRand,
        HandshakeError::ChallengeId,
    )?;
    if proof_challenge_id != challenge_id {
        return Err(HandshakeError::ChallengeMismatch);
    }

    let server_nonce = exact::<32>(&challenge.server_nonce, HandshakeError::ServerNonceLength)?;
    let client_nonce = exact::<32>(&proof.client_nonce, HandshakeError::ClientNonceLength)?;
    let public_key = exact::<32>(
        &proof.control_public_key,
        HandshakeError::ControlPublicKeyLength,
    )?;
    let machine_hardware_id = canonical_uuid(
        &proof.machine_hardware_id,
        Version::Sha1,
        HandshakeError::MachineHardwareId,
    )?;
    let claimed_device_id = proof
        .claimed_device_id
        .as_deref()
        .map(|value| canonical_uuid(value, Version::SortRand, HandshakeError::ClaimedDeviceId))
        .transpose()?;
    let intent = proof_intent_byte(proof.intent)?;
    let enrollment_attempt_id = exact::<16>(
        &proof.enrollment_attempt_id,
        HandshakeError::EnrollmentAttemptId,
    )?;
    validate_uuid_bytes(
        enrollment_attempt_id,
        Version::SortRand,
        HandshakeError::EnrollmentAttemptId,
    )?;
    let client_init_sha256 = exact::<32>(
        &proof.client_init_sha256,
        HandshakeError::ClientInitHashLength,
    )?;

    let mut transcript = Vec::with_capacity(268);
    transcript.extend_from_slice(TRANSCRIPT_DOMAIN);
    append_context(&mut transcript, CONTROL_ROUTE)?;
    append_context(&mut transcript, CONTROL_SUBPROTOCOL)?;
    transcript.extend_from_slice(&challenge_id);
    transcript.extend_from_slice(&server_nonce);
    transcript.extend_from_slice(&client_nonce);
    transcript.extend_from_slice(&challenge.protocol_version.to_be_bytes());
    transcript.push(intent);
    transcript.extend_from_slice(&public_key);
    transcript.extend_from_slice(&machine_hardware_id);
    transcript.push(u8::from(claimed_device_id.is_some()));
    if let Some(device_id) = claimed_device_id {
        transcript.extend_from_slice(&device_id);
    }
    transcript.extend_from_slice(&enrollment_attempt_id);
    transcript.extend_from_slice(&client_init_sha256);
    Ok(transcript)
}

/// Strictly verifies one typed control proof against its connection-local challenge.
///
/// # Errors
///
/// Returns a redacted typed error for invalid transcript fields, public-key parsing failures,
/// malformed signatures, weak-key signatures, or any other strict verification failure.
pub fn verify_proof_strict(
    challenge: &ServerChallenge,
    proof: &ClientProof,
) -> Result<(), HandshakeError> {
    let transcript = proof_transcript(challenge, proof)?;
    let public_key = exact::<32>(
        &proof.control_public_key,
        HandshakeError::ControlPublicKeyLength,
    )?;
    let signature = exact::<64>(&proof.signature, HandshakeError::SignatureLength)?;
    let key =
        VerifyingKey::from_bytes(&public_key).map_err(|_| HandshakeError::ControlPublicKey)?;
    let signature = Signature::from_bytes(&signature);
    key.verify_strict(&transcript, &signature)
        .map_err(|_| HandshakeError::Signature)
}

fn append_context(transcript: &mut Vec<u8>, value: &str) -> Result<(), HandshakeError> {
    let length = u16::try_from(value.len()).map_err(|_| HandshakeError::TranscriptContext)?;
    transcript.extend_from_slice(&length.to_be_bytes());
    transcript.extend_from_slice(value.as_bytes());
    Ok(())
}
