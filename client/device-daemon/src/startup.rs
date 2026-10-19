use std::{fs, io, path::Path, path::PathBuf};

use natsume_device_protocol::generated::EnrollmentEvidenceQuality;
use natsume_local_control_api::{
    DerivedMachineIdentity, HardwareSourceStatus, MachineIdentityError, MachineIdentityOutcome,
    MachineIdentityQuality, MachineIdentityReport, Privileged1Proxy, ResourceControlError,
    diagnostics::{
        DiagnosticCode, DiagnosticStage, DiagnosticStatus, IdentityComparisonDiagnostic,
        IdentityRecordDiagnostic,
    },
    is_canonical_gateway_hostname,
};
use serde::Deserialize;
use snafu::Snafu;
use tokio::time::{Duration, timeout};
use uuid::Uuid;

use crate::{
    CanonicalEndpoint,
    atomic_write::ATOMIC_TEMP_PREFIX,
    canonical_uuid,
    control::{self, ControlIdentityError, ControlLoopError},
    diagnostics::Diagnostics,
    identity_record::{self, IdentityRecordState},
    reconcile::SnapshotReconciler,
};

const LOCAL_CONTROL_TIMEOUT: Duration = Duration::from_secs(10);

struct StartupPaths {
    config: PathBuf,
    identity_directory: PathBuf,
    control_directory: PathBuf,
    keys_directory: PathBuf,
    state_directory: PathBuf,
}

