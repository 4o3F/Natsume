use std::collections::BTreeSet;

use natsume_device_protocol::{
    CONTROL_MAX_ACTIVE_MESSAGE_BYTES, CONTROL_MAX_CLIENT_INIT_BYTES, CONTROL_MAX_PROOF_BYTES,
    CONTROL_ROUTE, CONTROL_SUBPROTOCOL, CONTROL_WIRE_VERSION,
};
use prost::Message as _;
use prost_types::{DescriptorProto, FileDescriptorProto, FileDescriptorSet};

#[test]
fn descriptor_is_the_exact_six_file_single_package_golden() {
    let generated = natsume_device_protocol::file_descriptor_set();
    let golden = include_bytes!("../../../crates/device-protocol/testdata/device_control.pb");
    assert_eq!(generated, golden);

    let descriptor = decode_descriptor(generated);
    let names = descriptor
        .file
        .iter()
        .filter_map(|file| file.name.as_deref())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        names,
        BTreeSet::from([
            "device_control.proto",
            "device_control_binding.proto",
            "device_control_command.proto",
            "device_control_common.proto",
            "device_control_handshake.proto",
            "device_control_observed.proto",
        ])
    );
    assert!(
        descriptor
            .file
            .iter()
            .all(|file| file.package.as_deref() == Some("natsume.device.control"))
    );

    assert_eq!(
        dependencies(descriptor_file(&descriptor, "device_control.proto")),
        BTreeSet::from([
            "device_control_binding.proto",
            "device_control_command.proto",
            "device_control_common.proto",
            "device_control_handshake.proto",
            "device_control_observed.proto",
        ])
    );
    assert_eq!(
        dependencies(descriptor_file(
            &descriptor,
            "device_control_observed.proto"
        )),
        BTreeSet::from(["device_control_common.proto"])
    );
    for name in [
        "device_control_binding.proto",
        "device_control_command.proto",
        "device_control_common.proto",
        "device_control_handshake.proto",
    ] {
        assert!(descriptor_file(&descriptor, name).dependency.is_empty());
    }
}

#[test]
fn root_pins_current_and_direction_envelopes_only() {
    let descriptor = decode_descriptor(natsume_device_protocol::file_descriptor_set());
    let root = descriptor_file(&descriptor, "device_control.proto");
    let root_messages = root
        .message_type
        .iter()
        .filter_map(|message| message.name.as_deref())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        root_messages,
        BTreeSet::from([
            "ClientActiveEnvelope",
            "ClientHandshakeEnvelope",
            "ControlEnvelope",
            "ServerActiveEnvelope",
            "ServerHandshakeEnvelope",
        ])
    );

    assert_field_numbers(
        &descriptor,
        "ControlEnvelope",
        &[
            ("client_hello", 1),
            ("server_hello", 2),
            ("heartbeat", 3),
            ("observed_state", 4),
            ("command", 5),
            ("command_status", 6),
            ("binding_request", 7),
            ("binding_result", 8),
            ("server_drain", 9),
            ("protocol_error", 10),
        ],
    );
    assert_field_numbers(
        &descriptor,
        "ClientHandshakeEnvelope",
        &[("credential_ack", 1), ("enrollment_heartbeat", 2)],
    );
    assert_field_numbers(
        &descriptor,
        "ServerHandshakeEnvelope",
        &[
            ("enrollment_pending", 1),
            ("credential_bundle", 2),
            ("session_ready", 3),
            ("protocol_error", 4),
            ("server_drain", 5),
        ],
    );
    assert_field_numbers(
        &descriptor,
        "ClientActiveEnvelope",
        &[
            ("session_id", 1),
            ("authority_revision", 2),
            ("client_sequence", 3),
            ("heartbeat", 10),
            ("command_status", 11),
            ("observed_state", 12),
            ("binding_result", 13),
            ("gateway_refresh", 14),
        ],
    );
    assert_field_numbers(
        &descriptor,
        "ServerActiveEnvelope",
        &[
            ("session_id", 1),
            ("authority_revision", 2),
            ("server_sequence", 3),
            ("command", 10),
            ("binding_request", 11),
            ("heartbeat", 12),
            ("protocol_error", 13),
            ("server_drain", 14),
        ],
    );
}

#[test]
fn challenge_proof_and_init_remain_standalone() {
    let descriptor = decode_descriptor(natsume_device_protocol::file_descriptor_set());
    let handshake = descriptor_file(&descriptor, "device_control_handshake.proto");
    let handshake_messages = handshake
        .message_type
        .iter()
        .filter_map(|message| message.name.as_deref())
        .collect::<BTreeSet<_>>();
    for standalone in ["ServerChallenge", "ClientProof", "ClientInit"] {
        assert!(handshake_messages.contains(standalone));
    }

    let root = descriptor_file(&descriptor, "device_control.proto");
    for envelope in &root.message_type {
        for field in &envelope.field {
            if let Some(type_name) = field.type_name.as_deref() {
                assert!(!matches!(
                    type_name.rsplit('.').next(),
                    Some("ServerChallenge" | "ClientProof" | "ClientInit")
                ));
            }
        }
    }
}

#[test]
fn live_control_constants_are_frozen() {
    assert_eq!(CONTROL_ROUTE, "/api/v2/device/control");
    assert_eq!(CONTROL_SUBPROTOCOL, "natsume.control");
    assert_eq!(CONTROL_WIRE_VERSION, 1);
    assert_eq!(CONTROL_MAX_PROOF_BYTES, 1_024);
    assert_eq!(CONTROL_MAX_CLIENT_INIT_BYTES, 48 * 1_024);
    assert_eq!(CONTROL_MAX_ACTIVE_MESSAGE_BYTES, 64 * 1_024);
}

fn decode_descriptor(bytes: &[u8]) -> FileDescriptorSet {
    let Ok(descriptor) = FileDescriptorSet::decode(bytes) else {
        panic!("descriptor must decode");
    };
    descriptor
}

fn descriptor_file<'a>(descriptor: &'a FileDescriptorSet, name: &str) -> &'a FileDescriptorProto {
    let Some(file) = descriptor
        .file
        .iter()
        .find(|file| file.name.as_deref() == Some(name))
    else {
        panic!("descriptor file must exist: {name}");
    };
    file
}

fn dependencies(file: &FileDescriptorProto) -> BTreeSet<&str> {
    file.dependency.iter().map(String::as_str).collect()
}

fn descriptor_message<'a>(descriptor: &'a FileDescriptorSet, name: &str) -> &'a DescriptorProto {
    let Some(message) = descriptor
        .file
        .iter()
        .flat_map(|file| &file.message_type)
        .find(|message| message.name.as_deref() == Some(name))
    else {
        panic!("descriptor message must exist: {name}");
    };
    message
}

fn assert_field_numbers(
    descriptor: &FileDescriptorSet,
    message_name: &str,
    expected: &[(&str, i32)],
) {
    let message = descriptor_message(descriptor, message_name);
    assert_eq!(message.field.len(), expected.len());
    for (field_name, field_number) in expected {
        let Some(field) = message
            .field
            .iter()
            .find(|field| field.name.as_deref() == Some(*field_name))
        else {
            panic!("descriptor field must exist: {message_name}.{field_name}");
        };
        assert_eq!(field.number, Some(*field_number));
    }
}
