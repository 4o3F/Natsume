use natsume_device_protocol::{
    CONTROL_WIRE_VERSION,
    generated::{
        ClientInit, ClientProof, CollectionCompleteness, EvidenceQuality, HardwareCandidate,
        HardwareClaim, ProofIntent, ServerChallenge,
    },
};
use uuid::Uuid;

pub(super) const MACHINE_HARDWARE_ID: &str = "550e8400-e29b-51d4-a716-446655440000";
pub(super) const OTHER_MACHINE_HARDWARE_ID: &str = "550e8400-e29b-51d4-a716-446655440001";
const SECOND_HARDWARE_CANDIDATE_ID: &str = "550e8400-e29b-51d4-a716-446655440002";
pub(super) const CLAIMED_DEVICE_ID: &str = "01900000-0000-7000-8000-000000000003";
pub(super) const CONTROL_KEY_SEED_PUBLIC_KEY: [u8; 32] = [
    0xd0, 0x4a, 0xb2, 0x32, 0x74, 0x2b, 0xb4, 0xab, 0x3a, 0x13, 0x68, 0xbd, 0x46, 0x15, 0xe4, 0xe6,
    0xd0, 0x22, 0x4a, 0xb7, 0x1a, 0x01, 0x6b, 0xaf, 0x85, 0x20, 0xa3, 0x32, 0xc9, 0x77, 0x87, 0x37,
];
pub(super) const RFC8032_PUBLIC_KEY: [u8; 32] = [
    0xd7, 0x5a, 0x98, 0x01, 0x82, 0xb1, 0x0a, 0xb7, 0xd5, 0x4b, 0xfe, 0xd3, 0xc9, 0x64, 0x07, 0x3a,
    0x0e, 0xe1, 0x72, 0xf3, 0xda, 0xa6, 0x23, 0x25, 0xaf, 0x02, 0x1a, 0x68, 0xf7, 0x07, 0x51, 0x1a,
];
pub(super) const EXPECTED_CONTROL_KEY_ID: [u8; 32] = [
    0x9b, 0x3b, 0x54, 0xa4, 0xf0, 0x96, 0xcd, 0xc7, 0xe2, 0xd2, 0x19, 0x89, 0x46, 0x01, 0x37, 0x8f,
    0x93, 0x53, 0x2a, 0xef, 0xbb, 0xb6, 0xb4, 0xec, 0x15, 0xae, 0x65, 0xf7, 0x10, 0x00, 0x46, 0xd8,
];
pub(super) const CLIENT_NONCE: [u8; 32] = [
    0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x3b, 0x3c, 0x3d, 0x3e, 0x3f,
    0x40, 0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4a, 0x4b, 0x4c, 0x4d, 0x4e, 0x4f,
];
pub(super) const GOLDEN_CHALLENGE_ID: [u8; 16] = [
    0x01, 0x90, 0x00, 0x00, 0x00, 0x00, 0x70, 0x00, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
];
pub(super) const GOLDEN_SERVER_NONCE: [u8; 32] = [
    0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f,
    0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2a, 0x2b, 0x2c, 0x2d, 0x2e, 0x2f,
];
pub(super) const ENROLLMENT_ATTEMPT_ID: [u8; 16] = [
    0x01, 0x90, 0x00, 0x00, 0x00, 0x00, 0x70, 0x00, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02,
];
pub(super) const PROTO_CLIENT_INIT_SHA256: [u8; 32] = [
    0x63, 0xf0, 0x8f, 0x13, 0x45, 0xad, 0x2b, 0x84, 0x21, 0x99, 0xcd, 0x1f, 0x3c, 0x42, 0x0c, 0x7c,
    0xee, 0xcc, 0xff, 0xc7, 0xdf, 0xa3, 0xc2, 0x9a, 0x52, 0x58, 0x29, 0x21, 0x12, 0xc4, 0xf8, 0x90,
];
pub(super) const PROTO_TRANSCRIPT_SHA256: [u8; 32] = [
    0xb6, 0x5b, 0xd5, 0x0d, 0xe2, 0xb5, 0x33, 0xff, 0x74, 0x4b, 0x2b, 0x9b, 0x32, 0xde, 0xe1, 0x92,
    0xd1, 0xac, 0x06, 0xdf, 0xc7, 0x75, 0x8c, 0x6f, 0xf6, 0x30, 0x89, 0x40, 0x59, 0xc3, 0xc0, 0xf0,
];
pub(super) const PROTO_EXPECTED_SIGNATURE: [u8; 64] = [
    0x26, 0xb8, 0x12, 0xb4, 0x00, 0x20, 0x20, 0x5b, 0xfe, 0x4d, 0xa1, 0xe9, 0x31, 0x9a, 0xb2, 0x48,
    0x29, 0xa6, 0x3b, 0x3a, 0x58, 0x38, 0x96, 0x12, 0xb4, 0x50, 0x01, 0x39, 0x24, 0x25, 0x0c, 0x7e,
    0xd5, 0x87, 0x67, 0x5d, 0x79, 0xa1, 0xe2, 0x08, 0xb8, 0x29, 0x00, 0x5b, 0xfd, 0x07, 0x7f, 0x35,
    0x34, 0xce, 0x98, 0x03, 0x3d, 0xa2, 0x43, 0x78, 0xab, 0xe1, 0x39, 0xb0, 0x3f, 0xa7, 0x16, 0x03,
];

