use ed25519_dalek::{
    Signature, Signer as _, SigningKey, VerifyingKey,
    pkcs8::{DecodePrivateKey as _, EncodePrivateKey as _},
};
use sha2::{Digest as _, Sha256};
use uuid::Uuid;

pub(super) const PROTOCOL_VERSION: u32 = 1;
pub(super) const FIRST_ENROLLMENT_INTENT: u8 = 1;

const CHALLENGE_TAG: u8 = 1;
const PROOF_TAG: u8 = 2;
const CLIENT_INIT_TAG: u8 = 3;
pub(super) const ACCEPTED_TAG: u8 = 4;
const CHALLENGE_LEN: usize = 53;
const PROOF_WITHOUT_DEVICE_ID_LEN: usize = 211;
const PROOF_WITH_DEVICE_ID_LEN: usize = 227;
const CLIENT_INIT_LEN: usize = 102;
const TRANSCRIPT_DOMAIN: &[u8] = b"NATSUME-WSS-CONTROL-PROOF-v1\0";
const CONTROL_KEY_ID_DOMAIN: &[u8] = b"NATSUME-CONTROL-KEY-ID-v1\0";
const ED25519_ALGORITHM_ID: u8 = 0x01;
pub(super) const CONTROL_ROUTE: &str = "/api/v2/device/control";
pub(super) const CONTROL_SUBPROTOCOL: &str = "natsume.control";
const TRANSCRIPT_ROUTE: &[u8] = CONTROL_ROUTE.as_bytes();
const TRANSCRIPT_SUBPROTOCOL: &[u8] = CONTROL_SUBPROTOCOL.as_bytes();

