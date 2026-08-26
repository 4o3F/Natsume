use std::{
    fs,
    os::unix::fs::{MetadataExt as _, PermissionsExt as _},
    path::Path,
};

use tempfile::{TempDir, tempdir};
use uuid::Uuid;

use super::super::{
    CONTROL_KEY_NAME, CONTROL_MANIFEST_NAME, DormantControlIdentityError, ensure_dormant_identity,
    reconcile_dormant_identity,
};

const MACHINE_ID: Uuid = Uuid::from_u128(0xa9aa_9d04_3ece_5567_8260_9109_30ff_5e03);
// Public deterministic test material copied from the Batch 0 RFC 8410 vector.
const PUBLIC_TEST_KEY_DER: [u8; 83] = [
    0x30, 0x51, 0x02, 0x01, 0x01, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20,
    0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11,
    0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11,
    0x81, 0x21, 0x00, 0xd0, 0x4a, 0xb2, 0x32, 0x74, 0x2b, 0xb4, 0xab, 0x3a, 0x13, 0x68, 0xbd, 0x46,
    0x15, 0xe4, 0xe6, 0xd0, 0x22, 0x4a, 0xb7, 0x1a, 0x01, 0x6b, 0xaf, 0x85, 0x20, 0xa3, 0x32, 0xc9,
    0x77, 0x87, 0x37,
];
const EXPECTED_PUBLIC_KEY: &str =
    "d04ab232742bb4ab3a1368bd4615e4e6d0224ab71a016baf8520a332c9778737";
const EXPECTED_MANIFEST: &str = concat!(
    r#"{"format_version":1,"machine_hardware_id":"a9aa9d04-3ece-5567-8260-910930ff5e03","#,
    r#""control_key_generation":1,"control_public_key":"d04ab232742bb4ab3a1368bd4615e4e6d0224ab71a016baf8520a332c9778737","state":"dormant"}"#
);

fn require_tempdir() -> TempDir {
    match tempdir() {
        Ok(directory) => directory,
        Err(error) => panic!("test directory must be created: {error}"),
    }
}

fn write_owner_only(path: &Path, bytes: &[u8]) {
    if let Err(error) = fs::write(path, bytes) {
        panic!("control artifact fixture must be written: {error}");
    }
    if let Err(error) = fs::set_permissions(path, fs::Permissions::from_mode(0o600)) {
        panic!("control artifact fixture mode must be set: {error}");
    }
}

fn require_read(path: &Path) -> Vec<u8> {
    match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => panic!("control artifact must be readable: {error}"),
    }
}

fn require_mode(path: &Path) -> u32 {
    match fs::symlink_metadata(path) {
        Ok(metadata) => metadata.mode() & 0o777,
        Err(error) => panic!("control artifact metadata must be readable: {error}"),
    }
}

fn raw_manifest(
    machine_hardware_id: &str,
    control_public_key: &str,
    state: &str,
    extra_field: &str,
) -> String {
    format!(
        r#"{{"format_version":1,"machine_hardware_id":"{machine_hardware_id}","control_key_generation":1,"control_public_key":"{control_public_key}","state":"{state}"{extra_field}}}"#
    )
}

#[test]
fn new_identity_is_owner_only_stable_and_has_no_credential_directory() {
    let directory = require_tempdir();
    if let Err(error) = ensure_dormant_identity(directory.path(), MACHINE_ID) {
        panic!("dormant control identity must be created: {error}");
    }
    let key_path = directory.path().join(CONTROL_KEY_NAME);
    let manifest_path = directory.path().join(CONTROL_MANIFEST_NAME);
    let first_key = require_read(&key_path);
    let first_manifest = require_read(&manifest_path);

    assert_eq!(first_key.len(), PUBLIC_TEST_KEY_DER.len());
    assert_eq!(require_mode(&key_path), 0o600);
    assert_eq!(require_mode(&manifest_path), 0o600);
    assert!(!directory.path().join("credentials").exists());

    if let Err(error) = ensure_dormant_identity(directory.path(), MACHINE_ID) {
        panic!("dormant control identity must reload: {error}");
    }
    assert_eq!(require_read(&key_path), first_key);
    assert_eq!(require_read(&manifest_path), first_manifest);
}

#[test]
fn separate_first_starts_generate_distinct_control_keys() {
    let first = require_tempdir();
    let second = require_tempdir();
    for directory in [&first, &second] {
        if let Err(error) = ensure_dormant_identity(directory.path(), MACHINE_ID) {
            panic!("dormant control identity must be created: {error}");
        }
    }

    let first_key = require_read(&first.path().join(CONTROL_KEY_NAME));
    let second_key = require_read(&second.path().join(CONTROL_KEY_NAME));
    assert_ne!(first_key, second_key);
}

