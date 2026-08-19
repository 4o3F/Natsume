use ed25519_dalek::{Signature, Signer as _, SigningKey};
use natsume_device_protocol::{
    CONTROL_ROUTE, CONTROL_SUBPROTOCOL, ControlKeyId, HandshakeError, canonical_client_init_sha256,
    encode_client_init_canonical,
    generated::{ClientProof, ProofIntent, ServerChallenge},
    proof_transcript, verify_proof_strict,
};
use sha2::{Digest as _, Sha256};

use super::fixture::{
    CLAIMED_DEVICE_ID, CONTROL_KEY_SEED_PUBLIC_KEY, EXPECTED_CONTROL_KEY_ID, MACHINE_HARDWARE_ID,
    OTHER_MACHINE_HARDWARE_ID, PROTO_CLIENT_INIT_SHA256, PROTO_EXPECTED_SIGNATURE,
    PROTO_TRANSCRIPT_SHA256, RFC8032_PUBLIC_KEY, canonical_client_init, golden_challenge,
    golden_proof, independent_transcript,
};

#[test]
fn canonical_client_init_hash_transcript_and_signature_goldens_match() {
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

    let Ok(transcript) = proof_transcript(&challenge, &proof) else {
        panic!("golden proof transcript must be valid");
    };
    assert_eq!(
        transcript,
        independent_transcript(&challenge, &proof, CONTROL_ROUTE, CONTROL_SUBPROTOCOL)
    );
    assert_eq!(transcript.len(), 252);
    let digest: [u8; 32] = Sha256::digest(&transcript).into();
    assert_eq!(digest, PROTO_TRANSCRIPT_SHA256);

    let signing_key = SigningKey::from_bytes(&[0x11; 32]);
    let strict_signature = signing_key.sign(&transcript).to_bytes();
    assert_eq!(strict_signature, PROTO_EXPECTED_SIGNATURE);
    let key = signing_key.verifying_key();
    let signature = Signature::from_bytes(&PROTO_EXPECTED_SIGNATURE);
    assert!(key.verify_strict(&transcript, &signature).is_ok());
    if let Err(error) = verify_proof_strict(&challenge, &proof) {
        panic!("golden proof must verify strictly: {error:?}");
    }
}

#[test]
fn route_subprotocol_and_optional_device_slot_are_bound() {
    let challenge = golden_challenge();
    let proof = golden_proof();
    let Ok(base) = proof_transcript(&challenge, &proof) else {
        panic!("golden proof transcript must be valid");
    };

    let wrong_route =
        independent_transcript(&challenge, &proof, "/different/route", CONTROL_SUBPROTOCOL);
    let wrong_subprotocol =
        independent_transcript(&challenge, &proof, CONTROL_ROUTE, "different.protocol");
    assert_ne!(base, wrong_route);
    assert_ne!(base, wrong_subprotocol);

    let mut with_device_id = proof;
    with_device_id.claimed_device_id = Some(CLAIMED_DEVICE_ID.to_owned());
    let Ok(with_device_id_transcript) = proof_transcript(&challenge, &with_device_id) else {
        panic!("canonical optional Device ID must be accepted");
    };
    assert_eq!(with_device_id_transcript.len(), base.len() + 16);
    assert_eq!(
        with_device_id_transcript,
        independent_transcript(
            &challenge,
            &with_device_id,
            CONTROL_ROUTE,
            CONTROL_SUBPROTOCOL
        )
    );
    assert!(verify_proof_strict(&challenge, &with_device_id).is_err());
}

#[test]
fn strict_verifier_rejects_wrong_and_weak_keys() {
    let challenge = golden_challenge();

    let mut wrong_key = golden_proof();
    wrong_key.control_public_key = RFC8032_PUBLIC_KEY.to_vec();
    assert!(verify_proof_strict(&challenge, &wrong_key).is_err());

    let mut weak_key = golden_proof();
    weak_key.control_public_key = vec![0; 32];
    weak_key.control_public_key[0] = 1;
    weak_key.signature = vec![0; 64];
    weak_key.signature[0] = 1;
    assert!(verify_proof_strict(&challenge, &weak_key).is_err());
}

