use ed25519_dalek::VerifyingKey;
use prost::Message as _;
use sha2::{Digest as _, Sha256};
use uuid::Version;

use super::{HandshakeError, canonical_uuid, exact, proof_intent_byte, validate_uuid_bytes};
use crate::{
    CONTROL_MAX_CLIENT_INIT_BYTES, CONTROL_WIRE_VERSION,
    generated::{ClientInit, CollectionCompleteness, EvidenceQuality, HardwareClaim},
};

/// Applies the closed semantic checks shared by `ClientInit` encoders and future consumers.
///
/// # Errors
///
/// Returns a redacted typed error for an oversized value, an invalid protocol or intent,
/// malformed identifiers or fixed-size values, weak or invalid public keys, an invalid hardware
/// claim, or an incoherent last-applied generation and hash.
pub fn validate_client_init(value: &ClientInit) -> Result<(), HandshakeError> {
    if value.encoded_len() > CONTROL_MAX_CLIENT_INIT_BYTES {
        return Err(HandshakeError::ClientInitTooLarge);
    }
    if value.protocol_version != CONTROL_WIRE_VERSION {
        return Err(HandshakeError::ProtocolVersion);
    }
    let intent = proof_intent_byte(value.intent)?;
    canonical_uuid(
        &value.machine_hardware_id,
        Version::Sha1,
        HandshakeError::MachineHardwareId,
    )?;
    match (intent, value.claimed_device_id.as_deref()) {
        (1, None) => {}
        (1, Some(_)) | (2..=5, None) => return Err(HandshakeError::ClaimedDeviceId),
        (2..=5, Some(device_id)) => {
            canonical_uuid(
                device_id,
                Version::SortRand,
                HandshakeError::ClaimedDeviceId,
            )?;
        }
        _ => return Err(HandshakeError::ProofIntent),
    }

    let public_key = exact::<32>(
        &value.control_public_key,
        HandshakeError::ControlPublicKeyLength,
    )?;
    let key =
        VerifyingKey::from_bytes(&public_key).map_err(|_| HandshakeError::ControlPublicKey)?;
    if key.is_weak() {
        return Err(HandshakeError::WeakControlPublicKey);
    }
    exact::<32>(&value.client_nonce, HandshakeError::ClientNonceLength)?;
    let enrollment_attempt_id = exact::<16>(
        &value.enrollment_attempt_id,
        HandshakeError::EnrollmentAttemptId,
    )?;
    validate_uuid_bytes(
        enrollment_attempt_id,
        Version::SortRand,
        HandshakeError::EnrollmentAttemptId,
    )?;

    let Some(hardware_claim) = value.hardware_claim.as_ref() else {
        return Err(HandshakeError::HardwareClaim);
    };
    validate_hardware_claim(hardware_claim)?;

    if (value.last_applied_generation == 0 && !value.last_applied_hash.is_empty())
        || (value.last_applied_generation > 0 && value.last_applied_hash.len() != 32)
    {
        return Err(HandshakeError::LastAppliedHash);
    }
    Ok(())
}

/// Decodes and semantically validates one bounded `ClientInit` message.
///
/// Incoming protobuf field order and equivalent wire encodings are normalized by Prost. Callers
/// derive the signed digest from [`encode_client_init_canonical`] on the returned typed value, not
/// from the received bytes.
///
/// # Errors
///
/// Returns a redacted typed error for an oversized message, malformed protobuf, or invalid
/// `ClientInit` semantics.
pub fn decode_client_init(bytes: &[u8]) -> Result<ClientInit, HandshakeError> {
    if bytes.len() > CONTROL_MAX_CLIENT_INIT_BYTES {
        return Err(HandshakeError::ClientInitTooLarge);
    }
    let value = ClientInit::decode(bytes).map_err(|_| HandshakeError::ClientInitDecode)?;
    validate_client_init(&value)?;
    Ok(value)
}

/// Encodes one semantically valid `ClientInit` in deterministic Prost field order.
///
/// # Errors
///
/// Returns a redacted typed error when semantic validation fails or the encoded message exceeds
/// the hard limit.
pub fn encode_client_init_canonical(value: &ClientInit) -> Result<Vec<u8>, HandshakeError> {
    validate_client_init(value)?;
    Ok(value.encode_to_vec())
}

/// Hashes the deterministic Prost encoding of one semantically valid `ClientInit`.
///
/// # Errors
///
/// Returns the same redacted semantic or size error as [`encode_client_init_canonical`].
pub fn canonical_client_init_sha256(value: &ClientInit) -> Result<[u8; 32], HandshakeError> {
    Ok(Sha256::digest(encode_client_init_canonical(value)?).into())
}

fn validate_hardware_claim(value: &HardwareClaim) -> Result<(), HandshakeError> {
    let completeness = CollectionCompleteness::try_from(value.completeness)
        .map_err(|_| HandshakeError::CollectionCompleteness)?;
    if completeness != CollectionCompleteness::Complete {
        return Err(HandshakeError::CollectionCompleteness);
    }
    if !(2..=3).contains(&value.candidates.len()) {
        return Err(HandshakeError::HardwareClaim);
    }

    let mut seen_anchors = [false; 3];
    for candidate in &value.candidates {
        let anchor = match candidate.anchor_kind.as_str() {
            "dmi_system_uuid" => 0,
            "dmi_board_serial" => 1,
            "first_disk_serial" => 2,
            _ => return Err(HandshakeError::HardwareCandidate),
        };
        if seen_anchors[anchor] {
            return Err(HandshakeError::HardwareCandidate);
        }
        seen_anchors[anchor] = true;
        canonical_uuid(
            &candidate.candidate_id,
            Version::Sha1,
            HandshakeError::HardwareCandidate,
        )?;
        let quality = EvidenceQuality::try_from(candidate.quality)
            .map_err(|_| HandshakeError::EvidenceQuality)?;
        if quality == EvidenceQuality::Unspecified {
            return Err(HandshakeError::EvidenceQuality);
        }
    }
    Ok(())
}
