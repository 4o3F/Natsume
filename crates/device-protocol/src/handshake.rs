mod client_init;
mod transcript;

use snafu::Snafu;
use uuid::{Uuid, Variant, Version};

use crate::generated::ProofIntent;

pub use client_init::{
    canonical_client_init_sha256, decode_client_init, encode_client_init_canonical,
    validate_client_init,
};
pub use transcript::{ControlKeyId, proof_transcript, verify_proof_strict};

/// Typed failures for control handshake, verification, and canonical wire handling.
///
/// Variants deliberately contain no peer-provided bytes or strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Snafu)]
pub enum HandshakeError {
    #[snafu(display("control protocol version is invalid"))]
    ProtocolVersion,

    #[snafu(display("control challenge ID is invalid"))]
    ChallengeId,

    #[snafu(display("control proof does not match its challenge"))]
    ChallengeMismatch,

    #[snafu(display("control server nonce length is invalid"))]
    ServerNonceLength,

    #[snafu(display("control client nonce length is invalid"))]
    ClientNonceLength,

    #[snafu(display("control public key length is invalid"))]
    ControlPublicKeyLength,

    #[snafu(display("control public key is invalid"))]
    ControlPublicKey,

    #[snafu(display("control public key is weak"))]
    WeakControlPublicKey,

    #[snafu(display("Machine Hardware ID is invalid"))]
    MachineHardwareId,

    #[snafu(display("claimed Device ID is invalid"))]
    ClaimedDeviceId,

    #[snafu(display("control proof intent is invalid"))]
    ProofIntent,

    #[snafu(display("Enrollment attempt ID is invalid"))]
    EnrollmentAttemptId,

    #[snafu(display("ClientInit digest length is invalid"))]
    ClientInitHashLength,

    #[snafu(display("control proof signature length is invalid"))]
    SignatureLength,

    #[snafu(display("control proof signature is invalid"))]
    Signature,

    #[snafu(display("control proof context is invalid"))]
    TranscriptContext,

    #[snafu(display("ClientInit hardware claim is invalid"))]
    HardwareClaim,

    #[snafu(display("ClientInit evidence quality is invalid"))]
    EvidenceQuality,

    #[snafu(display("ClientInit collection completeness is invalid"))]
    CollectionCompleteness,

    #[snafu(display("ClientInit hardware candidate is invalid"))]
    HardwareCandidate,

    #[snafu(display("ClientInit last-applied hash is invalid"))]
    LastAppliedHash,

    #[snafu(display("ClientInit exceeds the protocol size limit"))]
    ClientInitTooLarge,

    #[snafu(display("ClientInit protobuf decoding failed"))]
    ClientInitDecode,
}

pub(super) fn exact<const N: usize>(
    bytes: &[u8],
    error: HandshakeError,
) -> Result<[u8; N], HandshakeError> {
    bytes.try_into().map_err(|_| error)
}

pub(super) fn proof_intent_byte(value: i32) -> Result<u8, HandshakeError> {
    let intent = ProofIntent::try_from(value).map_err(|_| HandshakeError::ProofIntent)?;
    match intent {
        ProofIntent::Unspecified => Err(HandshakeError::ProofIntent),
        ProofIntent::FirstEnrollment => Ok(1),
        ProofIntent::Resume => Ok(2),
        ProofIntent::RotateControlKey => Ok(3),
        ProofIntent::RecoverControlKey => Ok(4),
        ProofIntent::RefreshGatewayCredential => Ok(5),
    }
}

pub(super) fn canonical_uuid(
    value: &str,
    version: Version,
    error: HandshakeError,
) -> Result<[u8; 16], HandshakeError> {
    let uuid = Uuid::parse_str(value).map_err(|_| error)?;
    if uuid.get_version() != Some(version)
        || uuid.get_variant() != Variant::RFC4122
        || uuid.to_string() != value
    {
        return Err(error);
    }
    Ok(*uuid.as_bytes())
}

pub(super) fn validate_uuid_bytes(
    value: [u8; 16],
    version: Version,
    error: HandshakeError,
) -> Result<(), HandshakeError> {
    let uuid = Uuid::from_bytes(value);
    if uuid.get_version() != Some(version) || uuid.get_variant() != Variant::RFC4122 {
        return Err(error);
    }
    Ok(())
}
