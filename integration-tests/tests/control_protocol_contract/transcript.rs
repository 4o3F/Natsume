use ed25519_dalek::{Signature, SigningKey, VerifyingKey};
use natsume_device_protocol::{
    CONTROL_ROUTE, CONTROL_SUBPROTOCOL, ControlKeyId, HandshakeError, canonical_client_init_sha256,
    encode_client_init_canonical,
    generated::{ClientProof, ProofIntent, ServerChallenge},
    proof_signing_digest, sign_client_proof, verify_proof_strict,
};
use sha2::{Digest as _, Sha256};

use super::fixture::{
    CLAIMED_DEVICE_ID, CONTROL_KEY_SEED_PUBLIC_KEY, EXPECTED_CONTROL_KEY_ID,
    OTHER_MACHINE_HARDWARE_ID, PROTO_CLIENT_INIT_SHA256, PROTO_EXPECTED_SIGNATURE,
    PROTO_PROOF_DIGEST, RFC8032_PUBLIC_KEY, canonical_client_init, golden_challenge, golden_proof,
    independent_proof_digest,
};

const OTHER_CLAIMED_DEVICE_ID: &str = "01900000-0000-7000-8000-000000000004";

#[test]
fn canonical_client_init_proof_digest_and_signature_goldens_match() {
    let client_init = canonical_client_init(None);
    let Ok(client_init_bytes) = encode_client_init_canonical(&client_init) else {
        panic!("canonical ClientInit must encode");
    };
    assert_eq!(client_init_bytes.len(), 371);
    let client_init_sha256: [u8; 32] = Sha256::digest(&client_init_bytes).into();
    assert_eq!(client_init_sha256, PROTO_CLIENT_INIT_SHA256);
    assert_eq!(
        canonical_client_init_sha256(&client_init),
        Ok(PROTO_CLIENT_INIT_SHA256)
    );

    let challenge = golden_challenge();
    let proof = golden_proof();
    assert_eq!(proof.client_init_sha256, client_init_sha256);
    let key_id = ControlKeyId::derive(CONTROL_KEY_SEED_PUBLIC_KEY);
    assert_eq!(key_id.as_bytes(), &EXPECTED_CONTROL_KEY_ID);

    let proof_digest = proof_signing_digest(&challenge, &proof);
    assert_eq!(proof_digest.len(), 32);
    assert_eq!(
        proof_digest,
        independent_proof_digest(&challenge, &proof, CONTROL_ROUTE, CONTROL_SUBPROTOCOL)
    );
    assert_eq!(proof_digest, PROTO_PROOF_DIGEST);

    let signed = fixture_signed_proof(&challenge, proof.clone());
    assert_eq!(signed.signature, PROTO_EXPECTED_SIGNATURE);
    assert_eq!(signed, proof);
    assert_signature_accepts(&proof, &proof_digest);
    if let Err(error) = verify_proof_strict(&challenge, &proof) {
        panic!("golden proof must verify strictly: {error:?}");
    }
}

#[test]
fn signature_is_omitted_from_the_canonical_digest() {
    let challenge = golden_challenge();
    let signed = golden_proof();
    let mut unsigned = signed.clone();
    unsigned.signature.clear();

    assert_eq!(
        proof_signing_digest(&challenge, &signed),
        proof_signing_digest(&challenge, &unsigned)
    );
    assert_eq!(
        independent_proof_digest(&challenge, &signed, CONTROL_ROUTE, CONTROL_SUBPROTOCOL),
        independent_proof_digest(&challenge, &unsigned, CONTROL_ROUTE, CONTROL_SUBPROTOCOL)
    );
}

