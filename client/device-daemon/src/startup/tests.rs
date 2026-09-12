use std::fs;

use tempfile::TempDir;

use super::*;
use crate::{control::ControlLoopError, identity_record::IdentityRecordState};

const MACHINE_ID: Uuid = Uuid::from_u128(0x0c0f_ef01_1126_5297_a522_92cf_e494_ce48);

fn run_with_decision(
    paths: &StartupPaths,
    decision: Result<DerivedMachineIdentity, MachineIdentityError>,
) -> Result<StartupIdentityState, StartupError> {
    let context = preflight(paths, false)?;
    apply_identity_decision(paths, &context, decision).map(|ready| ready.state)
}

fn tempdir() -> TempDir {
    match TempDir::new() {
        Ok(directory) => directory,
        Err(error) => panic!("test directory must be created: {error}"),
    }
}

fn fixture_paths(directory: &TempDir) -> StartupPaths {
    let paths = StartupPaths {
        config: directory.path().join("etc/natsume/config.toml"),
        identity_directory: directory.path().join("var/lib/natsume/identity"),
        control_directory: directory.path().join("var/lib/natsume/control"),
        keys_directory: directory.path().join("var/lib/natsume/keys"),
        state_directory: directory.path().join("var/lib/natsume/state"),
    };
    for path in [
        paths.config.parent(),
        Some(paths.identity_directory.as_path()),
        Some(paths.control_directory.as_path()),
        Some(paths.keys_directory.as_path()),
        Some(paths.state_directory.as_path()),
    ] {
        let Some(path) = path else {
            panic!("fixture path must have a parent");
        };
        if let Err(error) = fs::create_dir_all(path) {
            panic!("fixture directory must be created: {error}");
        }
    }
    write_site(&paths, "gateway.example");
    paths
}

fn write_site(paths: &StartupPaths, gateway_hostname: &str) {
    let content = format!(
        "[server]\nip = \"192.0.2.10\"\nport = 8443\n\n[site]\ngateway_hostname = \"{gateway_hostname}\"\n"
    );
    if let Err(error) = fs::write(&paths.config, content) {
        panic!("site fixture must be written: {error}");
    }
}

fn derived_identity(machine_hardware_id: Uuid) -> DerivedMachineIdentity {
    DerivedMachineIdentity {
        machine_hardware_id: machine_hardware_id.to_string(),
        quality: MachineIdentityQuality::Strong,
    }
}

fn insufficient_identity() -> Result<DerivedMachineIdentity, MachineIdentityError> {
    Err(MachineIdentityError::InsufficientSources(String::new()))
}

fn unsupported_identity() -> Result<DerivedMachineIdentity, MachineIdentityError> {
    Err(MachineIdentityError::Unsupported(String::new()))
}

fn assert_failure_state(
    result: Result<StartupIdentityState, StartupError>,
    expected: StartupIdentityState,
) {
    match result {
        Err(StartupError::FailClosed { state }) => assert_eq!(state, state_label(expected)),
        Err(other) => panic!("unexpected startup failure: {other}"),
        Ok(state) => panic!("startup unexpectedly succeeded in state {state:?}"),
    }
}

#[test]
fn control_failure_keeps_its_safe_source_visible() {
    let error = StartupError::Control {
        source: ControlLoopError::EndpointConfiguration,
    };

    assert_eq!(
        error.to_string(),
        "device control loop failed closed: the Device control endpoint configuration is invalid"
    );
    assert_eq!(
        std::error::Error::source(&error).map(ToString::to_string),
        Some("the Device control endpoint configuration is invalid".to_owned())
    );
}

#[test]
fn clean_first_start_writes_the_pinned_record() {
    let directory = tempdir();
    let paths = fixture_paths(&directory);

    let result = run_with_decision(&paths, Ok(derived_identity(MACHINE_ID)));

    assert!(matches!(result, Ok(StartupIdentityState::CleanFirstStart)));
    assert_eq!(
        identity_record::read(&paths.identity_directory),
        IdentityRecordState::Valid {
            machine_hardware_id: MACHINE_ID,
        }
    );
    let content = match fs::read_to_string(paths.identity_directory.join("identity.json")) {
        Ok(content) => content,
        Err(error) => panic!("identity record must be readable: {error}"),
    };
    assert_eq!(
        content,
        r#"{"machine_hardware_id":"0c0fef01-1126-5297-a522-92cfe494ce48"}"#
    );
}

