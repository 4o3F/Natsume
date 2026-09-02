use ed25519_dalek::{Signature, Signer as _, SigningKey, VerifyingKey};
use sha2::{Digest as _, Sha256};

use crate::{
    CONTROL_ROUTE, CONTROL_SUBPROTOCOL,
    generated::{ClientProof, ServerChallenge, client_proof},
};

const CLIENT_PROOF_DOMAIN: &[u8] = b"NATSUME-DEVICE-CONTROL-CLIENT-PROOF\0";
const CLIENT_PROOF_TRANSCRIPT_VERSION: u8 = 1;
const PURPOSE_MISSING: u8 = 0;
const PURPOSE_ENROLLMENT: u8 = 1;
const PURPOSE_RESUME: u8 = 2;

/// Cryptographic failures independent of Enrollment and Resume semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProofVerificationError {
    InvalidPublicKey,
    WeakPublicKey,
    InvalidSignature,
}

/// Hashes the canonical, versioned Client proof transcript.
///
/// Enrollment and Resume share exactly the same fields. The public key is the
/// candidate key for Enrollment and the Server-selected current authority for
/// Resume. Self-reported version and Enrollment review metadata, the signature
/// field, and Prost's wire encoding are not part of the identity proof. The
/// caller remains responsible for validating every semantic field.
#[must_use]
pub fn client_proof_signing_digest(
    public_key: &[u8; 32],
    challenge: &ServerChallenge,
    proof: &ClientProof,
) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(CLIENT_PROOF_DOMAIN);
    digest.update([CLIENT_PROOF_TRANSCRIPT_VERSION]);
    digest.update(CONTROL_ROUTE.as_bytes());
    digest.update([0]);
    digest.update(CONTROL_SUBPROTOCOL.as_bytes());
    digest.update([0]);
    digest.update(&challenge.challenge_nonce);
    digest.update(public_key);
    digest.update([match proof.purpose.as_ref() {
        None => PURPOSE_MISSING,
        Some(client_proof::Purpose::Enrollment(_)) => PURPOSE_ENROLLMENT,
        Some(client_proof::Purpose::Resume(_)) => PURPOSE_RESUME,
    }]);
    digest.update(proof.machine_hardware_id.as_bytes());
    digest.finalize().into()
}

/// Signs a Client proof without applying Enrollment or Resume semantics.
#[must_use]
pub fn sign_client_proof(
    signing_key: &SigningKey,
    challenge: &ServerChallenge,
    mut proof: ClientProof,
) -> ClientProof {
    let public_key = signing_key.verifying_key();
    let digest = client_proof_signing_digest(public_key.as_bytes(), challenge, &proof);
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
    let digest = client_proof_signing_digest(key.as_bytes(), challenge, proof);
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
                candidate_public_key: vec![0x11; 32],
                evidence_quality: EnrollmentEvidenceQuality::Strong.into(),
            })),
        };
        (challenge, proof)
    }

    #[test]
    fn signing_digest_has_a_stable_golden_and_ignores_non_identity_fields() {
        let (challenge, proof) = fixture();
        let public_key = [0x11; 32];
        let digest = client_proof_signing_digest(&public_key, &challenge, &proof);
        assert_eq!(
            digest,
            [
                114, 19, 201, 89, 60, 14, 5, 183, 109, 114, 52, 149, 166, 44, 87, 118, 54, 149, 6,
                150, 213, 143, 60, 198, 116, 125, 25, 97, 62, 195, 87, 19,
            ]
        );

        let mut different_signature = proof;
        different_signature.signature = vec![0x55; 64];
        different_signature.daemon_version = "2.0.1".to_owned();
        different_signature.agent_version = "2.0.1".to_owned();
        let Some(Purpose::Enrollment(attempt)) = different_signature.purpose.as_mut() else {
            panic!("fixture lost its Enrollment purpose");
        };
        attempt.evidence_quality = EnrollmentEvidenceQuality::Medium.into();
        assert_eq!(
            client_proof_signing_digest(&public_key, &challenge, &different_signature),
            digest
        );
    }

    #[test]
    fn strict_verification_binds_the_common_identity_fields() {
        let signing_key = SigningKey::from_bytes(&[0x42; 32]);
        let (challenge, proof) = fixture();
        let proof = sign_client_proof(&signing_key, &challenge, proof);
        let public_key = signing_key.verifying_key().to_bytes();
        assert_eq!(verify_client_proof(&public_key, &challenge, &proof), Ok(()));

        let mut different_hardware = proof.clone();
        different_hardware.machine_hardware_id = "01900000-0000-7000-8000-000000000001".to_owned();
        assert_eq!(
            verify_client_proof(&public_key, &challenge, &different_hardware),
            Err(ProofVerificationError::InvalidSignature)
        );

        let mut different_purpose = proof;
        different_purpose.purpose = Some(Purpose::Resume(crate::generated::ResumeSession {}));
        assert_eq!(
            verify_client_proof(&public_key, &challenge, &different_purpose),
            Err(ProofVerificationError::InvalidSignature)
        );
    }
}