#[test]
fn route_and_subprotocol_context_are_bound() {
    let challenge = golden_challenge();
    let proof = golden_proof();
    let base = proof_signing_digest(&challenge, &proof);

    for changed in [
        independent_proof_digest(&challenge, &proof, "/different/route", CONTROL_SUBPROTOCOL),
        independent_proof_digest(&challenge, &proof, CONTROL_ROUTE, "different.protocol"),
    ] {
        assert_ne!(base, changed);
        assert_signature_rejects(&proof, &changed);
    }
}

#[test]
fn strict_verifier_parses_key_and_signature_and_rejects_weak_keys() {
    let challenge = golden_challenge();

    let mut malformed_key = golden_proof();
    malformed_key.control_public_key.pop();
    assert_eq!(
        verify_proof_strict(&challenge, &malformed_key),
        Err(HandshakeError::ControlPublicKey)
    );

    let mut wrong_key = golden_proof();
    wrong_key.control_public_key = RFC8032_PUBLIC_KEY.to_vec();
    assert_eq!(
        verify_proof_strict(&challenge, &wrong_key),
        Err(HandshakeError::Signature)
    );

    let mut weak_key = golden_proof();
    weak_key.control_public_key = vec![0; 32];
    weak_key.control_public_key[0] = 1;
    assert_eq!(
        verify_proof_strict(&challenge, &weak_key),
        Err(HandshakeError::WeakControlPublicKey)
    );

    let mut malformed_signature = golden_proof();
    malformed_signature.signature.pop();
    assert_eq!(
        verify_proof_strict(&challenge, &malformed_signature),
        Err(HandshakeError::Signature)
    );
}

#[test]
fn every_challenge_field_is_bound() {
    let challenge = golden_challenge();
    let proof = golden_proof();

    let mut changed = challenge.clone();
    changed.protocol_version += 1;
    assert_digest_mutation_rejected(&proof, &changed, &proof);

    let mut changed_challenge = challenge.clone();
    let mut changed_proof = proof.clone();
    changed_challenge.challenge_id[15] ^= 1;
    changed_proof.challenge_id[15] ^= 1;
    assert_digest_mutation_rejected(&proof, &changed_challenge, &changed_proof);

    let mut changed = challenge.clone();
    changed.server_nonce[0] ^= 1;
    assert_digest_mutation_rejected(&proof, &changed, &proof);

    let mut changed = challenge.clone();
    changed.expires_at_unix_ms += 1;
    assert_digest_mutation_rejected(&proof, &changed, &proof);

    let mut changed = challenge;
    changed.max_client_init_bytes += 1;
    assert_digest_mutation_rejected(&proof, &changed, &proof);
}

#[test]
fn every_proof_field_and_canonical_init_hash_are_bound() {
    let challenge = golden_challenge();
    let proof = golden_proof();

    let mut changed = proof.clone();
    changed.challenge_id[15] ^= 1;
    assert_digest_mutation_rejected(&proof, &challenge, &changed);

    let mut changed = proof.clone();
    changed.client_nonce[0] ^= 1;
    assert_digest_mutation_rejected(&proof, &challenge, &changed);

    let mut changed = proof.clone();
    changed.control_public_key = RFC8032_PUBLIC_KEY.to_vec();
    assert_digest_mutation_rejected(&proof, &challenge, &changed);

    let mut changed = proof.clone();
    OTHER_MACHINE_HARDWARE_ID.clone_into(&mut changed.machine_hardware_id);
    assert_digest_mutation_rejected(&proof, &challenge, &changed);

    let mut changed = proof.clone();
    changed.enrollment_attempt_id[15] ^= 1;
    assert_digest_mutation_rejected(&proof, &challenge, &changed);

    let mut changed = proof.clone();
    changed.client_init_sha256[0] ^= 1;
    assert_digest_mutation_rejected(&proof, &challenge, &changed);

    let mut changed = proof.clone();
    changed.signature[0] ^= 1;
    assert_eq!(
        proof_signing_digest(&challenge, &changed),
        proof_signing_digest(&challenge, &proof)
    );
    assert_eq!(
        verify_proof_strict(&challenge, &changed),
        Err(HandshakeError::Signature)
    );
}