// Public deterministic test material; this is not a production or deployment credential.
const CONTROL_KEY_SEED: [u8; 32] = [0x11; 32];
// RFC 8410 OneAsymmetricKey DER, including the matching public key.
const EXPECTED_PKCS8_DER: [u8; 83] = [
    0x30, 0x51, 0x02, 0x01, 0x01, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20,
    0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11,
    0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11,
    0x81, 0x21, 0x00, 0xd0, 0x4a, 0xb2, 0x32, 0x74, 0x2b, 0xb4, 0xab, 0x3a, 0x13, 0x68, 0xbd, 0x46,
    0x15, 0xe4, 0xe6, 0xd0, 0x22, 0x4a, 0xb7, 0x1a, 0x01, 0x6b, 0xaf, 0x85, 0x20, 0xa3, 0x32, 0xc9,
    0x77, 0x87, 0x37,
];
const EXPECTED_PUBLIC_KEY: [u8; 32] = [
    0xd0, 0x4a, 0xb2, 0x32, 0x74, 0x2b, 0xb4, 0xab, 0x3a, 0x13, 0x68, 0xbd, 0x46, 0x15, 0xe4, 0xe6,
    0xd0, 0x22, 0x4a, 0xb7, 0x1a, 0x01, 0x6b, 0xaf, 0x85, 0x20, 0xa3, 0x32, 0xc9, 0x77, 0x87, 0x37,
];
const EXPECTED_CONTROL_KEY_ID: [u8; 32] = [
    0x9b, 0x3b, 0x54, 0xa4, 0xf0, 0x96, 0xcd, 0xc7, 0xe2, 0xd2, 0x19, 0x89, 0x46, 0x01, 0x37, 0x8f,
    0x93, 0x53, 0x2a, 0xef, 0xbb, 0xb6, 0xb4, 0xec, 0x15, 0xae, 0x65, 0xf7, 0x10, 0x00, 0x46, 0xd8,
];
const CLIENT_NONCE: [u8; 32] = [
    0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x3b, 0x3c, 0x3d, 0x3e, 0x3f,
    0x40, 0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4a, 0x4b, 0x4c, 0x4d, 0x4e, 0x4f,
];
// RFC 4122 variant UUIDv5: 550e8400-e29b-51d4-a716-446655440000.
const MACHINE_HARDWARE_ID: [u8; 16] = [
    0x55, 0x0e, 0x84, 0x00, 0xe2, 0x9b, 0x51, 0xd4, 0xa7, 0x16, 0x44, 0x66, 0x55, 0x44, 0x00, 0x00,
];
// Canonical UUIDv7: 01900000-0000-7000-8000-000000000003.
const CLAIMED_DEVICE_ID: [u8; 16] = [
    0x01, 0x90, 0x00, 0x00, 0x00, 0x00, 0x70, 0x00, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03,
];
// Canonical UUIDv7: 01900000-0000-7000-8000-000000000002.
const ENROLLMENT_ATTEMPT_ID: [u8; 16] = [
    0x01, 0x90, 0x00, 0x00, 0x00, 0x00, 0x70, 0x00, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02,
];
// Canonical UUIDv7: 01900000-0000-7000-8000-000000000001.
const GOLDEN_CHALLENGE_ID: [u8; 16] = [
    0x01, 0x90, 0x00, 0x00, 0x00, 0x00, 0x70, 0x00, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
];
const GOLDEN_SERVER_NONCE: [u8; 32] = [
    0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f,
    0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2a, 0x2b, 0x2c, 0x2d, 0x2e, 0x2f,
];
// Private Batch 0 tagged codec vector; production Proto uses separate canonical goldens.
const TAGGED_CLIENT_INIT_SHA256: [u8; 32] = [
    0x9d, 0x40, 0xdd, 0xdc, 0xc3, 0xfc, 0x94, 0x7d, 0x54, 0x64, 0x75, 0x98, 0x91, 0x0c, 0x9c, 0x45,
    0x5a, 0x20, 0x5d, 0x0b, 0xbc, 0x3a, 0xbf, 0x61, 0xf8, 0xa3, 0x83, 0x0e, 0x8f, 0xd8, 0x4a, 0x97,
];
const TAGGED_TRANSCRIPT_SHA256: [u8; 32] = [
    0x4f, 0x28, 0x66, 0x6e, 0x39, 0xca, 0x4b, 0xf8, 0x13, 0x40, 0x0e, 0x0a, 0x18, 0x50, 0x04, 0x1a,
    0x20, 0x82, 0xde, 0xb1, 0xeb, 0x6b, 0x31, 0x63, 0x5f, 0xc3, 0xc3, 0x0c, 0x7f, 0x33, 0xb1, 0xa1,
];
const TAGGED_SIGNATURE: [u8; 64] = [
    0xee, 0x36, 0x89, 0x71, 0xe8, 0x28, 0xbe, 0x59, 0xd7, 0x38, 0xef, 0xdc, 0xba, 0x9e, 0x50, 0x8a,
    0xb8, 0xed, 0x47, 0x12, 0xea, 0xe0, 0xe0, 0x62, 0x17, 0xa1, 0x62, 0x39, 0x1c, 0x93, 0xa6, 0xc9,
    0x81, 0xf0, 0x95, 0xf1, 0x48, 0x82, 0x44, 0xf5, 0x34, 0x72, 0xf0, 0xbe, 0x1e, 0xa8, 0x60, 0x27,
    0x16, 0xac, 0x6c, 0x30, 0x59, 0xe7, 0x90, 0xe7, 0xad, 0x9c, 0xd4, 0x54, 0xb1, 0xda, 0x51, 0x07,
];