#[test]
fn matching_recomputed_identity_is_ready() {
    let directory = tempdir();
    let paths = fixture_paths(&directory);
    if let Err(error) = identity_record::write_first_start(&paths.identity_directory, MACHINE_ID) {
        panic!("identity fixture must be written: {error}");
    }

    let result = run_with_decision(&paths, Ok(derived_identity(MACHINE_ID)));

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
        run_with_decision(&paths, Ok(derived_identity(MACHINE_ID))),
        StartupIdentityState::IdentityRecordMissingOrCorrupt,
    );
}

#[test]
fn changed_recomputed_identity_requires_reset() {
    let directory = tempdir();
    let paths = fixture_paths(&directory);
    if let Err(error) = identity_record::write_first_start(&paths.identity_directory, MACHINE_ID) {
        panic!("identity fixture must be written: {error}");
    }

    assert_failure_state(
        run_with_decision(
            &paths,
            Ok(derived_identity(Uuid::new_v5(
                &Uuid::NAMESPACE_OID,
                b"different-machine",
            ))),
        ),
        StartupIdentityState::ResetRequired,
    );
}

#[test]
fn too_few_recomputed_sources_are_indeterminate() {
    let directory = tempdir();
    let paths = fixture_paths(&directory);
    if let Err(error) = identity_record::write_first_start(&paths.identity_directory, MACHINE_ID) {
        panic!("identity fixture must be written: {error}");
    }

    assert_failure_state(
        run_with_decision(&paths, insufficient_identity()),
        StartupIdentityState::Indeterminate,
    );
}

#[test]
fn unsupported_recomputed_identity_is_unavailable() {
    let directory = tempdir();
    let paths = fixture_paths(&directory);
    if let Err(error) = identity_record::write_first_start(&paths.identity_directory, MACHINE_ID) {
        panic!("identity fixture must be written: {error}");
    }

    assert_failure_state(
        run_with_decision(&paths, unsupported_identity()),
        StartupIdentityState::IdentityUnavailable,
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
        run_with_decision(&paths, Ok(derived_identity(MACHINE_ID))),
        StartupIdentityState::IdentityRecordMissingOrCorrupt,
    );
}

#[test]
fn privileged_home_state_without_a_record_fails_closed_before_identity_claim() {
    let directory = tempdir();
    let paths = fixture_paths(&directory);

    assert_failure_state(
        preflight(&paths, true).and_then(|context| {
            apply_identity_decision(&paths, &context, Ok(derived_identity(MACHINE_ID)))
                .map(|ready| ready.state)
        }),
        StartupIdentityState::IdentityRecordMissingOrCorrupt,
    );
    assert_eq!(
        identity_record::read(&paths.identity_directory),
        IdentityRecordState::Absent
    );
}

#[test]
fn control_artifacts_without_a_record_fail_closed_before_identity_claim() {
    let directory = tempdir();
    let paths = fixture_paths(&directory);
    let nested = paths.control_directory.join("nested");
    if let Err(error) = fs::create_dir(&nested) {
        panic!("nested control fixture directory must be created: {error}");
    }
    if let Err(error) = fs::write(nested.join("manifest.json"), b"identity-bound") {
        panic!("control fixture must be written: {error}");
    }

    assert_failure_state(
        run_with_decision(&paths, Ok(derived_identity(MACHINE_ID))),
        StartupIdentityState::IdentityRecordMissingOrCorrupt,
    );
}

#[test]
fn first_start_without_two_sources_has_no_identity() {
    let directory = tempdir();
    let paths = fixture_paths(&directory);

    assert_failure_state(
        run_with_decision(&paths, insufficient_identity()),
        StartupIdentityState::IdentityUnavailable,
    );
}

#[test]
fn deployment_example_initializes_identity_with_only_gateway_site_config() {
    let directory = tempdir();
    let paths = fixture_paths(&directory);
    let example = include_str!("../../../../packaging/client/config.example.toml");
    fs::write(&paths.config, example)
        .unwrap_or_else(|error| panic!("configuration fixture must be written: {error}"));
    assert_eq!(
        run_with_decision(&paths, Ok(derived_identity(MACHINE_ID))).unwrap_or_else(|error| panic!(
            "single configuration must initialize identity: {error}"
        )),
        StartupIdentityState::CleanFirstStart
    );
    fs::write(&paths.config, example.replace("[site]", "[unrelated]"))
        .unwrap_or_else(|error| panic!("configuration fixture must be written: {error}"));
    assert!(matches!(
        read_site_config(&paths.config),
        Err(StartupError::SiteConfiguration)
    ));
}