#[test]
fn typed_intent_and_optional_device_id_are_bound() {
    let challenge = golden_challenge();
    let mut resume = golden_proof();
    resume.intent = ProofIntent::Resume as i32;
    resume.claimed_device_id = Some(CLAIMED_DEVICE_ID.to_owned());
    resume.signature.clear();
    let resume = fixture_signed_proof(&challenge, resume);
    assert!(verify_proof_strict(&challenge, &resume).is_ok());

    let mut changed = resume.clone();
    changed.intent = ProofIntent::RotateControlKey as i32;
    assert_digest_mutation_rejected(&resume, &challenge, &changed);

    let mut changed = resume.clone();
    changed.claimed_device_id = Some(OTHER_CLAIMED_DEVICE_ID.to_owned());
    assert_digest_mutation_rejected(&resume, &challenge, &changed);
}

#[test]
fn arbitrary_typed_fields_can_be_digested_signed_and_verified() {
    let mut challenge = golden_challenge();
    challenge.protocol_version = 0;
    challenge.challenge_id = vec![0xff];
    challenge.server_nonce.clear();
    challenge.expires_at_unix_ms = i64::MIN;
    challenge.max_client_init_bytes = 0;

    let mut proof = golden_proof();
    proof.challenge_id = vec![1, 2, 3];
    proof.client_nonce = vec![4];
    proof.machine_hardware_id = "not-a-uuid".to_owned();
    proof.claimed_device_id = Some(String::new());
    proof.intent = i32::MAX;
    proof.enrollment_attempt_id.clear();
    proof.client_init_sha256 = vec![5, 6];
    proof.signature = vec![0xaa];

    let proof_digest = proof_signing_digest(&challenge, &proof);
    assert_eq!(proof_digest.len(), 32);
    assert_eq!(
        proof_digest,
        independent_proof_digest(&challenge, &proof, CONTROL_ROUTE, CONTROL_SUBPROTOCOL)
    );

    let signed = fixture_signed_proof(&challenge, proof);
    assert_eq!(proof_signing_digest(&challenge, &signed), proof_digest);
    assert_eq!(signed.signature.len(), 64);
    assert!(verify_proof_strict(&challenge, &signed).is_ok());
}

fn fixture_signed_proof(challenge: &ServerChallenge, proof: ClientProof) -> ClientProof {
    let signing_key = SigningKey::from_bytes(&[0x11; 32]);
    sign_client_proof(&signing_key, challenge, proof)
}

fn assert_digest_mutation_rejected(
    original: &ClientProof,
    challenge: &ServerChallenge,
    changed: &ClientProof,
) {
    let digest = independent_proof_digest(challenge, changed, CONTROL_ROUTE, CONTROL_SUBPROTOCOL);
    assert_eq!(proof_signing_digest(challenge, changed), digest);
    assert_signature_rejects(original, &digest);
    assert_eq!(
        verify_proof_strict(challenge, changed),
        Err(HandshakeError::Signature)
    );
}

fn assert_signature_accepts(proof: &ClientProof, proof_digest: &[u8; 32]) {
    let (key, signature) = verification_material(proof);
    assert!(key.verify_strict(proof_digest, &signature).is_ok());
}

fn assert_signature_rejects(proof: &ClientProof, proof_digest: &[u8; 32]) {
    let (key, signature) = verification_material(proof);
    assert!(key.verify_strict(proof_digest, &signature).is_err());
}

fn verification_material(proof: &ClientProof) -> (VerifyingKey, Signature) {
    let Ok(key) = VerifyingKey::try_from(proof.control_public_key.as_slice()) else {
        panic!("fixture public key must parse");
    };
    let Ok(signature) = Signature::try_from(proof.signature.as_slice()) else {
        panic!("fixture signature must parse");
    };
    (key, signature)
}
