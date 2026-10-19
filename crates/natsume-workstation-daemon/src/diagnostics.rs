//! Best-effort evidence publication, independent of Agent and business readiness.

use std::{
    fs,
    os::unix::fs::MetadataExt as _,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use natsume_workstation_ipc::diagnostics::{
    ConnectionDiagnostic, DIAGNOSTIC_DIRECTORY, DIAGNOSTIC_FAILURE_NAME, DIAGNOSTIC_FORMAT_VERSION,
    DIAGNOSTIC_REPORT_NAME, DiagnosticCode, DiagnosticEvent, DiagnosticFailure, DiagnosticReport,
    DiagnosticStage, DiagnosticStatus, IdentityDiagnostic, MAX_DIAGNOSTIC_EVENTS,
    MAX_DIAGNOSTIC_REPORT_BYTES,
};
use natsume_workstation_protocol::CONTROL_SUBPROTOCOL;

use crate::atomic_write::{WritePolicy, atomic_write};

mod network;

pub(crate) struct Diagnostics {
    report: DiagnosticReport,
    failure_streak: Option<DiagnosticFailure>,
    directory: Option<PathBuf>,
}

fn boot_millis() -> Option<u64> {
    let uptime = fs::read_to_string("/proc/uptime").ok()?;
    let first = uptime.split_ascii_whitespace().next()?;
    let (seconds, fraction) = first.split_once('.')?;
    let seconds = seconds.parse::<u64>().ok()?.checked_mul(1_000)?;
    let fraction = fraction.get(..2)?.parse::<u64>().ok()?.checked_mul(10)?;
    seconds.checked_add(fraction)
}

fn unix_millis() -> Option<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
}

impl Diagnostics {
    pub(crate) fn production() -> Self {
        Self::new(Some(PathBuf::from(DIAGNOSTIC_DIRECTORY)))
    }

    fn new(directory: Option<PathBuf>) -> Self {
        let mut random = [0_u8; 16];
        let instance = if getrandom::fill(&mut random).is_ok() {
            hex::encode(random)
        } else {
            // Diagnostic entropy failure must not prevent safety-critical startup/exit.
            format!("{}-{}", std::process::id(), boot_millis().unwrap_or(0))
        };
        let boot_id = fs::read_to_string("/proc/sys/kernel/random/boot_id")
            .ok()
            .and_then(|value| crate::canonical_uuid(value.trim()))
            .map(|id| id.to_string());
        let mut diagnostics = Self {
            report: DiagnosticReport {
                format_version: DIAGNOSTIC_FORMAT_VERSION,
                boot_id,
                daemon_instance_id: instance,
                daemon_pid: std::process::id(),
                daemon_version: env!("CARGO_PKG_VERSION").to_owned(),
                control_protocol: CONTROL_SUBPROTOCOL.to_owned(),
                sequence: 0,
                captured_at_unix_ms: None,
                captured_at_boot_ms: None,
                stage: DiagnosticStage::Starting,
                last_completed_stage: None,
                status: DiagnosticStatus::Starting,
                failure: None,
                identity: IdentityDiagnostic::default(),
                connection: ConnectionDiagnostic::default(),
                events: Vec::new(),
                events_truncated: false,
            },
            failure_streak: None,
            directory,
        };
        diagnostics.set_stage(DiagnosticStage::Starting, DiagnosticStatus::Starting);
        diagnostics
    }

    #[cfg(test)]
    pub(crate) fn disabled() -> Self {
        Self::new(None)
    }

    pub(crate) fn report(&self) -> &DiagnosticReport {
        &self.report
    }

    pub(crate) fn stop(&mut self, code: DiagnosticCode) {
        if self
            .report
            .failure
            .as_ref()
            .is_some_and(|failure| failure.code == code)
        {
            self.report.status = DiagnosticStatus::Stopped;
            self.event(Some(code));
            self.publish(true);
        } else {
            self.failure(code, DiagnosticStatus::Stopped);
        }
    }

    pub(crate) fn set_stage(&mut self, stage: DiagnosticStage, status: DiagnosticStatus) {
        self.report.stage = stage;
        self.report.status = status;
        self.event(None);
        self.publish(false);
    }

    pub(crate) fn complete(&mut self, stage: DiagnosticStage) {
        self.report.last_completed_stage = Some(stage);
        self.publish(false);
    }

