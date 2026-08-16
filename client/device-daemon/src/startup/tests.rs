use std::fs;

use natsume_machine_identity::IdentityRecordState;
use rcgen::{CertificateParams, KeyPair, PKCS_ECDSA_P256_SHA256};
use tempfile::TempDir;

use super::*;

const NAMESPACE: Uuid = Uuid::from_u128(0x1234_5678_1234_5678_9234_5678_1234_5678);
const MACHINE_ID: Uuid = Uuid::from_u128(0xa9aa_9d04_3ece_5567_8260_9109_30ff_5e03);

fn tempdir() -> TempDir {
    match TempDir::new() {
        Ok(directory) => directory,
        Err(error) => panic!("test directory must be created: {error}"),
    }
}

fn fixture_paths(directory: &TempDir) -> StartupPaths {
    let paths = StartupPaths {
        site_config: directory.path().join("etc/natsume/site.toml"),
        identity_directory: directory.path().join("var/lib/natsume/identity"),
        keys_directory: directory.path().join("var/lib/natsume/keys"),
        enrollment: EnrollmentPaths::new(
            directory.path().join("etc/natsume/config.toml"),
            directory.path().join("etc/natsume/trust/control-ca.crt"),
            directory
                .path()
                .join("etc/natsume/trust/local-origin-ca.crt"),
            directory.path().join("var/lib/natsume/keys"),
        ),
    };
    for path in [
        paths.site_config.parent(),
        Some(paths.identity_directory.as_path()),
        Some(paths.keys_directory.as_path()),
    ] {
        let Some(path) = path else {
            panic!("fixture path must have a parent");
        };
        if let Err(error) = fs::create_dir_all(path) {
            panic!("fixture directory must be created: {error}");
        }
    }
    write_site(&paths, NAMESPACE.to_string().as_str());
    paths
}

fn write_site(paths: &StartupPaths, namespace: &str) {
    let content = format!(
        "schema_version = 1\nfleet_namespace_uuid = \"{namespace}\"\ngateway_hostname = \"gateway.example\"\n"
    );
    if let Err(error) = fs::write(&paths.site_config, content) {
        panic!("site fixture must be written: {error}");
    }
}

fn derived_claim(machine_hardware_id: Uuid) -> SanitizedHardwareClaim {
    SanitizedHardwareClaim {
        candidates: vec![
            HardwareCandidate {
                anchor_kind: "dmi_system_uuid".to_owned(),
                candidate_id: Uuid::new_v5(&NAMESPACE, b"system").to_string(),
                quality: "strong".to_owned(),
            },
            HardwareCandidate {
                anchor_kind: "dmi_board_serial".to_owned(),
                candidate_id: Uuid::new_v5(&NAMESPACE, b"board").to_string(),
                quality: "strong".to_owned(),
            },
            HardwareCandidate {
                anchor_kind: "first_disk_serial".to_owned(),
                candidate_id: Uuid::new_v5(&NAMESPACE, b"disk").to_string(),
                quality: "medium".to_owned(),
            },
        ],
        collection_complete: true,
        decision: "derived".to_owned(),
        machine_hardware_id: Some(machine_hardware_id.to_string()),
        present_slot_count: 3,
    }
}

fn insufficient_claim() -> SanitizedHardwareClaim {
    SanitizedHardwareClaim {
        candidates: vec![HardwareCandidate {
            anchor_kind: "dmi_system_uuid".to_owned(),
            candidate_id: Uuid::new_v5(&NAMESPACE, b"system").to_string(),
            quality: "strong".to_owned(),
        }],
        collection_complete: false,
        decision: "insufficient_sources".to_owned(),
        machine_hardware_id: None,
        present_slot_count: 1,
    }
}

fn assert_failure_state(
    result: Result<StartupIdentityState, StartupError>,
    expected: StartupIdentityState,
) {
    match result {
        Err(StartupError::FailClosed { state }) => assert_eq!(state, expected),
        Err(other) => panic!("unexpected startup failure: {other}"),
        Ok(state) => panic!("startup unexpectedly succeeded in state {state:?}"),
    }
}

#[test]
fn clean_first_start_writes_the_pinned_record() {
    let directory = tempdir();
    let paths = fixture_paths(&directory);

    let result = run_with_claim(&paths, &derived_claim(MACHINE_ID));

    assert!(matches!(result, Ok(StartupIdentityState::CleanFirstStart)));
    assert_eq!(
        identity_record::read(&paths.identity_directory),
        IdentityRecordState::Valid {
            fleet_namespace_uuid: NAMESPACE,
            machine_hardware_id: MACHINE_ID,
        }
    );
    let content = match fs::read_to_string(paths.identity_directory.join("identity.json")) {
        Ok(content) => content,
        Err(error) => panic!("identity record must be readable: {error}"),
    };
    assert_eq!(
        content,
        r#"{"fleet_namespace_uuid":"12345678-1234-5678-9234-567812345678","machine_hardware_id":"a9aa9d04-3ece-5567-8260-910930ff5e03"}"#
    );
}