#[derive(Clone, Copy)]
pub(super) struct Challenge {
    pub(super) protocol_version: u32,
    pub(super) id: [u8; 16],
    pub(super) server_nonce: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Proof {
    pub(super) challenge_id: [u8; 16],
    pub(super) client_nonce: [u8; 32],
    pub(super) control_public_key: [u8; 32],
    pub(super) machine_hardware_id: [u8; 16],
    pub(super) intent: u8,
    pub(super) claimed_device_id: Option<[u8; 16]>,
    pub(super) enrollment_attempt_id: [u8; 16],
    pub(super) client_init_sha256: [u8; 32],
    pub(super) signature: [u8; 64],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ProofError {
    ProtocolVersionMismatch,
    ChallengeMismatch,
    IntentMismatch,
    UnexpectedDeviceId,
    PublicKeyInvalid,
    SignatureInvalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FrameError {
    Length,
    Tag,
    Presence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum InitError {
    HashMismatch,
    Frame(FrameError),
    FieldMismatch,
}

pub(super) struct ClientInit {
    pub(super) protocol_version: u32,
    pub(super) intent: u8,
    pub(super) machine_hardware_id: [u8; 16],
    pub(super) control_public_key: [u8; 32],
    pub(super) client_nonce: [u8; 32],
    pub(super) enrollment_attempt_id: [u8; 16],
}

pub(super) fn random_challenge() -> Challenge {
    let challenge_id = Uuid::now_v7().into_bytes();
    let mut server_nonce = [0_u8; 32];
    require_ok(
        getrandom::fill(&mut server_nonce),
        "server nonce entropy unavailable",
    );
    Challenge {
        protocol_version: PROTOCOL_VERSION,
        id: challenge_id,
        server_nonce,
    }
}

pub(super) fn deterministic_signing_key() -> SigningKey {
    let source = SigningKey::from_bytes(&CONTROL_KEY_SEED);
    let document = require_ok(source.to_pkcs8_der(), "control key PKCS#8 encoding failed");
    assert_eq!(document.as_bytes(), EXPECTED_PKCS8_DER);
    let decoded = require_ok(
        SigningKey::from_pkcs8_der(&EXPECTED_PKCS8_DER),
        "control key PKCS#8 decoding failed",
    );
    assert_eq!(decoded.to_bytes(), CONTROL_KEY_SEED);
    decoded
}

pub(super) fn client_init(control_public_key: [u8; 32]) -> ClientInit {
    ClientInit {
        protocol_version: PROTOCOL_VERSION,
        intent: FIRST_ENROLLMENT_INTENT,
        machine_hardware_id: MACHINE_HARDWARE_ID,
        control_public_key,
        client_nonce: CLIENT_NONCE,
        enrollment_attempt_id: ENROLLMENT_ATTEMPT_ID,
    }
}

pub(super) fn sign_proof(
    signing_key: &SigningKey,
    challenge: Challenge,
    client_init: &ClientInit,
    client_init_sha256: [u8; 32],
) -> Proof {
    let mut proof = Proof {
        challenge_id: challenge.id,
        client_nonce: client_init.client_nonce,
        control_public_key: client_init.control_public_key,
        machine_hardware_id: client_init.machine_hardware_id,
        intent: client_init.intent,
        claimed_device_id: None,
        enrollment_attempt_id: client_init.enrollment_attempt_id,
        client_init_sha256,
        signature: [0_u8; 64],
    };
    proof.signature = signing_key
        .sign(&proof_transcript(challenge, &proof))
        .to_bytes();
    proof
}

pub(super) fn verify_proof(challenge: Challenge, proof: &Proof) -> Result<(), ProofError> {
    if challenge.protocol_version != PROTOCOL_VERSION {
        return Err(ProofError::ProtocolVersionMismatch);
    }
    if proof.challenge_id != challenge.id {
        return Err(ProofError::ChallengeMismatch);
    }
    if proof.intent != FIRST_ENROLLMENT_INTENT {
        return Err(ProofError::IntentMismatch);
    }
    if proof.claimed_device_id.is_some() {
        return Err(ProofError::UnexpectedDeviceId);
    }
    let key = VerifyingKey::from_bytes(&proof.control_public_key)
        .map_err(|_| ProofError::PublicKeyInvalid)?;
    let signature = Signature::from_bytes(&proof.signature);
    key.verify_strict(&proof_transcript(challenge, proof), &signature)
        .map_err(|_| ProofError::SignatureInvalid)
}

pub(super) fn assert_deterministic_vectors() {
    let challenge = Challenge {
        protocol_version: PROTOCOL_VERSION,
        id: GOLDEN_CHALLENGE_ID,
        server_nonce: GOLDEN_SERVER_NONCE,
    };
    let key = deterministic_signing_key();
    assert_eq!(key.verifying_key().to_bytes(), EXPECTED_PUBLIC_KEY);
    assert_eq!(control_key_id(EXPECTED_PUBLIC_KEY), EXPECTED_CONTROL_KEY_ID);
    let init = client_init(EXPECTED_PUBLIC_KEY);
    let init_bytes = encode_client_init(&init);
    assert_eq!(init_bytes.len(), CLIENT_INIT_LEN);
    assert_eq!(sha256(&init_bytes), TAGGED_CLIENT_INIT_SHA256);
    let proof = sign_proof(&key, challenge, &init, TAGGED_CLIENT_INIT_SHA256);
    let transcript = proof_transcript(challenge, &proof);
    assert_eq!(transcript.len(), 252);
    assert_eq!(sha256(&transcript), TAGGED_TRANSCRIPT_SHA256);
    assert_eq!(proof.signature, TAGGED_SIGNATURE);
    assert_eq!(verify_proof(challenge, &proof), Ok(()));
}

fn control_key_id(public_key: [u8; 32]) -> [u8; 32] {
    let mut input = Vec::with_capacity(CONTROL_KEY_ID_DOMAIN.len() + 1 + 2 + public_key.len());
    input.extend_from_slice(CONTROL_KEY_ID_DOMAIN);
    input.push(ED25519_ALGORITHM_ID);
    input.extend_from_slice(&u16_len(&public_key).to_be_bytes());
    input.extend_from_slice(&public_key);
    sha256(&input)
}

pub(super) fn encode_challenge(challenge: Challenge) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(CHALLENGE_LEN);
    bytes.push(CHALLENGE_TAG);
    bytes.extend_from_slice(&challenge.protocol_version.to_be_bytes());
    bytes.extend_from_slice(&challenge.id);
    bytes.extend_from_slice(&challenge.server_nonce);
    bytes
}

pub(super) fn decode_challenge(bytes: &[u8]) -> Result<Challenge, FrameError> {
    require_frame(bytes, CHALLENGE_LEN, CHALLENGE_TAG)?;
    Ok(Challenge {
        protocol_version: u32::from_be_bytes(fixed(bytes, 1)),
        id: fixed(bytes, 5),
        server_nonce: fixed(bytes, 21),
    })
}

pub(super) fn encode_proof(proof: &Proof) -> Vec<u8> {
    let capacity = if proof.claimed_device_id.is_some() {
        PROOF_WITH_DEVICE_ID_LEN
    } else {
        PROOF_WITHOUT_DEVICE_ID_LEN
    };
    let mut bytes = Vec::with_capacity(capacity);
    bytes.push(PROOF_TAG);
    bytes.extend_from_slice(&proof.challenge_id);
    bytes.extend_from_slice(&proof.client_nonce);
    bytes.extend_from_slice(&proof.control_public_key);
    bytes.extend_from_slice(&proof.machine_hardware_id);
    bytes.push(proof.intent);
    bytes.push(u8::from(proof.claimed_device_id.is_some()));
    if let Some(device_id) = proof.claimed_device_id {
        bytes.extend_from_slice(&device_id);
    }
    bytes.extend_from_slice(&proof.enrollment_attempt_id);
    bytes.extend_from_slice(&proof.client_init_sha256);
    bytes.extend_from_slice(&proof.signature);
    bytes
}

pub(super) fn decode_proof(bytes: &[u8]) -> Result<Proof, FrameError> {
    if bytes.len() < 99 {
        return Err(FrameError::Length);
    }
    if bytes[0] != PROOF_TAG {
        return Err(FrameError::Tag);
    }
    let (claimed_device_id, tail_offset) = match bytes[98] {
        0 if bytes.len() == PROOF_WITHOUT_DEVICE_ID_LEN => (None, 99),
        1 if bytes.len() == PROOF_WITH_DEVICE_ID_LEN => (Some(fixed(bytes, 99)), 115),
        0 | 1 => return Err(FrameError::Length),
        _ => return Err(FrameError::Presence),
    };
    Ok(Proof {
        challenge_id: fixed(bytes, 1),
        client_nonce: fixed(bytes, 17),
        control_public_key: fixed(bytes, 49),
        machine_hardware_id: fixed(bytes, 81),
        intent: bytes[97],
        claimed_device_id,
        enrollment_attempt_id: fixed(bytes, tail_offset),
        client_init_sha256: fixed(bytes, tail_offset + 16),
        signature: fixed(bytes, tail_offset + 48),
    })
}

pub(super) fn encode_client_init(client_init: &ClientInit) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(CLIENT_INIT_LEN);
    bytes.push(CLIENT_INIT_TAG);
    bytes.extend_from_slice(&client_init.protocol_version.to_be_bytes());
    bytes.push(client_init.intent);
    bytes.extend_from_slice(&client_init.machine_hardware_id);
    bytes.extend_from_slice(&client_init.control_public_key);
    bytes.extend_from_slice(&client_init.client_nonce);
    bytes.extend_from_slice(&client_init.enrollment_attempt_id);
    bytes
}

pub(super) fn decode_client_init(bytes: &[u8]) -> Result<ClientInit, FrameError> {
    require_frame(bytes, CLIENT_INIT_LEN, CLIENT_INIT_TAG)?;
    Ok(ClientInit {
        protocol_version: u32::from_be_bytes(fixed(bytes, 1)),
        intent: bytes[5],
        machine_hardware_id: fixed(bytes, 6),
        control_public_key: fixed(bytes, 22),
        client_nonce: fixed(bytes, 54),
        enrollment_attempt_id: fixed(bytes, 86),
    })
}

pub(super) fn verify_client_init(proof: &Proof, bytes: &[u8]) -> Result<ClientInit, InitError> {
    if sha256(bytes) != proof.client_init_sha256 {
        return Err(InitError::HashMismatch);
    }
    let init = decode_client_init(bytes).map_err(InitError::Frame)?;
    if init.protocol_version != PROTOCOL_VERSION
        || init.intent != proof.intent
        || init.machine_hardware_id != proof.machine_hardware_id
        || init.control_public_key != proof.control_public_key
        || init.client_nonce != proof.client_nonce
        || init.enrollment_attempt_id != proof.enrollment_attempt_id
    {
        return Err(InitError::FieldMismatch);
    }
    Ok(init)
}

pub(super) fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn proof_transcript(challenge: Challenge, proof: &Proof) -> Vec<u8> {
    proof_transcript_with_context(challenge, proof, TRANSCRIPT_ROUTE, TRANSCRIPT_SUBPROTOCOL)
}

fn proof_transcript_with_context(
    challenge: Challenge,
    proof: &Proof,
    route: &[u8],
    subprotocol: &[u8],
) -> Vec<u8> {
    let mut transcript = Vec::with_capacity(256);
    transcript.extend_from_slice(TRANSCRIPT_DOMAIN);
    transcript.extend_from_slice(&u16_len(route).to_be_bytes());
    transcript.extend_from_slice(route);
    transcript.extend_from_slice(&u16_len(subprotocol).to_be_bytes());
    transcript.extend_from_slice(subprotocol);
    transcript.extend_from_slice(&challenge.id);
    transcript.extend_from_slice(&challenge.server_nonce);
    transcript.extend_from_slice(&proof.client_nonce);
    transcript.extend_from_slice(&challenge.protocol_version.to_be_bytes());
    transcript.push(proof.intent);
    transcript.extend_from_slice(&proof.control_public_key);
    transcript.extend_from_slice(&proof.machine_hardware_id);
    transcript.push(u8::from(proof.claimed_device_id.is_some()));
    if let Some(device_id) = proof.claimed_device_id {
        transcript.extend_from_slice(&device_id);
    }
    transcript.extend_from_slice(&proof.enrollment_attempt_id);
    transcript.extend_from_slice(&proof.client_init_sha256);
    transcript
}

fn u16_len(bytes: &[u8]) -> u16 {
    match u16::try_from(bytes.len()) {
        Ok(length) => length,
        Err(error) => panic!("test transcript field is too long: {error}"),
    }
}

fn require_frame(bytes: &[u8], expected_len: usize, tag: u8) -> Result<(), FrameError> {
    if bytes.len() != expected_len {
        return Err(FrameError::Length);
    }
    if bytes[0] != tag {
        return Err(FrameError::Tag);
    }
    Ok(())
}

fn fixed<const N: usize>(bytes: &[u8], offset: usize) -> [u8; N] {
    let Some(value) = bytes.get(offset..offset + N) else {
        panic!("fixed test frame field must be present");
    };
    let mut output = [0_u8; N];
    output.copy_from_slice(value);
    output
}

fn require_ok<T, E>(result: Result<T, E>, message: &str) -> T {
    match result {
        Ok(value) => value,
        Err(error) => {
            drop(error);
            panic!("{message}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RFC8032_SEED: [u8; 32] = [
        0x9d, 0x61, 0xb1, 0x9d, 0xef, 0xfd, 0x5a, 0x60, 0xba, 0x84, 0x4a, 0xf4, 0x92, 0xec, 0x2c,
        0xc4, 0x44, 0x49, 0xc5, 0x69, 0x7b, 0x32, 0x69, 0x19, 0x70, 0x3b, 0xac, 0x03, 0x1c, 0xae,
        0x7f, 0x60,
    ];
    const RFC8032_PUBLIC_KEY: [u8; 32] = [
        0xd7, 0x5a, 0x98, 0x01, 0x82, 0xb1, 0x0a, 0xb7, 0xd5, 0x4b, 0xfe, 0xd3, 0xc9, 0x64, 0x07,
        0x3a, 0x0e, 0xe1, 0x72, 0xf3, 0xda, 0xa6, 0x23, 0x25, 0xaf, 0x02, 0x1a, 0x68, 0xf7, 0x07,
        0x51, 0x1a,
    ];
    const RFC8032_SIGNATURE: [u8; 64] = [
        0xe5, 0x56, 0x43, 0x00, 0xc3, 0x60, 0xac, 0x72, 0x90, 0x86, 0xe2, 0xcc, 0x80, 0x6e, 0x82,
        0x8a, 0x84, 0x87, 0x7f, 0x1e, 0xb8, 0xe5, 0xd9, 0x74, 0xd8, 0x73, 0xe0, 0x65, 0x22, 0x49,
        0x01, 0x55, 0x5f, 0xb8, 0x82, 0x15, 0x90, 0xa3, 0x3b, 0xac, 0xc6, 0x1e, 0x39, 0x70, 0x1c,
        0xf9, 0xb4, 0x6b, 0xd2, 0x5b, 0xf5, 0xf0, 0x59, 0x5b, 0xbe, 0x24, 0x65, 0x51, 0x41, 0x43,
        0x8e, 0x7a, 0x10, 0x0b,
    ];

    #[test]
    fn rfc8032_empty_message_vector_matches() {
        let key = SigningKey::from_bytes(&RFC8032_SEED);
        assert_eq!(key.verifying_key().to_bytes(), RFC8032_PUBLIC_KEY);
        assert_eq!(key.sign(&[]).to_bytes(), RFC8032_SIGNATURE);
        assert!(
            key.verifying_key()
                .verify_strict(&[], &Signature::from_bytes(&RFC8032_SIGNATURE))
                .is_ok()
        );
    }

    #[test]
    fn live_proof_rejects_a_weak_identity_key_signature() {
        let (challenge, _init, _init_bytes, mut proof) = golden_exchange();
        proof.control_public_key = [0_u8; 32];
        proof.control_public_key[0] = 1;
        proof.signature = [0_u8; 64];
        proof.signature[0] = 1;

        let key = require_ok(
            VerifyingKey::from_bytes(&proof.control_public_key),
            "weak identity public key must parse",
        );
        assert!(key.is_weak());
        assert_eq!(
            verify_proof(challenge, &proof),
            Err(ProofError::SignatureInvalid)
        );
    }

    #[test]
    fn proof_rejects_wrong_key_replay_and_first_enrollment_signed_field_mutations() {
        let (challenge, init, init_bytes, proof) = golden_exchange();
        assert_eq!(verify_proof(challenge, &proof), Ok(()));
        assert_challenge_mutations(challenge, &proof);
        assert_context_mutations(challenge, &proof);
        assert_proof_mutations(challenge, &proof);
        assert_client_init_mutations(&init, &init_bytes, &proof);
    }

    fn golden_exchange() -> (Challenge, ClientInit, Vec<u8>, Proof) {
        let challenge = Challenge {
            protocol_version: PROTOCOL_VERSION,
            id: GOLDEN_CHALLENGE_ID,
            server_nonce: GOLDEN_SERVER_NONCE,
        };
        let key = deterministic_signing_key();
        let init = client_init(key.verifying_key().to_bytes());
        let init_bytes = encode_client_init(&init);
        let proof = sign_proof(&key, challenge, &init, sha256(&init_bytes));
        (challenge, init, init_bytes, proof)
    }

    fn assert_challenge_mutations(challenge: Challenge, proof: &Proof) {
        let mut changed = challenge;
        changed.id[15] ^= 1;
        assert_original_signature_rejects(changed, proof, proof);
        assert_eq!(
            verify_proof(changed, proof),
            Err(ProofError::ChallengeMismatch)
        );

        let mut changed = challenge;
        changed.server_nonce[0] ^= 1;
        assert_eq!(
            verify_proof(changed, proof),
            Err(ProofError::SignatureInvalid)
        );

        let mut changed = challenge;
        changed.protocol_version += 1;
        assert_original_signature_rejects(changed, proof, proof);
        assert_eq!(
            verify_proof(changed, proof),
            Err(ProofError::ProtocolVersionMismatch)
        );
    }

    fn assert_context_mutations(challenge: Challenge, proof: &Proof) {
        let Ok(verifying_key) = VerifyingKey::from_bytes(&proof.control_public_key) else {
            panic!("golden public key must parse");
        };
        let signature = Signature::from_bytes(&proof.signature);
        for (route, subprotocol) in [
            (b"/different/route".as_slice(), TRANSCRIPT_SUBPROTOCOL),
            (TRANSCRIPT_ROUTE, b"different.protocol".as_slice()),
        ] {
            assert!(
                verifying_key
                    .verify_strict(
                        &proof_transcript_with_context(challenge, proof, route, subprotocol),
                        &signature,
                    )
                    .is_err()
            );
        }
    }

    fn assert_original_signature_rejects(challenge: Challenge, original: &Proof, changed: &Proof) {
        let key = require_ok(
            VerifyingKey::from_bytes(&original.control_public_key),
            "golden public key must parse",
        );
        assert!(
            key.verify_strict(
                &proof_transcript(challenge, changed),
                &Signature::from_bytes(&original.signature),
            )
            .is_err()
        );
    }

    fn assert_proof_mutations(challenge: Challenge, proof: &Proof) {
        for mutate in [
            mutate_client_nonce as fn(&mut Proof),
            mutate_public_key,
            mutate_hardware_id,
            mutate_attempt_id,
            mutate_init_hash,
            mutate_signature,
        ] {
            let mut changed = proof.clone();
            mutate(&mut changed);
            assert!(verify_proof(challenge, &changed).is_err());
        }

        let mut changed = proof.clone();
        changed.intent = 2;
        assert_original_signature_rejects(challenge, proof, &changed);
        assert_eq!(
            verify_proof(challenge, &changed),
            Err(ProofError::IntentMismatch)
        );

        let mut changed = proof.clone();
        changed.claimed_device_id = Some(CLAIMED_DEVICE_ID);
        assert_original_signature_rejects(challenge, proof, &changed);
        assert_eq!(
            verify_proof(challenge, &changed),
            Err(ProofError::UnexpectedDeviceId)
        );
        assert_eq!(decode_proof(&encode_proof(&changed)), Ok(changed.clone()));

        let wrong_key = SigningKey::from_bytes(&[0x22; 32]);
        let mut changed = proof.clone();
        changed.signature = wrong_key
            .sign(&proof_transcript(challenge, proof))
            .to_bytes();
        assert_eq!(
            verify_proof(challenge, &changed),
            Err(ProofError::SignatureInvalid)
        );
    }

    fn assert_client_init_mutations(init: &ClientInit, init_bytes: &[u8], proof: &Proof) {
        let mut changed = init_bytes.to_vec();
        changed[6] ^= 1;
        assert!(matches!(
            verify_client_init(proof, &changed),
            Err(InitError::HashMismatch)
        ));

        let mut mismatched = encode_client_init(init);
        mismatched[5] = 2;
        let mut matching_hash_proof = proof.clone();
        matching_hash_proof.client_init_sha256 = sha256(&mismatched);
        assert!(matches!(
            verify_client_init(&matching_hash_proof, &mismatched),
            Err(InitError::FieldMismatch)
        ));
    }

    #[test]
    fn malformed_fixed_frames_are_rejected() {
        let challenge = Challenge {
            protocol_version: PROTOCOL_VERSION,
            id: GOLDEN_CHALLENGE_ID,
            server_nonce: GOLDEN_SERVER_NONCE,
        };
        let key = deterministic_signing_key();
        let init = client_init(key.verifying_key().to_bytes());
        let init_bytes = encode_client_init(&init);
        let proof = sign_proof(&key, challenge, &init, sha256(&init_bytes));

        assert_frame_rejections(encode_challenge(challenge), |bytes| {
            decode_challenge(bytes).map(|_| ())
        });
        let encoded_proof = encode_proof(&proof);
        assert_frame_rejections(encoded_proof.clone(), |bytes| {
            decode_proof(bytes).map(|_| ())
        });
        let mut invalid_presence = encoded_proof;
        invalid_presence[98] = 2;
        assert_eq!(decode_proof(&invalid_presence), Err(FrameError::Presence));
        assert_frame_rejections(init_bytes, |bytes| decode_client_init(bytes).map(|_| ()));
    }

    #[test]
    fn pkcs8_golden_rejects_malformed_encodings() {
        assert!(SigningKey::from_pkcs8_der(&EXPECTED_PKCS8_DER[..82]).is_err());

        let mut wrong_algorithm = EXPECTED_PKCS8_DER;
        wrong_algorithm[11] ^= 1;
        assert!(SigningKey::from_pkcs8_der(&wrong_algorithm).is_err());

        let mut malformed_private_key_tag = EXPECTED_PKCS8_DER;
        malformed_private_key_tag[12] ^= 1;
        assert!(SigningKey::from_pkcs8_der(&malformed_private_key_tag).is_err());

        let mut mismatched_public_key = EXPECTED_PKCS8_DER;
        mismatched_public_key[EXPECTED_PKCS8_DER.len() - EXPECTED_PUBLIC_KEY.len()] ^= 1;
        assert!(SigningKey::from_pkcs8_der(&mismatched_public_key).is_err());

        let mut trailing = EXPECTED_PKCS8_DER.to_vec();
        trailing.push(0);
        assert!(SigningKey::from_pkcs8_der(&trailing).is_err());
    }

    fn assert_frame_rejections<F>(bytes: Vec<u8>, decode: F)
    where
        F: Fn(&[u8]) -> Result<(), FrameError>,
    {
        let mut truncated = bytes.clone();
        truncated.pop();
        assert_eq!(decode(&truncated), Err(FrameError::Length));

        let mut extended = bytes.clone();
        extended.push(0);
        assert_eq!(decode(&extended), Err(FrameError::Length));

        let mut wrong_tag = bytes;
        wrong_tag[0] ^= 0xff;
        assert_eq!(decode(&wrong_tag), Err(FrameError::Tag));
    }

    fn mutate_client_nonce(proof: &mut Proof) {
        proof.client_nonce[0] ^= 1;
    }

    fn mutate_public_key(proof: &mut Proof) {
        proof.control_public_key[0] ^= 1;
    }

    fn mutate_hardware_id(proof: &mut Proof) {
        proof.machine_hardware_id[0] ^= 1;
    }

    fn mutate_attempt_id(proof: &mut Proof) {
        proof.enrollment_attempt_id[0] ^= 1;
    }

    fn mutate_init_hash(proof: &mut Proof) {
        proof.client_init_sha256[0] ^= 1;
    }

    fn mutate_signature(proof: &mut Proof) {
        proof.signature[0] ^= 1;
    }
}