    /// Only fixed non-secret report fields may be changed here. No error text is accepted.
    pub(crate) fn update(&mut self, change: impl FnOnce(&mut DiagnosticReport)) {
        change(&mut self.report);
        self.publish(false);
    }

    pub(crate) fn failure(&mut self, code: DiagnosticCode, status: DiagnosticStatus) {
        let now = boot_millis();
        let failure = match self.failure_streak.take() {
            Some(mut failure) if failure.code == code => {
                failure.last_at_boot_ms = now;
                failure.consecutive_count = failure.consecutive_count.saturating_add(1);
                failure
            }
            _ => DiagnosticFailure {
                code,
                first_at_boot_ms: now,
                last_at_boot_ms: now,
                consecutive_count: 1,
            },
        };
        self.failure_streak = Some(failure.clone());
        self.report.failure = Some(failure);
        self.report.status = status;
        self.event(Some(code));
        self.publish(true);
    }

    pub(crate) fn sample_network(&mut self, endpoint: std::net::IpAddr) {
        let mut evidence = network::sample(Path::new("/"), endpoint);
        evidence.sampled_at_boot_ms = boot_millis();
        self.update(|report| report.connection.network = evidence);
    }

    /// A successful transport or normal pending review clears the displayed
    /// failure, but not accounting for repeated unsuccessful control attempts.
    pub(crate) fn clear_failure(&mut self) {
        self.report.failure = None;
        self.publish(false);
    }

    pub(crate) fn reset_failures(&mut self) {
        self.failure_streak = None;
        self.clear_failure();
    }

    fn event(&mut self, code: Option<DiagnosticCode>) {
        if self.report.events.len() == MAX_DIAGNOSTIC_EVENTS {
            self.report.events.remove(0);
            self.report.events_truncated = true;
        }
        self.report.events.push(DiagnosticEvent {
            at_boot_ms: boot_millis(),
            stage: self.report.stage,
            status: self.report.status,
            code,
        });
    }

    fn publish(&mut self, retain_failure: bool) {
        self.report.sequence = self.report.sequence.saturating_add(1);
        self.report.captured_at_unix_ms = unix_millis();
        self.report.captured_at_boot_ms = boot_millis();
        let Some(directory) = self.directory.as_ref() else {
            return;
        };
        if self.write(directory, retain_failure).is_err() {
            tracing::warn!(
                "Diagnostic evidence publication failed; business behavior is unchanged"
            );
        }
    }