#[test]
fn matching_recomputed_identity_is_ready() {
    let directory = tempdir();
    let paths = fixture_paths(&directory);
    if let Err(error) =
        identity_record::write_first_start(&paths.identity_directory, NAMESPACE, MACHINE_ID)
    {
        panic!("identity fixture must be written: {error}");
    }

    let result = run_with_claim(&paths, &derived_claim(MACHINE_ID));

    assert!(matches!(result, Ok(StartupIdentityState::Matched)));
}

#[test]
fn corrupt_record_fails_closed() {
    let directory = tempdir();
    let paths = fixture_paths(&directory);
    if let Err(error) = fs::write(
        paths.identity_directory.join("identity.json"),
        b"truncated {",
    ) {
        panic!("corrupt identity fixture must be written: {error}");
    }

    assert_failure_state(
        run_with_claim(&paths, &derived_claim(MACHINE_ID)),
        StartupIdentityState::IdentityRecordMissingOrCorrupt,
    );
}

#[test]
fn site_namespace_mismatch_fails_closed() {
    let directory = tempdir();
    let paths = fixture_paths(&directory);
    if let Err(error) = identity_record::write_first_start(
        &paths.identity_directory,
        Uuid::from_u128(1),
        MACHINE_ID,
    ) {
        panic!("identity fixture must be written: {error}");
    }

    assert_failure_state(
        run_with_claim(&paths, &derived_claim(MACHINE_ID)),
        StartupIdentityState::SiteNamespaceMismatch,
    );
}

#[test]
fn changed_recomputed_identity_requires_reset() {
    let directory = tempdir();
    let paths = fixture_paths(&directory);
    if let Err(error) =
        identity_record::write_first_start(&paths.identity_directory, NAMESPACE, MACHINE_ID)
    {
        panic!("identity fixture must be written: {error}");
    }

    assert_failure_state(
        run_with_claim(&paths, &derived_claim(Uuid::from_u128(2))),
        StartupIdentityState::ResetRequired,
    );
}

#[test]
fn too_few_recomputed_sources_are_indeterminate() {
    let directory = tempdir();
    let paths = fixture_paths(&directory);
    if let Err(error) =
        identity_record::write_first_start(&paths.identity_directory, NAMESPACE, MACHINE_ID)
    {
        panic!("identity fixture must be written: {error}");
    }

    assert_failure_state(
        run_with_claim(&paths, &insufficient_claim()),
        StartupIdentityState::Indeterminate,
    );
}

#[test]
fn artifacts_without_a_record_fail_closed_before_identity_claim() {
    let directory = tempdir();
    let paths = fixture_paths(&directory);
    let nested = paths.keys_directory.join("gateway");
    if let Err(error) = fs::create_dir_all(&nested) {
        panic!("nested key fixture must be created: {error}");
    }
    if let Err(error) = fs::write(nested.join("token.bin"), b"identity-bound") {
        panic!("key fixture must be written: {error}");
    }

    assert_failure_state(
        run_with_claim(&paths, &derived_claim(MACHINE_ID)),
        StartupIdentityState::IdentityRecordMissingOrCorrupt,
    );
}

#[test]
fn first_start_without_two_sources_has_no_identity() {
    let directory = tempdir();
    let paths = fixture_paths(&directory);

    assert_failure_state(
        run_with_claim(&paths, &insufficient_claim()),
        StartupIdentityState::IdentityUnavailable,
    );
}

#[test]
fn site_configuration_must_exist_and_use_canonical_uuid() {
    let directory = tempdir();
    let paths = fixture_paths(&directory);
    write_site(&paths, "12345678-1234-5678-9234-56781234567A");
    assert!(matches!(
        run_with_claim(&paths, &derived_claim(MACHINE_ID)),
        Err(StartupError::SiteConfiguration)
    ));

    if let Err(error) = fs::remove_file(&paths.site_config) {
        panic!("site fixture must be removed: {error}");
    }
    assert!(matches!(
        run_with_claim(&paths, &derived_claim(MACHINE_ID)),
        Err(StartupError::SiteConfiguration)
    ));
}

#[test]
fn inconsistent_sanitized_claim_fails_closed() {
    let directory = tempdir();
    let paths = fixture_paths(&directory);
    let mut claim = derived_claim(MACHINE_ID);
    claim.machine_hardware_id = None;

    assert_failure_state(
        run_with_claim(&paths, &claim),
        StartupIdentityState::IdentityUnavailable,
    );
}

