use std::{fs, io, path::Path, path::PathBuf};

use natsume_device_protocol::generated::EnrollmentEvidenceQuality;
use natsume_local_control_api::{
    DerivedMachineIdentity, MachineIdentityError, MachineIdentityQuality, Privileged1Proxy,
};
use serde::Deserialize;
use snafu::Snafu;
use tokio::time::{Duration, timeout};
use uuid::Uuid;

use crate::{
    atomic_write::ATOMIC_TEMP_PREFIX,
    canonical_uuid,
    control::{self, ControlIdentityError},
    identity_record::{self, IdentityRecordState},
    reconcile::SnapshotReconciler,
};

const LOCAL_CONTROL_TIMEOUT: Duration = Duration::from_secs(10);

struct StartupPaths {
    site_config: PathBuf,
    identity_directory: PathBuf,
    control_directory: PathBuf,
    keys_directory: PathBuf,
    state_directory: PathBuf,
}

impl StartupPaths {
    fn production() -> Self {
        Self {
            site_config: PathBuf::from("/etc/natsume/site.toml"),
            identity_directory: PathBuf::from("/var/lib/natsume/identity"),
            control_directory: PathBuf::from("/var/lib/natsume/control"),
            keys_directory: PathBuf::from("/var/lib/natsume/keys"),
            state_directory: PathBuf::from("/var/lib/natsume/state"),
        }
    }
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub(crate) enum StartupError {
    #[snafu(display("device startup site identity configuration is missing or invalid"))]
    SiteConfiguration,

    #[snafu(display("device startup identity-bound artifact scan failed"))]
    ArtifactScan,

    #[snafu(display("device identity startup failed closed: {state}"))]
    FailClosed { state: &'static str },

    #[snafu(display("device startup could not persist its first identity record"))]
    IdentityPersistence,

    #[snafu(display("device control identity startup failed closed: {source}"))]
    ControlIdentity { source: ControlIdentityError },

    #[snafu(display("device state reconciliation startup failed closed"))]
    Reconciliation,

    #[snafu(display("device control loop failed closed: {source}"))]
    Control { source: control::ControlLoopError },
}

#[derive(Deserialize)]
struct SiteIdentityConfig {
    fleet_namespace_uuid: String,
    gateway_hostname: String,
}

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
    hardware_identity_quality: MachineIdentityQuality,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartupIdentityState {
    CleanFirstStart,
    Matched,
    Indeterminate,
    IdentityUnavailable,
    IdentityRecordMissingOrCorrupt,
    SiteNamespaceMismatch,
    ResetRequired,
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
    }
}

fn fail_closed(state: StartupIdentityState) -> StartupError {
    tracing::error!(
        startup_identity_state = state_label(state),
        "device identity startup failed closed"
    );
    StartupError::FailClosed {
        state: state_label(state),
    }
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
    if !is_canonical_dns_hostname(&config.gateway_hostname) {
        tracing::error!(
            startup_identity_state = "site_configuration_invalid",
            "device startup gateway hostname is not canonical"
        );
        return Err(StartupError::SiteConfiguration);
    }
    Ok(SiteIdentity {
        fleet_namespace_uuid,
        gateway_hostname: config.gateway_hostname,
    })
}

fn is_canonical_dns_hostname(value: &str) -> bool {
    if value.is_empty()
        || value.len() > 253
        || value.ends_with('.')
        || value.parse::<std::net::IpAddr>().is_ok()
        || !value.bytes().any(|byte| byte.is_ascii_lowercase())
    {
        return false;
    }
    value.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && label
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            && label
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphanumeric)
            && label
                .as_bytes()
                .last()
                .is_some_and(u8::is_ascii_alphanumeric)
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
    state_directory: &Path,
) -> Result<bool, StartupError> {
    // Identity-bound regular files include Gateway material, reconciliation artifacts,
    // and the Device control identity.
    let keys_present = scan_identity_bound_directory(keys_directory)?;
    let control_present = scan_identity_bound_directory(control_directory)?;
    let state_present = scan_identity_bound_directory(state_directory)?;
    Ok(keys_present || control_present || state_present)
}

fn preflight(
    paths: &StartupPaths,
    privileged_home_state_present: bool,
) -> Result<StartupContext, StartupError> {
    let site = read_site_identity(&paths.site_config)?;
    let configured_namespace = site.fleet_namespace_uuid;
    let gateway_hostname = site.gateway_hostname;
    let artifacts_present = privileged_home_state_present
        || identity_bound_artifacts_present(
            &paths.keys_directory,
            &paths.control_directory,
            &paths.state_directory,
        )?;
    let record = identity_record::read(&paths.identity_directory);

    match record {
        IdentityRecordState::Absent if artifacts_present => Err(fail_closed(
            StartupIdentityState::IdentityRecordMissingOrCorrupt,
        )),
        IdentityRecordState::Absent => Ok(StartupContext {
            configured_namespace,
            stored_machine_hardware_id: None,
            gateway_hostname,
        }),
        IdentityRecordState::Corrupt => Err(fail_closed(
            StartupIdentityState::IdentityRecordMissingOrCorrupt,
        )),
        IdentityRecordState::Valid {
            fleet_namespace_uuid,
            ..
        } if fleet_namespace_uuid != configured_namespace => {
            Err(fail_closed(StartupIdentityState::SiteNamespaceMismatch))
        }
        IdentityRecordState::Valid {
            machine_hardware_id,
            ..
        } => Ok(StartupContext {
            configured_namespace,
            stored_machine_hardware_id: Some(machine_hardware_id),
            gateway_hostname,
        }),
    }
}