    fn write(&self, directory: &Path, retain_failure: bool) -> Result<(), ()> {
        // The dedicated tmpfiles-owned directory is outside all identity/artifact scans.
        // Do not recreate it or follow an unexpected replacement of the directory.
        let metadata = fs::symlink_metadata(directory).map_err(|_| ())?;
        if !metadata.is_dir()
            || metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.mode() & 0o777 != 0o755
        {
            return Err(());
        }
        let encoded = serde_json::to_vec(&self.report).map_err(|_| ())?;
        if encoded.len() > MAX_DIAGNOSTIC_REPORT_BYTES {
            return Err(());
        }
        // Preserve failure evidence separately: a new instance's Starting report must
        // not erase the previous instance's last failure before Agent can read it.
        let failure = if retain_failure && self.report.failure.is_some() {
            atomic_write(
                &directory.join(DIAGNOSTIC_FAILURE_NAME),
                &encoded,
                0o644,
                WritePolicy::Replace,
            )
            .map_err(|_| ())
        } else {
            Ok(())
        };
        let latest = atomic_write(
            &directory.join(DIAGNOSTIC_REPORT_NAME),
            &encoded,
            0o644,
            WritePolicy::Replace,
        )
        .map_err(|_| ());
        failure.and(latest)
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use super::*;

    fn fixture() -> tempfile::TempDir {
        use std::os::unix::fs::PermissionsExt as _;
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("fixture: {error}"));
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o755))
            .unwrap_or_else(|error| panic!("permissions: {error}"));
        directory
    }

    fn read(directory: &Path, name: &str) -> DiagnosticReport {
        let bytes = fs::read(directory.join(name))
            .unwrap_or_else(|error| panic!("report must be readable: {error}"));
        assert!(bytes.len() <= MAX_DIAGNOSTIC_REPORT_BYTES);
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|error| panic!("complete report must decode: {error}"))
    }

    #[test]
    fn final_failure_survives_daemon_restart_and_success() {
        let directory = fixture();
        let mut first = Diagnostics::new(Some(directory.path().to_owned()));
        first.failure(
            DiagnosticCode::IdentityInsufficientSources,
            DiagnosticStatus::Stopped,
        );
        let failure = read(directory.path(), DIAGNOSTIC_FAILURE_NAME);
        let mut second = Diagnostics::new(Some(directory.path().to_owned()));
        second.set_stage(DiagnosticStage::Active, DiagnosticStatus::Active);
        assert_eq!(read(directory.path(), DIAGNOSTIC_FAILURE_NAME), failure);
        assert_ne!(
            read(directory.path(), DIAGNOSTIC_REPORT_NAME).daemon_instance_id,
            failure.daemon_instance_id
        );
        assert_eq!(
            read(directory.path(), DIAGNOSTIC_REPORT_NAME).status,
            DiagnosticStatus::Active
        );
    }

    #[test]
    fn reports_are_bounded_complete_readable_and_without_temp_files() {
        let directory = fixture();
        let mut diagnostics = Diagnostics::new(Some(directory.path().to_owned()));
        for _ in 0..100 {
            diagnostics.failure(
                DiagnosticCode::TcpConnectionRefused,
                DiagnosticStatus::Retrying,
            );
        }
        let report = read(directory.path(), DIAGNOSTIC_REPORT_NAME);
        assert_eq!(report.events.len(), MAX_DIAGNOSTIC_EVENTS);
        assert!(report.events_truncated);
        assert_eq!(
            report.failure.map(|failure| failure.consecutive_count),
            Some(100)
        );
        let metadata = fs::metadata(directory.path().join(DIAGNOSTIC_REPORT_NAME))
            .unwrap_or_else(|error| panic!("metadata: {error}"));
        assert_eq!(metadata.mode() & 0o777, 0o644);
        assert_eq!(
            fs::read_dir(directory.path())
                .unwrap_or_else(|error| panic!("entries: {error}"))
                .count(),
            2
        );
    }

    #[test]
    fn permission_drift_is_rejected_without_changing_permissions() {
        use std::os::unix::fs::PermissionsExt as _;
        for mode in [0o700, 0o750, 0o777] {
            let directory = fixture();
            fs::set_permissions(directory.path(), fs::Permissions::from_mode(mode))
                .unwrap_or_else(|error| panic!("permissions: {error}"));
            let mut diagnostics = Diagnostics::new(Some(directory.path().to_owned()));
            diagnostics.failure(
                DiagnosticCode::IdentityUnavailable,
                DiagnosticStatus::Stopped,
            );
            assert!(!directory.path().join(DIAGNOSTIC_REPORT_NAME).exists());
            assert_eq!(
                fs::metadata(directory.path())
                    .unwrap_or_else(|error| panic!("metadata: {error}"))
                    .mode()
                    & 0o777,
                mode
            );
        }
    }

    #[test]
    fn stage_updates_do_not_rewrite_the_context_of_retained_failure() {
        let directory = fixture();
        let mut diagnostics = Diagnostics::new(Some(directory.path().to_owned()));
        diagnostics.set_stage(DiagnosticStage::Connecting, DiagnosticStatus::Retrying);
        diagnostics.failure(
            DiagnosticCode::TcpConnectionRefused,
            DiagnosticStatus::Retrying,
        );
        let failure = read(directory.path(), DIAGNOSTIC_FAILURE_NAME);
        diagnostics.set_stage(DiagnosticStage::Handshake, DiagnosticStatus::Retrying);
        assert_eq!(read(directory.path(), DIAGNOSTIC_FAILURE_NAME), failure);
    }

    #[test]
    fn missing_directory_or_symlink_does_not_change_diagnostic_state() {
        let directory = fixture();
        let missing = directory.path().join("missing");
        let mut diagnostics = Diagnostics::new(Some(missing.clone()));
        diagnostics.failure(DiagnosticCode::IdentityMismatch, DiagnosticStatus::Stopped);
        assert!(!missing.exists());
        assert_eq!(diagnostics.report.status, DiagnosticStatus::Stopped);
        let link = directory.path().join("link");
        symlink(directory.path(), &link).unwrap_or_else(|error| panic!("link: {error}"));
        let mut diagnostics = Diagnostics::new(Some(link));
        diagnostics.failure(DiagnosticCode::IdentityMismatch, DiagnosticStatus::Stopped);
        assert!(!directory.path().join(DIAGNOSTIC_REPORT_NAME).exists());
    }
}