#[test]
fn strict_verifier_rejects_every_signed_field_mutation() {
    assert_challenge_field_mutations();
    assert_proof_field_mutations();

    let challenge = golden_challenge();
    let proof = golden_proof();
    let mut changed_version = challenge.clone();
    changed_version.protocol_version += 1;
    assert_ne!(
        independent_transcript(&changed_version, &proof, CONTROL_ROUTE, CONTROL_SUBPROTOCOL),
        independent_transcript(&challenge, &proof, CONTROL_ROUTE, CONTROL_SUBPROTOCOL)
    );
    assert_rejected(&changed_version, &proof);
}

#[test]
fn transcript_validation_is_closed_and_redacted() {
    let challenge = golden_challenge();
    let proof = golden_proof();

    let mut changed = proof.clone();
    changed.client_nonce.pop();
    assert!(proof_transcript(&challenge, &changed).is_err());

    let mut changed = proof.clone();
    changed.control_public_key.pop();
    assert!(proof_transcript(&challenge, &changed).is_err());

    let mut changed = proof.clone();
    changed.enrollment_attempt_id.pop();
    assert!(proof_transcript(&challenge, &changed).is_err());

    let mut changed = proof.clone();
    changed.client_init_sha256.pop();
    assert!(proof_transcript(&challenge, &changed).is_err());

    let mut changed = proof.clone();
    changed.machine_hardware_id = MACHINE_HARDWARE_ID.to_uppercase();
    let Err(error) = proof_transcript(&challenge, &changed) else {
        panic!("noncanonical Machine Hardware ID must fail");
    };
    assert_eq!(error, HandshakeError::MachineHardwareId);
    assert_eq!(error.to_string(), "Machine Hardware ID is invalid");

    let mut changed = proof.clone();
    changed.claimed_device_id = Some(MACHINE_HARDWARE_ID.to_owned());
    assert!(proof_transcript(&challenge, &changed).is_err());

    let mut changed = proof.clone();
    changed.intent = ProofIntent::Unspecified as i32;
    assert!(proof_transcript(&challenge, &changed).is_err());

    let mut changed = proof;
    changed.signature.pop();
    assert!(verify_proof_strict(&challenge, &changed).is_err());
}

fn assert_challenge_field_mutations() {
    let challenge = golden_challenge();
    let proof = golden_proof();

    let mut changed_challenge = challenge.clone();
    let mut changed_proof = proof.clone();
    changed_challenge.challenge_id[15] ^= 1;
    changed_proof.challenge_id[15] ^= 1;
    assert_rejected(&changed_challenge, &changed_proof);

    let mut changed = challenge;
    changed.server_nonce[0] ^= 1;
    assert_rejected(&changed, &proof);
}

fn assert_proof_field_mutations() {
    let challenge = golden_challenge();
    let proof = golden_proof();

    let mut changed = proof.clone();
    changed.client_nonce[0] ^= 1;
    assert_rejected(&challenge, &changed);

    let mut changed = proof.clone();
    changed.control_public_key = RFC8032_PUBLIC_KEY.to_vec();
    assert_rejected(&challenge, &changed);

    let mut changed = proof.clone();
    OTHER_MACHINE_HARDWARE_ID.clone_into(&mut changed.machine_hardware_id);
    assert_rejected(&challenge, &changed);

    let mut changed = proof.clone();
    changed.intent = ProofIntent::Resume as i32;
    assert_rejected(&challenge, &changed);

    let mut changed = proof.clone();
    changed.claimed_device_id = Some(CLAIMED_DEVICE_ID.to_owned());
    assert_rejected(&challenge, &changed);

    let mut changed = proof.clone();
    changed.enrollment_attempt_id[15] ^= 1;
    assert_rejected(&challenge, &changed);

    let mut changed = proof.clone();
    changed.client_init_sha256[0] ^= 1;
    assert_rejected(&challenge, &changed);

    let mut changed = proof;
    changed.signature[0] ^= 1;
    assert_rejected(&challenge, &changed);
}

fn assert_rejected(challenge: &ServerChallenge, proof: &ClientProof) {
    assert!(verify_proof_strict(challenge, proof).is_err());
}