#[test]
fn key_only_crash_state_converges_to_the_public_key_golden() {
    let directory = require_tempdir();
    let key_path = directory.path().join(CONTROL_KEY_NAME);
    write_owner_only(&key_path, &PUBLIC_TEST_KEY_DER);

    if let Err(error) = ensure_dormant_identity(directory.path(), MACHINE_ID) {
        panic!("key-only state must converge: {error}");
    }

    assert_eq!(require_read(&key_path), PUBLIC_TEST_KEY_DER);
    let manifest_path = directory.path().join(CONTROL_MANIFEST_NAME);
    assert_eq!(require_read(&manifest_path), EXPECTED_MANIFEST.as_bytes());
}

#[test]
fn corrupt_key_is_never_replaced_or_completed_with_a_manifest() {
    let directory = require_tempdir();
    let key_path = directory.path().join(CONTROL_KEY_NAME);
    let corrupt = b"not an Ed25519 PKCS#8 key";
    write_owner_only(&key_path, corrupt);

    let result = ensure_dormant_identity(directory.path(), MACHINE_ID);

    assert!(matches!(
        result,
        Err(DormantControlIdentityError::ControlKey)
    ));
    assert_eq!(require_read(&key_path), corrupt);
    assert!(!directory.path().join(CONTROL_MANIFEST_NAME).exists());
}

#[test]
fn manifest_without_key_fails_closed_without_creating_a_key() {
    let directory = require_tempdir();
    let manifest_path = directory.path().join(CONTROL_MANIFEST_NAME);
    write_owner_only(&manifest_path, EXPECTED_MANIFEST.as_bytes());

    let result = ensure_dormant_identity(directory.path(), MACHINE_ID);

    assert!(matches!(
        result,
        Err(DormantControlIdentityError::ControlKey)
    ));
    assert!(!directory.path().join(CONTROL_KEY_NAME).exists());
    assert_eq!(require_read(&manifest_path), EXPECTED_MANIFEST.as_bytes());
}

#[test]
fn concurrent_manifest_snapshot_reloads_a_winning_key_once() {
    let directory = require_tempdir();
    let key_path = directory.path().join(CONTROL_KEY_NAME);
    let manifest_path = directory.path().join(CONTROL_MANIFEST_NAME);
    write_owner_only(&manifest_path, EXPECTED_MANIFEST.as_bytes());
    let Some(stored_manifest) = super::load(&manifest_path).unwrap_or_else(|error| {
        panic!("manifest snapshot must load: {error}");
    }) else {
        panic!("manifest snapshot must exist");
    };
    write_owner_only(&key_path, &PUBLIC_TEST_KEY_DER);

    let result = reconcile_dormant_identity(
        &key_path,
        &manifest_path,
        MACHINE_ID,
        None,
        Some(stored_manifest),
    );

    assert!(result.is_ok());
    assert_eq!(require_read(&key_path), PUBLIC_TEST_KEY_DER);
    assert_eq!(require_read(&manifest_path), EXPECTED_MANIFEST.as_bytes());
}

#[test]
fn mismatched_noncanonical_and_unknown_manifest_values_make_zero_writes() {
    let invalid_manifests = [
        raw_manifest(
            "550e8400-e29b-51d4-a716-446655440000",
            EXPECTED_PUBLIC_KEY,
            "dormant",
            "",
        ),
        raw_manifest(
            "A9AA9D04-3ECE-5567-8260-910930FF5E03",
            EXPECTED_PUBLIC_KEY,
            "dormant",
            "",
        ),
        raw_manifest(
            &MACHINE_ID.to_string(),
            "0000000000000000000000000000000000000000000000000000000000000000",
            "dormant",
            "",
        ),
        raw_manifest(
            &MACHINE_ID.to_string(),
            &EXPECTED_PUBLIC_KEY.to_ascii_uppercase(),
            "dormant",
            "",
        ),
        raw_manifest(&MACHINE_ID.to_string(), EXPECTED_PUBLIC_KEY, "active", ""),
        raw_manifest(
            &MACHINE_ID.to_string(),
            EXPECTED_PUBLIC_KEY,
            "dormant",
            r#", "unknown":true"#,
        ),
        EXPECTED_MANIFEST.replacen("\"format_version\":1", "\"format_version\":2", 1),
        EXPECTED_MANIFEST.replacen(
            "\"control_key_generation\":1",
            "\"control_key_generation\":2",
            1,
        ),
    ];

    for invalid in invalid_manifests {
        let directory = require_tempdir();
        let key_path = directory.path().join(CONTROL_KEY_NAME);
        let manifest_path = directory.path().join(CONTROL_MANIFEST_NAME);
        write_owner_only(&key_path, &PUBLIC_TEST_KEY_DER);
        write_owner_only(&manifest_path, invalid.as_bytes());

        let result = ensure_dormant_identity(directory.path(), MACHINE_ID);

        assert!(matches!(result, Err(DormantControlIdentityError::Manifest)));
        assert_eq!(require_read(&key_path), PUBLIC_TEST_KEY_DER);
        assert_eq!(require_read(&manifest_path), invalid.as_bytes());
        let entry_count = match fs::read_dir(directory.path()) {
            Ok(entries) => entries.count(),
            Err(error) => panic!("control directory must be readable: {error}"),
        };
        assert_eq!(entry_count, 2, "manifest rejection created an artifact");
    }
}