#[test]
fn completeness_inconsistent_claim_fails_closed() {
    let directory = tempdir();
    let paths = fixture_paths(&directory);
    let mut claim = derived_claim(MACHINE_ID);
    claim.collection_complete = false;

    assert_failure_state(
        run_with_claim(&paths, &claim),
        StartupIdentityState::IdentityUnavailable,
    );
}

#[test]
fn reported_whole_machine_quality_is_the_minimum_present_slot_quality() {
    let mut claim = derived_claim(MACHINE_ID);
    assert_eq!(whole_machine_quality(&claim), Some(EvidenceQuality::Medium));

    claim.candidates[2].quality = "weak".to_owned();
    assert_eq!(whole_machine_quality(&claim), Some(EvidenceQuality::Weak));

    claim.candidates.pop();
    claim.present_slot_count = 2;
    assert_eq!(whole_machine_quality(&claim), Some(EvidenceQuality::Strong));
}

#[test]
fn malformed_candidate_quality_fails_before_first_identity_write() {
    let directory = tempdir();
    let paths = fixture_paths(&directory);
    let mut claim = derived_claim(MACHINE_ID);
    claim.candidates[0].quality = "unreviewed".to_owned();

    assert_failure_state(
        run_with_claim(&paths, &claim),
        StartupIdentityState::IdentityUnavailable,
    );
    assert_eq!(
        identity_record::read(&paths.identity_directory),
        IdentityRecordState::Absent
    );
}

fn install_parseable_gateway_key_and_leaf(paths: &StartupPaths) {
    let key = match KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256) {
        Ok(key) => key,
        Err(error) => panic!("Gateway key fixture must be generated: {error}"),
    };
    let params = match CertificateParams::new(vec!["gateway.example".to_owned()]) {
        Ok(params) => params,
        Err(error) => panic!("Gateway leaf fixture parameters must be created: {error}"),
    };
    let leaf = match params.self_signed(&key) {
        Ok(leaf) => leaf,
        Err(error) => panic!("Gateway leaf fixture must be signed: {error}"),
    };
    if let Err(error) = fs::write(
        paths.keys_directory.join("gateway-key.pk8"),
        key.serialize_der(),
    ) {
        panic!("Gateway key fixture must be written: {error}");
    }
    if let Err(error) = fs::write(
        paths.keys_directory.join("gateway-leaf.der"),
        leaf.der().as_ref(),
    ) {
        panic!("Gateway leaf fixture must be written: {error}");
    }
}

#[test]
fn token_presence_marks_enrolled_only_with_parseable_key_and_leaf() {
    let directory = tempdir();
    let paths = fixture_paths(&directory);
    install_parseable_gateway_key_and_leaf(&paths);
    if let Err(error) = fs::write(paths.keys_directory.join("device-token"), b"opaque") {
        panic!("Device Token fixture must be written: {error}");
    }

    assert!(matches!(
        existing_enrollment_state(&paths),
        Ok(Some(StartupIdentityState::Enrolled))
    ));
}

#[test]
fn token_with_absent_or_corrupt_key_fails_closed_without_reenrollment() {
    let directory = tempdir();
    let paths = fixture_paths(&directory);
    if let Err(error) = fs::write(paths.keys_directory.join("device-token"), b"opaque") {
        panic!("Device Token fixture must be written: {error}");
    }
    assert!(matches!(
        existing_enrollment_state(&paths),
        Err(StartupError::Enrollment { .. })
    ));

    if let Err(error) = fs::write(paths.keys_directory.join("gateway-key.pk8"), b"corrupt") {
        panic!("corrupt Gateway key fixture must be written: {error}");
    }
    assert!(matches!(
        existing_enrollment_state(&paths),
        Err(StartupError::Enrollment { .. })
    ));
}

#[test]
fn token_with_missing_leaf_fails_closed_without_reenrollment() {
    let directory = tempdir();
    let paths = fixture_paths(&directory);
    let key = match KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256) {
        Ok(key) => key,
        Err(error) => panic!("Gateway key fixture must be generated: {error}"),
    };
    if let Err(error) = fs::write(
        paths.keys_directory.join("gateway-key.pk8"),
        key.serialize_der(),
    ) {
        panic!("Gateway key fixture must be written: {error}");
    }
    if let Err(error) = fs::write(paths.keys_directory.join("device-token"), b"opaque") {
        panic!("Device Token fixture must be written: {error}");
    }

    assert!(matches!(
        existing_enrollment_state(&paths),
        Err(StartupError::Enrollment { .. })
    ));
}
