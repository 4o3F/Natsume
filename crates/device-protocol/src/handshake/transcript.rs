use ed25519_dalek::{Signature, Signer as _, SigningKey, VerifyingKey};
use prost::Message as _;
use sha2::{Digest as _, Sha256};

use super::HandshakeError;
use crate::{
    CONTROL_ROUTE, CONTROL_SUBPROTOCOL,
    generated::{ClientProof, ServerChallenge},
};

const PROOF_DIGEST_DOMAIN: &[u8] = b"NATSUME-WSS-CONTROL-PROOF-v2\0";
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

/// Hashes the canonical control-proof input without allocating a combined transcript.
#[must_use]
pub fn proof_signing_digest(challenge: &ServerChallenge, proof: &ClientProof) -> [u8; 32] {
    let mut unsigned_proof = proof.clone();
    unsigned_proof.signature.clear();

    let mut digest = Sha256::new();
    digest.update(PROOF_DIGEST_DOMAIN);
    digest.update(CONTROL_ROUTE.as_bytes());
    digest.update([0]);
    digest.update(CONTROL_SUBPROTOCOL.as_bytes());
    digest.update([0]);
    digest.update(challenge.encode_length_delimited_to_vec());
    digest.update(unsigned_proof.encode_length_delimited_to_vec());
    digest.finalize().into()
}

/// Signs the fixed control-proof digest without validating typed fields.
#[must_use]
pub fn sign_client_proof(
    signing_key: &SigningKey,
    challenge: &ServerChallenge,
    mut proof: ClientProof,
) -> ClientProof {
    let proof_digest = proof_signing_digest(challenge, &proof);
    proof.signature = signing_key.sign(&proof_digest).to_bytes().to_vec();
    proof
}

/// Cryptographically verifies one control proof without applying protocol semantics.
///
/// The caller must select `authoritative_public_key` from the intent-authoritative source. This
/// function never infers the verification key from a proof candidate.
///
/// # Errors
///
/// Returns a redacted typed error when the public key or signature cannot be parsed, the public
/// key is weak, or strict Ed25519 verification fails.
pub fn verify_proof_strict(
    authoritative_public_key: &[u8],
    challenge: &ServerChallenge,
    proof: &ClientProof,
) -> Result<(), HandshakeError> {
    let key = VerifyingKey::try_from(authoritative_public_key)
        .map_err(|_| HandshakeError::ControlPublicKey)?;
    if key.is_weak() {
        return Err(HandshakeError::WeakControlPublicKey);
    }
    let signature =
        Signature::try_from(proof.signature.as_slice()).map_err(|_| HandshakeError::Signature)?;
    let proof_digest = proof_signing_digest(challenge, proof);
    key.verify_strict(&proof_digest, &signature)
        .map_err(|_| HandshakeError::Signature)
}
