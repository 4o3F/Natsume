use std::{fs, io, path::Path, path::PathBuf};

use natsume_local_control_api::{Privileged1Proxy, SanitizedHardwareClaim, StartupIdentityState};
use natsume_machine_identity::{
    CollectionCompleteness, LocalIdentityPreflightDecision, MachineIdentityDecision,
    StartupIdentityDecision, evaluate_local_identity_preflight, evaluate_startup_identity,
};
use serde::Deserialize;
use snafu::Snafu;
use uuid::Uuid;

use crate::identity_record::{self, IdentityRecordWriteError};

#[derive(Clone)]
pub(super) struct StartupPaths {
    site_config: PathBuf,
    identity_directory: PathBuf,
    keys_directory: PathBuf,
}

impl StartupPaths {
    #[must_use]
    pub(super) fn production() -> Self {
        Self {
            site_config: PathBuf::from("/etc/natsume/site.toml"),
            identity_directory: PathBuf::from("/var/lib/natsume/identity"),
            keys_directory: PathBuf::from("/var/lib/natsume/keys"),
        }
    }
}

#[derive(Debug, Snafu)]
pub(super) enum StartupError {
    #[snafu(display("device startup site identity configuration is missing or invalid"))]
    SiteConfiguration,

    #[snafu(display("device startup identity-bound artifact scan failed"))]
    ArtifactScan,

    #[snafu(display("device identity startup failed closed: {}", state_label(*state)))]
    FailClosed { state: StartupIdentityState },

    #[snafu(display("device startup could not persist its first identity record"))]
    IdentityPersistence { source: IdentityRecordWriteError },
}

#[derive(Deserialize)]
struct SiteIdentityConfig {
    fleet_namespace_uuid: String,
}

#[derive(Clone, Copy)]
struct StartupContext {
    configured_namespace: Uuid,
    stored_machine_hardware_id: Option<Uuid>,
}

fn state_label(state: StartupIdentityState) -> &'static str {
    match state {
        StartupIdentityState::CleanFirstStart => "clean_first_start",
        StartupIdentityState::Matched => "matched",
        StartupIdentityState::Indeterminate => "indeterminate",
        StartupIdentityState::IdentityUnavailable => "identity_unavailable",
        StartupIdentityState::IdentityRecordMissingOrCorrupt => {
            "identity_record_missing_or_corrupt"
        }
        StartupIdentityState::SiteNamespaceMismatch => "site_namespace_mismatch",
        StartupIdentityState::ResetRequired => "reset_required",
        StartupIdentityState::VaultCorrupt => "vault_corrupt",
        StartupIdentityState::EnrollmentPending => "enrollment_pending",
        StartupIdentityState::Enrolled => "enrolled",
    }
}

fn fail_closed(state: StartupIdentityState) -> StartupError {
    tracing::error!(
        startup_identity_state = state_label(state),
        "device identity startup failed closed"
    );
    StartupError::FailClosed { state }
}

fn canonical_uuid(value: &str) -> Option<Uuid> {
    Uuid::parse_str(value)
        .ok()
        .filter(|uuid| uuid.hyphenated().to_string() == value)
}

fn read_site_namespace(path: &Path) -> Result<Uuid, StartupError> {
    let text = fs::read_to_string(path).map_err(|_| {
        tracing::error!(
            startup_identity_state = "site_configuration_invalid",
            "device startup site identity configuration is unavailable"
        );
        StartupError::SiteConfiguration
    })?;
    let config = toml::from_str::<SiteIdentityConfig>(&text).map_err(|_| {
        tracing::error!(
            startup_identity_state = "site_configuration_invalid",
            "device startup site identity configuration is invalid"
        );
        StartupError::SiteConfiguration
    })?;
    canonical_uuid(&config.fleet_namespace_uuid).ok_or_else(|| {
        tracing::error!(
            startup_identity_state = "site_configuration_invalid",
            "device startup site namespace is not canonical"
        );
        StartupError::SiteConfiguration
    })
}

fn regular_file_below(directory: &Path) -> io::Result<bool> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_file() {
            return Ok(true);
        }
        if file_type.is_dir() && regular_file_below(&entry.path())? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn identity_bound_artifacts_present(keys_directory: &Path) -> Result<bool, StartupError> {
    // WP3 binds regular files below the keys directory. Future Client databases, LKGs, and
    // initialization journals extend this closed startup scan when their writers are introduced.
    regular_file_below(keys_directory).map_err(|_| {
        tracing::error!(
            startup_identity_state = "identity_bound_artifact_scan_failed",
            "device startup could not scan identity-bound artifacts"
        );
        StartupError::ArtifactScan
    })
}