pub(super) fn golden_challenge() -> ServerChallenge {
    ServerChallenge {
        protocol_version: CONTROL_WIRE_VERSION,
        challenge_id: GOLDEN_CHALLENGE_ID.to_vec(),
        server_nonce: GOLDEN_SERVER_NONCE.to_vec(),
        expires_at_unix_ms: 0,
        max_client_init_bytes: 49_152,
    }
}

pub(super) fn golden_proof() -> ClientProof {
    ClientProof {
        challenge_id: GOLDEN_CHALLENGE_ID.to_vec(),
        client_nonce: CLIENT_NONCE.to_vec(),
        control_public_key: CONTROL_KEY_SEED_PUBLIC_KEY.to_vec(),
        machine_hardware_id: MACHINE_HARDWARE_ID.to_owned(),
        claimed_device_id: None,
        intent: ProofIntent::FirstEnrollment as i32,
        enrollment_attempt_id: ENROLLMENT_ATTEMPT_ID.to_vec(),
        client_init_sha256: PROTO_CLIENT_INIT_SHA256.to_vec(),
        signature: PROTO_EXPECTED_SIGNATURE.to_vec(),
    }
}

pub(super) fn canonical_client_init(claimed_device_id: Option<&str>) -> ClientInit {
    ClientInit {
        protocol_version: CONTROL_WIRE_VERSION,
        intent: claimed_device_id.map_or(ProofIntent::FirstEnrollment, |_| ProofIntent::Resume)
            as i32,
        machine_hardware_id: MACHINE_HARDWARE_ID.to_owned(),
        claimed_device_id: claimed_device_id.map(str::to_owned),
        control_public_key: CONTROL_KEY_SEED_PUBLIC_KEY.to_vec(),
        client_nonce: CLIENT_NONCE.to_vec(),
        enrollment_attempt_id: ENROLLMENT_ATTEMPT_ID.to_vec(),
        hardware_claim: Some(HardwareClaim {
            candidates: vec![
                HardwareCandidate {
                    anchor_kind: "dmi_system_uuid".to_owned(),
                    candidate_id: OTHER_MACHINE_HARDWARE_ID.to_owned(),
                    quality: EvidenceQuality::Strong as i32,
                },
                HardwareCandidate {
                    anchor_kind: "dmi_board_serial".to_owned(),
                    candidate_id: SECOND_HARDWARE_CANDIDATE_ID.to_owned(),
                    quality: EvidenceQuality::Medium as i32,
                },
            ],
            completeness: CollectionCompleteness::Complete as i32,
        }),
        gateway_csr_der: vec![0x30, 0x00],
        daemon_version: "2.0.0".to_owned(),
        agent_version: "2.0.0".to_owned(),
        boot_id: "01900000-0000-7000-8000-000000000004".to_owned(),
        capabilities: vec!["observed-state".to_owned(), "binding".to_owned()],
        last_observed_sequence: 9,
        last_applied_generation: 7,
        last_applied_hash: vec![0x55; 32],
    }
}

pub(super) fn independent_transcript(
    challenge: &ServerChallenge,
    proof: &ClientProof,
    route: &str,
    subprotocol: &str,
) -> Vec<u8> {
    let Ok(machine_hardware_id) = Uuid::parse_str(&proof.machine_hardware_id) else {
        panic!("test Machine Hardware ID must parse");
    };
    let claimed_device_id = proof.claimed_device_id.as_deref().map(|value| {
        let Ok(device_id) = Uuid::parse_str(value) else {
            panic!("test Device ID must parse");
        };
        device_id
    });
    let Ok(intent) = u8::try_from(proof.intent) else {
        panic!("test intent must fit one byte");
    };

    let mut transcript = Vec::with_capacity(268);
    transcript.extend_from_slice(b"NATSUME-WSS-CONTROL-PROOF-v1\0");
    append_text(&mut transcript, route);
    append_text(&mut transcript, subprotocol);
    transcript.extend_from_slice(&challenge.challenge_id);
    transcript.extend_from_slice(&challenge.server_nonce);
    transcript.extend_from_slice(&proof.client_nonce);
    transcript.extend_from_slice(&challenge.protocol_version.to_be_bytes());
    transcript.push(intent);
    transcript.extend_from_slice(&proof.control_public_key);
    transcript.extend_from_slice(machine_hardware_id.as_bytes());
    transcript.push(u8::from(claimed_device_id.is_some()));
    if let Some(device_id) = claimed_device_id {
        transcript.extend_from_slice(device_id.as_bytes());
    }
    transcript.extend_from_slice(&proof.enrollment_attempt_id);
    transcript.extend_from_slice(&proof.client_init_sha256);
    transcript
}

fn append_text(output: &mut Vec<u8>, value: &str) {
    let Ok(length) = u16::try_from(value.len()) else {
        panic!("test transcript context must fit u16");
    };
    output.extend_from_slice(&length.to_be_bytes());
    output.extend_from_slice(value.as_bytes());
}
