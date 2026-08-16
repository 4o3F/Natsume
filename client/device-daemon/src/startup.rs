use std::{fs, io, path::Path, path::PathBuf};

use natsume_local_control_api::{
    HardwareCandidate, Privileged1Proxy, SanitizedHardwareClaim, StartupIdentityState,
};
use natsume_machine_identity::{
    CollectionCompleteness, EvidenceQuality, LocalIdentityPreflightDecision,
    MachineIdentityDecision, StartupIdentityDecision, evaluate_local_identity_preflight,
    evaluate_startup_identity,
};
use serde::Deserialize;
use snafu::Snafu;
use uuid::Uuid;

use crate::{
    enrollment::{self, EnrollmentError, EnrollmentPaths},
    identity_record,
};

#[derive(Clone)]
struct StartupPaths {
    site_config: PathBuf,
    identity_directory: PathBuf,
    keys_directory: PathBuf,
    enrollment: EnrollmentPaths,
}

impl StartupPaths {
    #[must_use]
    fn production() -> Self {
        Self {
            site_config: PathBuf::from("/etc/natsume/site.toml"),
            identity_directory: PathBuf::from("/var/lib/natsume/identity"),
            keys_directory: PathBuf::from("/var/lib/natsume/keys"),
            enrollment: EnrollmentPaths::production(),
        }
    }
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum StartupError {
    #[snafu(display("device startup site identity configuration is missing or invalid"))]
    SiteConfiguration,

    #[snafu(display("device startup identity-bound artifact scan failed"))]
    ArtifactScan,

    #[snafu(display("device identity startup failed closed: {}", state_label(*state)))]
    FailClosed { state: StartupIdentityState },

    #[snafu(display("device startup could not persist its first identity record"))]
    IdentityPersistence,

    #[snafu(display("device Enrollment startup failed closed"))]
    Enrollment { source: EnrollmentError },
}

#[derive(Deserialize)]
struct SiteIdentityConfig {
    fleet_namespace_uuid: String,
    gateway_hostname: String,
}

#[derive(Clone)]
struct StartupContext {
    configured_namespace: Uuid,
    stored_machine_hardware_id: Option<Uuid>,
    gateway_hostname: String,
}

struct SiteIdentity {
    fleet_namespace_uuid: Uuid,
    gateway_hostname: String,
}

struct IdentityReady {
    state: StartupIdentityState,
    machine_hardware_id: Uuid,
    hardware_identity_quality: EvidenceQuality,
    gateway_hostname: String,
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

fn read_site_identity(path: &Path) -> Result<SiteIdentity, StartupError> {
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
    let fleet_namespace_uuid = canonical_uuid(&config.fleet_namespace_uuid).ok_or_else(|| {
        tracing::error!(
            startup_identity_state = "site_configuration_invalid",
            "device startup site namespace is not canonical"
        );
        StartupError::SiteConfiguration
    })?;
    Ok(SiteIdentity {
        fleet_namespace_uuid,
        gateway_hostname: config.gateway_hostname,
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
    let site = read_site_identity(&paths.site_config)?;
    let configured_namespace = site.fleet_namespace_uuid;
    let gateway_hostname = site.gateway_hostname;
    let artifacts_present = identity_bound_artifacts_present(&paths.keys_directory)?;
    let record = identity_record::read(&paths.identity_directory);

    match evaluate_local_identity_preflight(configured_namespace, record, artifacts_present) {
        LocalIdentityPreflightDecision::CleanFirstStart => Ok(StartupContext {
            configured_namespace,
            stored_machine_hardware_id: None,
            gateway_hostname,
        }),
        LocalIdentityPreflightDecision::ReadyForHardwareCheck {
            stored_machine_hardware_id,
        } => Ok(StartupContext {
            configured_namespace,
            stored_machine_hardware_id: Some(stored_machine_hardware_id),
            gateway_hostname,
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

fn candidate_slot(candidate: &HardwareCandidate) -> Option<usize> {
    match candidate.anchor_kind.as_str() {
        "dmi_system_uuid" => Some(0),
        "dmi_board_serial" => Some(1),
        "first_disk_serial" => Some(2),
        _ => None,
    }
}

fn candidate_quality(candidate: &HardwareCandidate) -> Option<EvidenceQuality> {
    match candidate.quality.as_str() {
        "weak" => Some(EvidenceQuality::Weak),
        "medium" => Some(EvidenceQuality::Medium),
        "strong" => Some(EvidenceQuality::Strong),
        _ => None,
    }
}

fn whole_machine_quality(claim: &SanitizedHardwareClaim) -> Option<EvidenceQuality> {
    let present_slot_count = usize::try_from(claim.present_slot_count).ok()?;
    if claim.candidates.len() != present_slot_count {
        return None;
    }
    let mut seen = [false; 3];
    let mut minimum: Option<EvidenceQuality> = None;
    for candidate in &claim.candidates {
        let slot = candidate_slot(candidate)?;
        if seen[slot]
            || canonical_uuid(&candidate.candidate_id)
                .as_ref()
                .is_none_or(|candidate_id| candidate_id.get_version_num() != 5)
        {
            return None;
        }
        seen[slot] = true;
        let quality = candidate_quality(candidate)?;
        minimum = Some(minimum.map_or(quality, |current| current.min(quality)));
    }
    minimum
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
) -> Result<IdentityReady, StartupError> {
    let Some(decision) = decision_from_claim(claim) else {
        return Err(fail_closed(StartupIdentityState::IdentityUnavailable));
    };
    let machine_hardware_id = match decision {
        MachineIdentityDecision::Derived {
            machine_hardware_id,
            ..
        } => Some(machine_hardware_id),
        MachineIdentityDecision::InsufficientSources { .. }
        | MachineIdentityDecision::Unsupported { .. } => None,
    };
    let hardware_identity_quality = match decision {
        MachineIdentityDecision::Derived { .. } => Some(
            whole_machine_quality(claim)
                .ok_or_else(|| fail_closed(StartupIdentityState::IdentityUnavailable))?,
        ),
        MachineIdentityDecision::InsufficientSources { .. }
        | MachineIdentityDecision::Unsupported { .. } => None,
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
            .map_err(|_error| {
                tracing::error!(
                    startup_identity_state =
                        state_label(StartupIdentityState::IdentityRecordMissingOrCorrupt),
                    "device startup could not persist the first identity record"
                );
                StartupError::IdentityPersistence
            })?;
            tracing::info!(
                startup_identity_state = state_label(StartupIdentityState::CleanFirstStart),
                "device identity established on first start"
            );
            identity_ready(
                StartupIdentityState::CleanFirstStart,
                Some(machine_hardware_id),
                hardware_identity_quality,
                context.gateway_hostname,
            )
        }
        StartupIdentityDecision::Matched => {
            tracing::info!(
                startup_identity_state = state_label(StartupIdentityState::Matched),
                "device identity matched"
            );
            identity_ready(
                StartupIdentityState::Matched,
                machine_hardware_id,
                hardware_identity_quality,
                context.gateway_hostname,
            )
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

fn identity_ready(
    state: StartupIdentityState,
    machine_hardware_id: Option<Uuid>,
    hardware_identity_quality: Option<EvidenceQuality>,
    gateway_hostname: String,
) -> Result<IdentityReady, StartupError> {
    let machine_hardware_id = machine_hardware_id
        .ok_or_else(|| fail_closed(StartupIdentityState::IdentityUnavailable))?;
    let hardware_identity_quality = hardware_identity_quality
        .ok_or_else(|| fail_closed(StartupIdentityState::IdentityUnavailable))?;
    Ok(IdentityReady {
        state,
        machine_hardware_id,
        hardware_identity_quality,
        gateway_hostname,
    })
}

fn existing_enrollment_state(
    paths: &StartupPaths,
) -> Result<Option<StartupIdentityState>, StartupError> {
    if !enrollment::device_token_present(&paths.enrollment)
        .map_err(|source| StartupError::Enrollment { source })?
    {
        return Ok(None);
    }
    enrollment::validate_enrolled_artifacts(&paths.enrollment)
        .map_err(|source| StartupError::Enrollment { source })?;
    tracing::info!(
        startup_identity_state = state_label(StartupIdentityState::Enrolled),
        "device Enrollment artifacts are present"
    );
    Ok(Some(StartupIdentityState::Enrolled))
}

async fn run_with_paths(paths: &StartupPaths) -> Result<StartupIdentityState, StartupError> {
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
    let identity = apply_claim(paths, context, &claim)?;
    if let Some(state) = existing_enrollment_state(paths)? {
        return Ok(state);
    }
    tracing::info!(
        startup_identity_state = state_label(StartupIdentityState::EnrollmentPending),
        previous_startup_identity_state = state_label(identity.state),
        "device Enrollment is pending"
    );
    let client = enrollment::EnrollmentClient::prepare(
        paths.enrollment.clone(),
        identity.machine_hardware_id,
        identity.hardware_identity_quality,
        identity.gateway_hostname,
    )
    .map_err(|source| StartupError::Enrollment { source })?;
    enrollment::enroll_until_parked(&client)
        .await
        .map_err(|source| StartupError::Enrollment { source })?;
    tracing::info!(
        startup_identity_state = state_label(StartupIdentityState::Enrolled),
        "device Enrollment completed"
    );
    Ok(StartupIdentityState::Enrolled)
}

/// Runs identity-first production startup through Enrollment finalization.
///
/// # Errors
///
/// Returns a redacted fail-closed startup error.
pub async fn run_production() -> Result<StartupIdentityState, StartupError> {
    run_with_paths(&StartupPaths::production()).await
}

#[cfg(test)]
fn run_with_claim(
    paths: &StartupPaths,
    claim: &SanitizedHardwareClaim,
) -> Result<StartupIdentityState, StartupError> {
    let context = preflight(paths)?;
    apply_claim(paths, context, claim).map(|ready| ready.state)
}

#[cfg(test)]
mod tests {
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
}