impl StartupPaths {
    fn production() -> Self {
        Self {
            config: PathBuf::from("/etc/natsume/config.toml"),
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
    #[snafu(display("device startup site configuration is missing or invalid"))]
    SiteConfiguration,

    #[snafu(display("local Gateway configuration failed: {source}"))]
    LocalGateway { source: ResourceControlError },

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
struct StartupConfig {
    site: SiteConfig,
}

#[derive(Deserialize)]
struct DiagnosticEndpointConfig {
    server: CanonicalEndpoint,
}

#[derive(Deserialize)]
struct SiteConfig {
    gateway_hostname: String,
}

struct StartupContext {
    stored_machine_hardware_id: Option<Uuid>,
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

fn read_site_config(
    path: &Path,
    diagnostics: &mut Diagnostics,
) -> Result<SiteConfig, StartupError> {
    let text = fs::read_to_string(path).map_err(|_| {
        tracing::error!(
            startup_identity_state = "site_configuration_invalid",
            "device startup site configuration is unavailable"
        );
        StartupError::SiteConfiguration
    })?;
    let config = toml::from_str::<StartupConfig>(&text).map_err(|_| {
        tracing::error!(
            startup_identity_state = "site_configuration_invalid",
            "device startup site configuration is invalid"
        );
        StartupError::SiteConfiguration
    })?;
    // Read only the typed, non-secret endpoint from the already-read input. Its
    // failure must not move control-config validation ahead of the identity gate.
    if let Ok(endpoint) = toml::from_str::<DiagnosticEndpointConfig>(&text) {
        diagnostics.update(|report| {
            report.connection.endpoint_ip = Some(endpoint.server.ip.to_string());
            report.connection.endpoint_port = Some(endpoint.server.port.get());
        });
        diagnostics.sample_network(endpoint.server.ip);
    }
    let config = config.site;
    if !is_canonical_gateway_hostname(&config.gateway_hostname) {
        tracing::error!(
            startup_identity_state = "site_configuration_invalid",
            "device startup gateway hostname is not canonical"
        );
        return Err(StartupError::SiteConfiguration);
    }
    Ok(config)
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
    diagnostics: &mut Diagnostics,
) -> Result<StartupContext, StartupError> {
    let site = read_site_config(&paths.config, diagnostics)?;
    let gateway_hostname = site.gateway_hostname;
    let artifacts_present = privileged_home_state_present
        || identity_bound_artifacts_present(
            &paths.keys_directory,
            &paths.control_directory,
            &paths.state_directory,
        )?;
    let record = identity_record::read(&paths.identity_directory);
    diagnostics.update(|report| {
        report.identity.record = match record {
            IdentityRecordState::Absent => IdentityRecordDiagnostic::Absent,
            IdentityRecordState::Corrupt => IdentityRecordDiagnostic::Corrupt,
            IdentityRecordState::Valid { .. } => IdentityRecordDiagnostic::Valid,
        };
    });

    match record {
        IdentityRecordState::Absent if artifacts_present => Err(fail_closed(
            StartupIdentityState::IdentityRecordMissingOrCorrupt,
        )),
        IdentityRecordState::Absent => Ok(StartupContext {
            stored_machine_hardware_id: None,
            gateway_hostname,
        }),
        IdentityRecordState::Corrupt => Err(fail_closed(
            StartupIdentityState::IdentityRecordMissingOrCorrupt,
        )),
        IdentityRecordState::Valid {
            machine_hardware_id,
        } => Ok(StartupContext {
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
            identity_record::write_first_start(&paths.identity_directory, machine_hardware_id)
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
        } if machine_hardware_id == expected => Ok(machine_hardware_id),
        IdentityRecordState::Valid { .. } => Err(fail_closed(StartupIdentityState::ResetRequired)),
        IdentityRecordState::Absent | IdentityRecordState::Corrupt => Err(fail_closed(
            StartupIdentityState::IdentityRecordMissingOrCorrupt,
        )),
    }?;
    control::load_or_create_identity(&paths.control_directory, machine_hardware_id)
        .map_err(|source| StartupError::ControlIdentity { source })
}

fn identity_decision(
    report: MachineIdentityReport,
    diagnostics: &mut Diagnostics,
) -> Result<DerivedMachineIdentity, MachineIdentityError> {
    diagnostics.update(|evidence| {
        let count = [
            report.sources.dmi_system_uuid,
            report.sources.dmi_board_serial,
            report.sources.first_disk_serial,
        ]
        .into_iter()
        .filter(|status| *status == HardwareSourceStatus::Present)
        .count();
        evidence.identity.valid_source_count = u8::try_from(count).ok();
        evidence.identity.sources = Some(report.sources.clone());
    });
    match (report.outcome, report.identity) {
        (MachineIdentityOutcome::Derived, Some(identity)) => {
            // Only a canonical derived ID may enter non-secret evidence. Invalid
            // Helper strings are not copied to reports or error source chains.
            diagnostics.update(|evidence| {
                evidence.identity.machine_hardware_id =
                    canonical_uuid(&identity.machine_hardware_id)
                        .filter(|id| id.get_version_num() == 5)
                        .map(|id| id.to_string());
                evidence.identity.quality = evidence
                    .identity
                    .machine_hardware_id
                    .as_ref()
                    .map(|_| identity.quality);
            });
            Ok(identity)
        }
        (MachineIdentityOutcome::InsufficientSources, None) => {
            diagnostics.failure(
                DiagnosticCode::IdentityInsufficientSources,
                DiagnosticStatus::Starting,
            );
            Err(MachineIdentityError::InsufficientSources(String::new()))
        }
        (MachineIdentityOutcome::Unsupported, None) => {
            diagnostics.failure(
                DiagnosticCode::IdentityUnsupported,
                DiagnosticStatus::Starting,
            );
            Err(MachineIdentityError::Unsupported(String::new()))
        }
        _ => {
            diagnostics.failure(
                DiagnosticCode::IdentityUnavailable,
                DiagnosticStatus::Starting,
            );
            Err(MachineIdentityError::ZBus(zbus::Error::Failure(
                "invalid identity report".to_owned(),
            )))
        }
    }
}

fn control_identity_diagnostic_code(error: &ControlIdentityError) -> DiagnosticCode {
    match error {
        ControlIdentityError::ControlKey => DiagnosticCode::ControlKeyInvalid,
        ControlIdentityError::ControlKeyEntropy => DiagnosticCode::ControlKeyEntropyUnavailable,
        ControlIdentityError::ControlKeyEncoding => DiagnosticCode::ControlKeyEncodingFailed,
        ControlIdentityError::ControlKeyPersistence => DiagnosticCode::ControlKeyPersistenceFailed,
        ControlIdentityError::Manifest => DiagnosticCode::ControlManifestInvalid,
        ControlIdentityError::ManifestSerialization => {
            DiagnosticCode::ControlManifestSerializationFailed
        }
        ControlIdentityError::ManifestPersistence => {
            DiagnosticCode::ControlManifestPersistenceFailed
        }
    }
}

fn startup_diagnostic_code(error: &StartupError, diagnostics: &mut Diagnostics) -> DiagnosticCode {
    match error {
        StartupError::SiteConfiguration => DiagnosticCode::SiteConfigurationInvalid,
        StartupError::LocalGateway { .. } => DiagnosticCode::LocalGatewayFailed,
        StartupError::ArtifactScan => DiagnosticCode::IdentityArtifactScanFailed,
        StartupError::IdentityPersistence => DiagnosticCode::IdentityPersistenceFailed,
        StartupError::ControlIdentity { source } => control_identity_diagnostic_code(source),
        StartupError::Reconciliation => DiagnosticCode::ReconciliationFailed,
        StartupError::FailClosed { state } => match *state {
            "identity_record_missing_or_corrupt" => DiagnosticCode::IdentityRecordMissingOrCorrupt,
            "reset_required" => {
                diagnostics.update(|report| {
                    report.identity.comparison = IdentityComparisonDiagnostic::Mismatched;
                });
                DiagnosticCode::IdentityMismatch
            }
            "indeterminate" => {
                diagnostics.update(|report| {
                    report.identity.comparison = IdentityComparisonDiagnostic::Indeterminate;
                });
                DiagnosticCode::IdentityInsufficientSources
            }
            _ => match diagnostics.report().stage {
                DiagnosticStage::SystemBus => DiagnosticCode::SystemBusUnavailable,
                DiagnosticStage::Helper => DiagnosticCode::HelperUnavailable,
                _ => diagnostics
                    .report()
                    .failure
                    .as_ref()
                    .map_or(DiagnosticCode::IdentityUnavailable, |failure| failure.code),
            },
        },
        StartupError::Control { source } => match source {
            ControlLoopError::PresentationCache => DiagnosticCode::PresentationCacheFailed,
            ControlLoopError::EndpointConfiguration => DiagnosticCode::EndpointConfigurationInvalid,
            ControlLoopError::TrustRootConfiguration => DiagnosticCode::TrustRootInvalid,
            ControlLoopError::Tls => DiagnosticCode::TlsConfigurationFailed,
            ControlLoopError::ReconnectEntropy => DiagnosticCode::ReconnectEntropyUnavailable,
            ControlLoopError::LocalDeactivation => DiagnosticCode::LocalDeactivationFailed,
            ControlLoopError::AuthorityPersistence { .. } => {
                DiagnosticCode::AuthorityPersistenceFailed
            }
        },
    }
}

/// Runs identity-first production startup and the single Device control loop.
///
/// # Errors
///
/// Returns a redacted fail-closed startup error.
pub(crate) async fn run_production() -> Result<(), StartupError> {
    let mut diagnostics = Diagnostics::production();
    let result = run_with_diagnostics(&mut diagnostics).await;
    if let Err(error) = &result {
        record_final_failure(error, &mut diagnostics);
    }
    result
}

fn record_final_failure(error: &StartupError, diagnostics: &mut Diagnostics) {
    if matches!(
        error,
        StartupError::Control {
            source: ControlLoopError::LocalDeactivation
        }
    ) {
        // systemd's ExecStopPost hard-kill is now the safety barrier. Do not
        // put diagnostic filesystem I/O between failed BLOCKED and process exit.
        tracing::error!(
            code = "LOCAL_DEACTIVATION_FAILED",
            "Immediate safety exit; diagnostic file publication skipped"
        );
        return;
    }
    let code = startup_diagnostic_code(error, diagnostics);
    diagnostics.stop(code);
}

async fn run_with_diagnostics(diagnostics: &mut Diagnostics) -> Result<(), StartupError> {
    let paths = StartupPaths::production();
    diagnostics.set_stage(DiagnosticStage::SystemBus, DiagnosticStatus::Starting);
    let builder = zbus::connection::Builder::system();
    let Ok(Ok(connection)) = timeout(
        LOCAL_CONTROL_TIMEOUT,
        builder.method_timeout(LOCAL_CONTROL_TIMEOUT).build(),
    )
    .await
    else {
        return Err(fail_closed(StartupIdentityState::IdentityUnavailable));
    };
    diagnostics.complete(DiagnosticStage::SystemBus);
    diagnostics.set_stage(DiagnosticStage::Helper, DiagnosticStatus::Starting);
    let Ok(proxy) = Privileged1Proxy::new(&connection).await else {
        return Err(fail_closed(StartupIdentityState::IdentityUnavailable));
    };
    let Ok(privileged_home_state_present) = proxy.has_home_reset_state().await else {
        return Err(fail_closed(StartupIdentityState::IdentityUnavailable));
    };
    diagnostics.complete(DiagnosticStage::Helper);
    diagnostics.set_stage(
        DiagnosticStage::IdentityPreflight,
        DiagnosticStatus::Starting,
    );
    let context = preflight(&paths, privileged_home_state_present, diagnostics)?;
    diagnostics.complete(DiagnosticStage::IdentityPreflight);
    diagnostics.set_stage(
        DiagnosticStage::HardwareIdentity,
        DiagnosticStatus::Starting,
    );
    let decision = proxy.derive_machine_identity().await;
    let decision = match decision {
        Ok(report) => identity_decision(report, diagnostics),
        Err(error) => Err(error),
    };
    let identity = apply_identity_decision(&paths, &context, decision)?;
    diagnostics.update(|report| {
        report.identity.comparison = match identity.state {
            StartupIdentityState::CleanFirstStart => IdentityComparisonDiagnostic::CleanFirstStart,
            StartupIdentityState::Matched => IdentityComparisonDiagnostic::Matched,
            _ => IdentityComparisonDiagnostic::NotChecked,
        };
    });
    diagnostics.complete(DiagnosticStage::HardwareIdentity);
    let gateway_hostname = context.gateway_hostname;
    diagnostics.set_stage(DiagnosticStage::LocalGateway, DiagnosticStatus::Starting);
    proxy
        .configure_local_gateway(&gateway_hostname)
        .await
        .map_err(|source| StartupError::LocalGateway { source })?;
    diagnostics.complete(DiagnosticStage::LocalGateway);
    diagnostics.set_stage(DiagnosticStage::ControlIdentity, DiagnosticStatus::Starting);
    let control_identity = load_control_identity(&paths, identity.machine_hardware_id)?;
    diagnostics.complete(DiagnosticStage::ControlIdentity);
    let IdentityReady {
        state,
        machine_hardware_id,
        hardware_identity_quality,
    } = identity;
    let evidence_quality = match hardware_identity_quality {
        MachineIdentityQuality::Medium => EnrollmentEvidenceQuality::Medium,
        MachineIdentityQuality::Strong => EnrollmentEvidenceQuality::Strong,
    };
    diagnostics.set_stage(DiagnosticStage::Reconciliation, DiagnosticStatus::Starting);
    let snapshots = SnapshotReconciler::production(gateway_hostname, connection)
        .await
        .map_err(|_| StartupError::Reconciliation)?;
    diagnostics.complete(DiagnosticStage::Reconciliation);
    tracing::info!(
        startup_identity_state = state_label(state),
        "starting Device control loop"
    );
    control::connection::run(
        control_identity,
        machine_hardware_id,
        evidence_quality,
        snapshots,
        diagnostics,
    )
    .await
    .map_err(|source| StartupError::Control { source })
}

#[cfg(test)]
mod tests;
