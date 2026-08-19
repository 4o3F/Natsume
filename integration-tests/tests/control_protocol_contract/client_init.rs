use natsume_device_protocol::{
    CONTROL_MAX_CLIENT_INIT_BYTES, HandshakeError, decode_client_init,
    encode_client_init_canonical,
    generated::{ClientInit, CollectionCompleteness, EvidenceQuality, ProofIntent},
    validate_client_init,
};
use sha2::{Digest as _, Sha256};

use super::fixture::{
    CLAIMED_DEVICE_ID, MACHINE_HARDWARE_ID, OTHER_MACHINE_HARDWARE_ID, PROTO_CLIENT_INIT_SHA256,
    RFC8032_PUBLIC_KEY, canonical_client_init,
};

#[test]
fn canonical_client_init_round_trips_with_optional_device_presence() {
    for claimed_device_id in [None, Some(CLAIMED_DEVICE_ID)] {
        let client_init = canonical_client_init(claimed_device_id);
        let bytes = canonical_bytes(&client_init);
        let Ok(decoded) = decode_client_init(&bytes) else {
            panic!("canonical ClientInit must decode");
        };
        assert_eq!(decoded, client_init);
        assert_eq!(canonical_bytes(&decoded), bytes);
    }
}

#[test]
fn semantic_equivalent_wire_encodings_share_the_canonical_hash() {
    let baseline = canonical_client_init(None);
    let canonical = canonical_bytes(&baseline);
    assert_eq!(canonical_sha256(&baseline), PROTO_CLIENT_INIT_SHA256);

    let mut reordered = canonical[2..4].to_vec();
    reordered.extend_from_slice(&canonical[..2]);
    reordered.extend_from_slice(&canonical[4..]);
    assert_normalizes_to(&reordered, &baseline, &canonical);

    let mut nonminimal_key = vec![0x88, 0x00];
    nonminimal_key.extend_from_slice(&canonical[1..]);
    assert_normalizes_to(&nonminimal_key, &baseline, &canonical);

    let mut nonminimal_value = vec![0x08, 0x81, 0x00];
    nonminimal_value.extend_from_slice(&canonical[2..]);
    assert_normalizes_to(&nonminimal_value, &baseline, &canonical);

    let mut unknown_field = canonical.clone();
    unknown_field.extend_from_slice(&[0x88, 0x01, 0x01]);
    assert_normalizes_to(&unknown_field, &baseline, &canonical);

    let mut duplicate_first_wins_nothing = vec![0x08, 0x02];
    duplicate_first_wins_nothing.extend_from_slice(&canonical);
    assert_normalizes_to(&duplicate_first_wins_nothing, &baseline, &canonical);

    let mut zero_sequence = baseline;
    zero_sequence.last_observed_sequence = 0;
    let zero_canonical = canonical_bytes(&zero_sequence);
    let mut explicit_default = zero_canonical.clone();
    explicit_default.extend_from_slice(&[0x70, 0x00]);
    assert_normalizes_to(&explicit_default, &zero_sequence, &zero_canonical);
}

#[test]
fn canonical_client_init_field_mutations_change_the_signed_hash() {
    let baseline = canonical_client_init(None);
    let mut mutations = Vec::new();

    let mut value = baseline.clone();
    OTHER_MACHINE_HARDWARE_ID.clone_into(&mut value.machine_hardware_id);
    mutations.push(value);
    let mut value = baseline.clone();
    value.intent = ProofIntent::Resume as i32;
    value.claimed_device_id = Some(CLAIMED_DEVICE_ID.to_owned());
    mutations.push(value);
    let mut value = baseline.clone();
    value.control_public_key = RFC8032_PUBLIC_KEY.to_vec();
    mutations.push(value);
    let mut value = baseline.clone();
    value.client_nonce[0] ^= 1;
    mutations.push(value);
    let mut value = baseline.clone();
    value.enrollment_attempt_id[15] ^= 1;
    mutations.push(value);
    let mut value = baseline.clone();
    let Some(candidate) = value
        .hardware_claim
        .as_mut()
        .and_then(|claim| claim.candidates.first_mut())
    else {
        panic!("fixture hardware candidate must exist");
    };
    candidate.quality = EvidenceQuality::Medium as i32;
    mutations.push(value);
    let mut value = baseline.clone();
    value.gateway_csr_der.push(1);
    mutations.push(value);
    let mut value = baseline.clone();
    value.daemon_version.push('1');
    mutations.push(value);
    let mut value = baseline.clone();
    value.agent_version.push('1');
    mutations.push(value);
    let mut value = baseline.clone();
    value.boot_id = "01900000-0000-7000-8000-000000000005".to_owned();
    mutations.push(value);
    let mut value = baseline.clone();
    value.capabilities.push("extra".to_owned());
    mutations.push(value);
    let mut value = baseline.clone();
    value.last_observed_sequence += 1;
    mutations.push(value);
    let mut value = baseline.clone();
    value.last_applied_generation += 1;
    mutations.push(value);
    let mut value = baseline;
    value.last_applied_hash[0] ^= 1;
    mutations.push(value);

    for changed in mutations {
        assert_ne!(canonical_sha256(&changed), PROTO_CLIENT_INIT_SHA256);
    }
}

