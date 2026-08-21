mod transcript;

use snafu::Snafu;

pub use transcript::{ControlKeyId, proof_signing_digest, sign_client_proof, verify_proof_strict};

/// Typed failures for control handshake, verification, and canonical wire handling.
///
/// Variants deliberately contain no peer-provided bytes or strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Snafu)]
pub enum HandshakeError {
    #[snafu(display("control protocol version is invalid"))]
    ProtocolVersion,

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

    #[snafu(display("control proof signature is invalid"))]
    Signature,

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