fn apply_identity_decision(
    paths: &StartupPaths,
    context: &StartupContext,
    decision: Result<DerivedMachineIdentity, MachineIdentityError>,
) -> Result<IdentityReady, StartupError> {
    let identity = match decision {
        Ok(identity) => identity,
        Err(MachineIdentityError::InsufficientSources(_))
            if context.stored_machine_hardware_id.is_some() =>
        {
            return Err(fail_closed(StartupIdentityState::Indeterminate));
        }
        Err(_) => return Err(fail_closed(StartupIdentityState::IdentityUnavailable)),
    };
    let machine_hardware_id = canonical_uuid(&identity.machine_hardware_id)
        .filter(|machine_hardware_id| machine_hardware_id.get_version_num() == 5)
        .ok_or_else(|| fail_closed(StartupIdentityState::IdentityUnavailable))?;
    let state = match context.stored_machine_hardware_id {
        None => {
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
            StartupIdentityState::CleanFirstStart
        }
        Some(stored) if stored == machine_hardware_id => {
            tracing::info!(
                startup_identity_state = state_label(StartupIdentityState::Matched),
                "device identity matched"
            );
            StartupIdentityState::Matched
        }
        Some(_) => return Err(fail_closed(StartupIdentityState::ResetRequired)),
    };
    Ok(IdentityReady {
        state,
        machine_hardware_id,
        hardware_identity_quality: identity.quality,
    })
}

fn load_control_identity(
    paths: &StartupPaths,
    expected: Uuid,
) -> Result<control::ControlIdentity, StartupError> {
    let machine_hardware_id = match identity_record::read(&paths.identity_directory) {
        IdentityRecordState::Valid {
            machine_hardware_id,
            ..
        } if machine_hardware_id == expected => Ok(machine_hardware_id),
        IdentityRecordState::Valid { .. } => Err(fail_closed(StartupIdentityState::ResetRequired)),
        IdentityRecordState::Absent | IdentityRecordState::Corrupt => Err(fail_closed(
            StartupIdentityState::IdentityRecordMissingOrCorrupt,
        )),
    }?;
    control::load_or_create_identity(&paths.control_directory, machine_hardware_id)
        .map_err(|source| StartupError::ControlIdentity { source })
}

/// Runs identity-first production startup and the single Device control loop.
///
/// # Errors
///
/// Returns a redacted fail-closed startup error.
pub(crate) async fn run_production() -> Result<(), StartupError> {
    let paths = StartupPaths::production();
    let Ok(builder) = zbus::connection::Builder::system() else {
        return Err(fail_closed(StartupIdentityState::IdentityUnavailable));
    };
    let Ok(Ok(connection)) = timeout(
        LOCAL_CONTROL_TIMEOUT,
        builder.method_timeout(LOCAL_CONTROL_TIMEOUT).build(),
    )
    .await
    else {
        return Err(fail_closed(StartupIdentityState::IdentityUnavailable));
    };
    let Ok(proxy) = Privileged1Proxy::new(&connection).await else {
        return Err(fail_closed(StartupIdentityState::IdentityUnavailable));
    };
    let Ok(privileged_home_state_present) = proxy.has_home_reset_state().await else {
        return Err(fail_closed(StartupIdentityState::IdentityUnavailable));
    };
    let context = preflight(&paths, privileged_home_state_present)?;
    let namespace = context.configured_namespace.to_string();
    let decision = proxy.derive_machine_identity(&namespace).await;
    let identity = apply_identity_decision(&paths, &context, decision)?;
    let gateway_hostname = context.gateway_hostname;
    let control_identity = load_control_identity(&paths, identity.machine_hardware_id)?;
    let IdentityReady {
        state,
        machine_hardware_id,
        hardware_identity_quality,
    } = identity;
    let evidence_quality = match hardware_identity_quality {
        MachineIdentityQuality::Medium => EnrollmentEvidenceQuality::Medium,
        MachineIdentityQuality::Strong => EnrollmentEvidenceQuality::Strong,
    };
    let snapshots = SnapshotReconciler::production(gateway_hostname, connection)
        .await
        .map_err(|_| StartupError::Reconciliation)?;
    tracing::info!(
        startup_identity_state = state_label(state),
        "starting Device control loop"
    );
    control::connection::run(
        control_identity,
        machine_hardware_id,
        evidence_quality,
        snapshots,
    )
    .await
    .map_err(|source| StartupError::Control { source })
}

#[cfg(test)]
mod tests;