fn preflight(paths: &StartupPaths) -> Result<StartupContext, StartupError> {
    let configured_namespace = read_site_namespace(&paths.site_config)?;
    let artifacts_present = identity_bound_artifacts_present(&paths.keys_directory)?;
    let record = identity_record::read(&paths.identity_directory);

    match evaluate_local_identity_preflight(configured_namespace, record, artifacts_present) {
        LocalIdentityPreflightDecision::CleanFirstStart => Ok(StartupContext {
            configured_namespace,
            stored_machine_hardware_id: None,
        }),
        LocalIdentityPreflightDecision::ReadyForHardwareCheck {
            stored_machine_hardware_id,
        } => Ok(StartupContext {
            configured_namespace,
            stored_machine_hardware_id: Some(stored_machine_hardware_id),
        }),
        LocalIdentityPreflightDecision::IdentityRecordMissingWithState
        | LocalIdentityPreflightDecision::IdentityRecordCorrupt => Err(fail_closed(
            StartupIdentityState::IdentityRecordMissingOrCorrupt,
        )),
        LocalIdentityPreflightDecision::SiteNamespaceMismatch { .. } => {
            Err(fail_closed(StartupIdentityState::SiteNamespaceMismatch))
        }
    }
}

fn decision_from_claim(claim: &SanitizedHardwareClaim) -> Option<MachineIdentityDecision> {
    let present_slot_count = usize::try_from(claim.present_slot_count).ok()?;
    let decision = match claim.decision.as_str() {
        "derived" if (2..=3).contains(&present_slot_count) => MachineIdentityDecision::Derived {
            machine_hardware_id: canonical_uuid(claim.machine_hardware_id.as_deref()?)?,
            present_slot_count,
        },
        "insufficient_sources"
            if present_slot_count <= 1 && claim.machine_hardware_id.is_none() =>
        {
            MachineIdentityDecision::InsufficientSources { present_slot_count }
        }
        "unsupported" if present_slot_count <= 3 && claim.machine_hardware_id.is_none() => {
            MachineIdentityDecision::Unsupported { present_slot_count }
        }
        _ => return None,
    };
    let expected_complete = decision.collection_completeness() == CollectionCompleteness::Complete;
    (claim.collection_complete == expected_complete).then_some(decision)
}

fn apply_claim(
    paths: &StartupPaths,
    context: StartupContext,
    claim: &SanitizedHardwareClaim,
) -> Result<StartupIdentityState, StartupError> {
    let Some(decision) = decision_from_claim(claim) else {
        return Err(fail_closed(StartupIdentityState::IdentityUnavailable));
    };
    match evaluate_startup_identity(context.stored_machine_hardware_id, &decision) {
        StartupIdentityDecision::FirstStart {
            machine_hardware_id,
        } => {
            identity_record::write_first_start(
                &paths.identity_directory,
                context.configured_namespace,
                machine_hardware_id,
            )
            .map_err(|source| {
                tracing::error!(
                    startup_identity_state =
                        state_label(StartupIdentityState::IdentityRecordMissingOrCorrupt),
                    "device startup could not persist the first identity record"
                );
                StartupError::IdentityPersistence { source }
            })?;
            tracing::info!(
                startup_identity_state = state_label(StartupIdentityState::CleanFirstStart),
                "device identity established on first start"
            );
            Ok(StartupIdentityState::CleanFirstStart)
        }
        StartupIdentityDecision::Matched => {
            tracing::info!(
                startup_identity_state = state_label(StartupIdentityState::Matched),
                "device identity matched"
            );
            Ok(StartupIdentityState::Matched)
        }
        StartupIdentityDecision::Indeterminate => {
            Err(fail_closed(StartupIdentityState::Indeterminate))
        }
        StartupIdentityDecision::IdentityUnavailable => {
            Err(fail_closed(StartupIdentityState::IdentityUnavailable))
        }
        StartupIdentityDecision::ResetRequired { .. } => {
            Err(fail_closed(StartupIdentityState::ResetRequired))
        }
    }
}

pub(super) async fn run_production(
    paths: &StartupPaths,
) -> Result<StartupIdentityState, StartupError> {
    let context = preflight(paths)?;
    let namespace = context.configured_namespace.to_string();
    let Ok(connection) = zbus::Connection::system().await else {
        return Err(fail_closed(StartupIdentityState::IdentityUnavailable));
    };
    let Ok(proxy) = Privileged1Proxy::new(&connection).await else {
        return Err(fail_closed(StartupIdentityState::IdentityUnavailable));
    };
    let Ok(claim) = proxy.collect_hardware_candidates(&namespace).await else {
        return Err(fail_closed(StartupIdentityState::IdentityUnavailable));
    };
    apply_claim(paths, context, &claim)
}

#[cfg(test)]
fn run_with_claim(
    paths: &StartupPaths,
    claim: &SanitizedHardwareClaim,
) -> Result<StartupIdentityState, StartupError> {
    let context = preflight(paths)?;
    apply_claim(paths, context, claim)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use natsume_machine_identity::IdentityRecordState;
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
            candidates: Vec::new(),
            collection_complete: true,
            decision: "derived".to_owned(),
            machine_hardware_id: Some(machine_hardware_id.to_string()),
            present_slot_count: 3,
        }
    }

    fn insufficient_claim() -> SanitizedHardwareClaim {
        SanitizedHardwareClaim {
            candidates: Vec::new(),
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
}