#[test]
fn malformed_protobuf_is_rejected_by_prost_before_semantics() {
    let canonical = canonical_bytes(&canonical_client_init(None));

    let mut truncated = canonical.clone();
    assert!(truncated.pop().is_some());
    assert_eq!(
        decode_client_init(&truncated),
        Err(HandshakeError::ClientInitDecode)
    );

    let mut trailing_tag_zero = canonical.clone();
    trailing_tag_zero.push(0);
    assert_eq!(
        decode_client_init(&trailing_tag_zero),
        Err(HandshakeError::ClientInitDecode)
    );

    let mut wrong_wire_type = canonical.clone();
    wrong_wire_type[0] = 0x0a;
    assert_eq!(
        decode_client_init(&wrong_wire_type),
        Err(HandshakeError::ClientInitDecode)
    );

    let mut invalid_utf8 = canonical;
    let Some(version_offset) = invalid_utf8
        .windows(7)
        .position(|window| window == b"\x52\x05\x32\x2e\x30\x2e\x30")
    else {
        panic!("daemon version field must be present");
    };
    invalid_utf8[version_offset + 2] = 0xff;
    assert_eq!(
        decode_client_init(&invalid_utf8),
        Err(HandshakeError::ClientInitDecode)
    );
}

#[test]
fn decoded_but_invalid_semantics_are_rejected() {
    let mut value = canonical_client_init(None);
    value.protocol_version += 1;
    assert!(validate_client_init(&value).is_err());

    let mut value = canonical_client_init(None);
    value.intent = 99;
    assert!(validate_client_init(&value).is_err());

    let mut value = canonical_client_init(None);
    value.intent = ProofIntent::Resume as i32;
    assert!(validate_client_init(&value).is_err());

    let mut value = canonical_client_init(None);
    value.machine_hardware_id = "not-a-machine-id".to_owned();
    let Err(error) = validate_client_init(&value) else {
        panic!("invalid Machine Hardware ID must fail");
    };
    assert_eq!(error, HandshakeError::MachineHardwareId);
    assert_eq!(error.to_string(), "Machine Hardware ID is invalid");

    let mut value = canonical_client_init(None);
    value.claimed_device_id = Some(MACHINE_HARDWARE_ID.to_owned());
    assert!(validate_client_init(&value).is_err());

    let mut value = canonical_client_init(None);
    value.control_public_key = vec![0; 32];
    value.control_public_key[0] = 1;
    assert!(validate_client_init(&value).is_err());

    let mut value = canonical_client_init(None);
    value.client_nonce.pop();
    assert!(validate_client_init(&value).is_err());

    let mut value = canonical_client_init(None);
    value.enrollment_attempt_id[6] = 0x40;
    assert!(validate_client_init(&value).is_err());

    let mut value = canonical_client_init(None);
    value.hardware_claim = None;
    assert!(validate_client_init(&value).is_err());
}

#[test]
fn hardware_claim_and_last_applied_state_are_closed() {
    let mut value = canonical_client_init(None);
    let Some(claim) = value.hardware_claim.as_mut() else {
        panic!("fixture hardware claim must exist");
    };
    claim.completeness = CollectionCompleteness::Unspecified as i32;
    assert!(validate_client_init(&value).is_err());

    let mut value = canonical_client_init(None);
    let Some(claim) = value.hardware_claim.as_mut() else {
        panic!("fixture hardware claim must exist");
    };
    claim.candidates.pop();
    assert!(validate_client_init(&value).is_err());

    let mut value = canonical_client_init(None);
    let Some(claim) = value.hardware_claim.as_mut() else {
        panic!("fixture hardware claim must exist");
    };
    let duplicate_anchor = claim.candidates[0].anchor_kind.clone();
    claim.candidates[1].anchor_kind = duplicate_anchor;
    assert!(validate_client_init(&value).is_err());

    let mut value = canonical_client_init(None);
    let Some(candidate) = value
        .hardware_claim
        .as_mut()
        .and_then(|claim| claim.candidates.first_mut())
    else {
        panic!("fixture hardware candidate must exist");
    };
    candidate.anchor_kind.clear();
    assert!(validate_client_init(&value).is_err());

    let mut value = canonical_client_init(None);
    let Some(candidate) = value
        .hardware_claim
        .as_mut()
        .and_then(|claim| claim.candidates.first_mut())
    else {
        panic!("fixture hardware candidate must exist");
    };
    candidate.quality = EvidenceQuality::Unspecified as i32;
    assert!(validate_client_init(&value).is_err());

    let mut value = canonical_client_init(None);
    value.last_applied_hash.pop();
    assert!(validate_client_init(&value).is_err());

    let mut value = canonical_client_init(None);
    value.last_applied_generation = 0;
    assert!(validate_client_init(&value).is_err());

    let mut value = canonical_client_init(None);
    value.last_applied_generation = 0;
    value.last_applied_hash.clear();
    assert!(validate_client_init(&value).is_ok());
}

#[test]
fn client_init_size_is_bounded_before_decode() {
    let oversized = vec![0; CONTROL_MAX_CLIENT_INIT_BYTES + 1];
    assert_eq!(
        decode_client_init(&oversized),
        Err(HandshakeError::ClientInitTooLarge)
    );

    let mut value = canonical_client_init(None);
    value.gateway_csr_der = vec![0; CONTROL_MAX_CLIENT_INIT_BYTES];
    assert!(encode_client_init_canonical(&value).is_err());
}

fn canonical_bytes(value: &ClientInit) -> Vec<u8> {
    let Ok(bytes) = encode_client_init_canonical(value) else {
        panic!("valid ClientInit must encode");
    };
    bytes
}

fn canonical_sha256(value: &ClientInit) -> [u8; 32] {
    Sha256::digest(canonical_bytes(value)).into()
}

fn assert_normalizes_to(raw: &[u8], expected: &ClientInit, canonical: &[u8]) {
    let Ok(decoded) = decode_client_init(raw) else {
        panic!("semantically equivalent protobuf must decode");
    };
    assert_eq!(&decoded, expected);
    assert_eq!(canonical_bytes(&decoded), canonical);
    assert_eq!(canonical_sha256(&decoded), canonical_sha256(expected));
}
