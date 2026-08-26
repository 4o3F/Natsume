use ed25519_dalek::{Signature, Signer as _, SigningKey, VerifyingKey};
use prost::Message as _;
use sha2::{Digest as _, Sha256};

use crate::{
    CONTROL_ROUTE, CONTROL_SUBPROTOCOL,
    generated::{ClientProof, ServerChallenge},
};

const CLIENT_PROOF_DOMAIN: &[u8] = b"NATSUME-DEVICE-CONTROL-CLIENT-PROOF\0";

/// Cryptographic failures independent of Enrollment and Resume semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProofVerificationError {
    InvalidPublicKey,
    WeakPublicKey,
    InvalidSignature,
}

/// Hashes the canonical Client proof transcript.
///
/// The signature field is always omitted from the signed representation. The
/// caller remains responsible for validating every semantic field.
#[must_use]
pub fn client_proof_signing_digest(challenge: &ServerChallenge, proof: &ClientProof) -> [u8; 32] {
    let mut unsigned_proof = proof.clone();
    unsigned_proof.signature.clear();

    let mut digest = Sha256::new();
    digest.update(CLIENT_PROOF_DOMAIN);
    digest.update(CONTROL_ROUTE.as_bytes());
    digest.update([0]);
    digest.update(CONTROL_SUBPROTOCOL.as_bytes());
    digest.update([0]);
    digest.update(challenge.encode_length_delimited_to_vec());
    digest.update(unsigned_proof.encode_length_delimited_to_vec());
    digest.finalize().into()
}

/// Signs a Client proof without applying Enrollment or Resume semantics.
#[must_use]
pub fn sign_client_proof(
    signing_key: &SigningKey,
    challenge: &ServerChallenge,
    mut proof: ClientProof,
) -> ClientProof {
    let digest = client_proof_signing_digest(challenge, &proof);
    proof.signature = signing_key.sign(&digest).to_bytes().to_vec();
    proof
}

/// Strictly verifies a Client proof against a caller-selected authoritative key.
///
/// # Errors
///
/// Returns a cryptographic error for an invalid or weak public key, an invalid
/// signature encoding, or a failed strict Ed25519 verification.
pub fn verify_client_proof(
    authoritative_public_key: &[u8],
    challenge: &ServerChallenge,
    proof: &ClientProof,
) -> Result<(), ProofVerificationError> {
    let key = VerifyingKey::try_from(authoritative_public_key)
        .map_err(|_| ProofVerificationError::InvalidPublicKey)?;
    if key.is_weak() {
        return Err(ProofVerificationError::WeakPublicKey);
    }
    let signature = Signature::try_from(proof.signature.as_slice())
        .map_err(|_| ProofVerificationError::InvalidSignature)?;
    let digest = client_proof_signing_digest(challenge, proof);
    key.verify_strict(&digest, &signature)
        .map_err(|_| ProofVerificationError::InvalidSignature)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generated::{EnrollmentAttempt, EnrollmentEvidenceQuality, client_proof::Purpose};

    fn fixture() -> (ServerChallenge, ClientProof) {
        let challenge = ServerChallenge {
            challenge_nonce: (0_u8..32).collect(),
        };
        let proof = ClientProof {
            daemon_version: "2.0.0".to_owned(),
            agent_version: "2.0.0".to_owned(),
            machine_hardware_id: "a9aa9d04-3ece-5567-8260-910930ff5e03".to_owned(),
            signature: vec![0xAA; 64],
            purpose: Some(Purpose::Enrollment(EnrollmentAttempt {
                enrollment_id: "01900000-0000-7000-8000-000000000001".to_owned(),
                candidate_public_key: vec![0x11; 32],
                evidence_quality: EnrollmentEvidenceQuality::Strong.into(),
            })),
        };
        (challenge, proof)
    }

    #[test]
    fn signing_digest_has_a_stable_golden_and_ignores_the_signature_field() {
        let (challenge, proof) = fixture();
        let digest = client_proof_signing_digest(&challenge, &proof);
        assert_eq!(
            digest,
            [
                40, 7, 132, 243, 49, 245, 220, 233, 112, 97, 171, 41, 44, 68, 254, 124, 38, 143,
                193, 86, 145, 207, 183, 228, 15, 64, 229, 29, 249, 110, 208, 114,
            ]
        );

        let mut different_signature = proof;
        different_signature.signature = vec![0x55; 64];
        assert_eq!(
            client_proof_signing_digest(&challenge, &different_signature),
            digest
        );
    }

    #[test]
    fn strict_verification_rejects_a_mutated_proof() {
        let signing_key = SigningKey::from_bytes(&[0x42; 32]);
        let (challenge, proof) = fixture();
        let proof = sign_client_proof(&signing_key, &challenge, proof);
        let public_key = signing_key.verifying_key().to_bytes();
        assert_eq!(verify_client_proof(&public_key, &challenge, &proof), Ok(()));

        let mut mutated = proof;
        mutated.agent_version = "2.0.1".to_owned();
        assert_eq!(
            verify_client_proof(&public_key, &challenge, &mutated),
            Err(ProofVerificationError::InvalidSignature)
        );
    }
}
