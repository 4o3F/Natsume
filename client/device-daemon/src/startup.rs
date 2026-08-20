use std::{fs, io, path::Path, path::PathBuf};

use natsume_local_control_api::{
    HardwareCandidate, Privileged1Proxy, SanitizedHardwareClaim, StartupIdentityState,
};
use natsume_machine_identity::{
    CollectionCompleteness, EvidenceQuality, IdentityRecordState, LocalIdentityPreflightDecision,
    MachineIdentityDecision, StartupIdentityDecision, evaluate_local_identity_preflight,
    evaluate_startup_identity,
};
use serde::Deserialize;
use snafu::Snafu;
use uuid::Uuid;

use crate::{
    atomic_write::ATOMIC_TEMP_PREFIX,
    canonical_uuid,
    client_configuration::KEYS_DIRECTORY_PATH,
    control::{self, DormantControlIdentityError},
    identity_record,
};

#[derive(Clone)]
struct StartupPaths {
    site_config: PathBuf,
    identity_directory: PathBuf,
    control_directory: PathBuf,
    keys_directory: PathBuf,
}

impl StartupPaths {
    #[must_use]
    fn production() -> Self {
        Self {
            site_config: PathBuf::from("/etc/natsume/site.toml"),
            identity_directory: PathBuf::from("/var/lib/natsume/identity"),
            control_directory: PathBuf::from("/var/lib/natsume/control"),
            keys_directory: PathBuf::from(KEYS_DIRECTORY_PATH),
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

    #[snafu(display("device dormant control identity startup failed closed"))]
    DormantControlIdentity { source: DormantControlIdentityError },
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
    #[allow(dead_code)]
    state: StartupIdentityState,
    machine_hardware_id: Uuid,
    #[allow(dead_code)]
    hardware_identity_quality: EvidenceQuality,
    #[allow(dead_code)]
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

/// Ignores an orphaned atomic-write temporary because it was never renamed into place and never
/// became a durable identity-bound artifact; counting it would fail-close a clean first start
/// after `SIGKILL`.
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
            if entry
                .file_name()
                .as_encoded_bytes()
                .starts_with(ATOMIC_TEMP_PREFIX.as_bytes())
            {
                continue;
            }
            return Ok(true);
        }
        if file_type.is_dir() && regular_file_below(&entry.path())? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn scan_identity_bound_directory(directory: &Path) -> Result<bool, StartupError> {
    regular_file_below(directory).map_err(|_| {
        tracing::error!(
            startup_identity_state = "identity_bound_artifact_scan_failed",
            "device startup could not scan identity-bound artifacts"
        );
        StartupError::ArtifactScan
    })
}

fn identity_bound_artifacts_present(
    keys_directory: &Path,
    control_directory: &Path,
) -> Result<bool, StartupError> {
    // Identity-bound regular files include the current Token/Gateway keys and the dormant
    // Device control identity. Future stores extend this closed scan with their first writer.
    let keys_present = scan_identity_bound_directory(keys_directory)?;
    let control_present = scan_identity_bound_directory(control_directory)?;
    Ok(keys_present || control_present)
}

fn preflight(paths: &StartupPaths) -> Result<StartupContext, StartupError> {
    let site = read_site_identity(&paths.site_config)?;
    let configured_namespace = site.fleet_namespace_uuid;
    let gateway_hostname = site.gateway_hostname;
    let artifacts_present =
        identity_bound_artifacts_present(&paths.keys_directory, &paths.control_directory)?;
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

fn persisted_machine_hardware_id(
    paths: &StartupPaths,
    expected: Uuid,
) -> Result<Uuid, StartupError> {
    match identity_record::read(&paths.identity_directory) {
        IdentityRecordState::Valid {
            machine_hardware_id,
            ..
        } if machine_hardware_id == expected => Ok(machine_hardware_id),
        IdentityRecordState::Valid { .. } => Err(fail_closed(StartupIdentityState::ResetRequired)),
        IdentityRecordState::Absent | IdentityRecordState::Corrupt => Err(fail_closed(
            StartupIdentityState::IdentityRecordMissingOrCorrupt,
        )),
    }
}

fn ensure_dormant_control_identity(
    paths: &StartupPaths,
    expected: Uuid,
) -> Result<Uuid, StartupError> {
    let machine_hardware_id = persisted_machine_hardware_id(paths, expected)?;
    control::ensure_dormant_identity(&paths.control_directory, machine_hardware_id)
        .map_err(|source| StartupError::DormantControlIdentity { source })?;
    Ok(machine_hardware_id)
}

async fn run_with_paths(paths: &StartupPaths) -> Result<(), StartupError> {
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
    ensure_dormant_control_identity(paths, identity.machine_hardware_id)?;
    Ok(())
}

/// Runs identity-first production startup and establishes the dormant control key.
///
/// # Errors
///
/// Returns a redacted fail-closed startup error.
pub async fn run_production() -> Result<(), StartupError> {
    run_with_paths(&StartupPaths::production()).await?;
    Ok(())
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
mod tests;