#[test]
fn site_configuration_must_exist() {
    let directory = tempdir();
    let paths = fixture_paths(&directory);

    if let Err(error) = fs::remove_file(&paths.config) {
        panic!("site fixture must be removed: {error}");
    }
    assert!(matches!(
        run_with_decision(&paths, Ok(derived_identity(MACHINE_ID))),
        Err(StartupError::SiteConfiguration)
    ));
}

#[test]
fn site_configuration_requires_a_canonical_dns_gateway_hostname() {
    let directory = tempdir();
    let paths = fixture_paths(&directory);

    for hostname in [
        "Gateway.example",
        "gateway.example.",
        "192.0.2.1",
        "-gateway.example",
    ] {
        write_site(&paths, hostname);
        assert!(matches!(
            read_site_config(&paths.config),
            Err(StartupError::SiteConfiguration)
        ));
    }
}

#[test]
fn invalid_derived_machine_id_fails_before_first_identity_write() {
    let directory = tempdir();
    let paths = fixture_paths(&directory);
    let decision = Ok(DerivedMachineIdentity {
        machine_hardware_id: "not-a-machine-id".to_owned(),
        quality: MachineIdentityQuality::Strong,
    });

    assert_failure_state(
        run_with_decision(&paths, decision),
        StartupIdentityState::IdentityUnavailable,
    );
    assert_eq!(
        identity_record::read(&paths.identity_directory),
        IdentityRecordState::Absent
    );
}

#[test]
fn orphaned_atomic_temporaries_are_not_identity_bound_artifacts() {
    let directory = tempdir();
    let paths = fixture_paths(&directory);
    for path in [
        paths.keys_directory.join(".natsume-tmp-key"),
        paths.control_directory.join(".natsume-tmp-manifest"),
        paths.state_directory.join(".natsume-tmp-state"),
    ] {
        if let Err(error) = fs::write(path, b"incomplete") {
            panic!("orphaned temporary fixture must be written: {error}");
        }
    }

    assert!(matches!(
        identity_bound_artifacts_present(
            &paths.keys_directory,
            &paths.control_directory,
            &paths.state_directory,
        ),
        Ok(false)
    ));
}

#[test]
fn normal_regular_file_is_an_identity_bound_artifact() {
    let directory = tempdir();
    let paths = fixture_paths(&directory);
    if let Err(error) = fs::write(paths.keys_directory.join("existing-artifact"), b"durable") {
        panic!("identity-bound artifact fixture must be written: {error}");
    }

    assert!(matches!(
        identity_bound_artifacts_present(
            &paths.keys_directory,
            &paths.control_directory,
            &paths.state_directory,
        ),
        Ok(true)
    ));
}

#[test]
fn reconciliation_artifact_without_identity_record_fails_closed() {
    let directory = tempdir();
    let paths = fixture_paths(&directory);
    if let Err(error) = fs::write(
        paths.state_directory.join("runtime-config.json"),
        b"durable",
    ) {
        panic!("reconciliation artifact fixture must be written: {error}");
    }

    assert_failure_state(
        run_with_decision(&paths, Ok(derived_identity(MACHINE_ID))),
        StartupIdentityState::IdentityRecordMissingOrCorrupt,
    );
}

#[test]
fn control_identity_is_loaded_after_the_identity_gate() {
    let directory = tempdir();
    let paths = fixture_paths(&directory);
    if let Err(error) = identity_record::write_first_start(&paths.identity_directory, MACHINE_ID) {
        panic!("identity fixture must be written: {error}");
    }
    let result = load_control_identity(&paths, MACHINE_ID);

    assert!(result.is_ok());
    assert!(paths.control_directory.join("control-key-1.pk8").is_file());
    assert!(paths.control_directory.join("manifest.json").is_file());
}

#[test]
fn absent_or_mismatched_persisted_identity_causes_zero_control_writes() {
    let directory = tempdir();
    let paths = fixture_paths(&directory);

    let absent = load_control_identity(&paths, MACHINE_ID);
    assert!(matches!(
        absent,
        Err(StartupError::FailClosed {
            state: "identity_record_missing_or_corrupt"
        })
    ));

    if let Err(error) =
        identity_record::write_first_start(&paths.identity_directory, Uuid::from_u128(2))
    {
        panic!("mismatched identity fixture must be written: {error}");
    }
    let mismatched = load_control_identity(&paths, MACHINE_ID);
    assert!(matches!(
        mismatched,
        Err(StartupError::FailClosed {
            state: "reset_required"
        })
    ));
    let entry_count = match fs::read_dir(&paths.control_directory) {
        Ok(entries) => entries.count(),
        Err(error) => panic!("control fixture directory must be readable: {error}"),
    };
    assert_eq!(entry_count, 0);
}
