use std::{
    fs,
    path::{Path, PathBuf},
};

use natsume_device_daemon::{
    control::{ControlClient, ControlPaths},
    enrollment::{EnrollmentClient, EnrollmentStep},
};
use uuid::Uuid;
use zeroize::Zeroizing;

use super::require_ok;

pub struct ClientFixture {
    pub(super) enrollment: EnrollmentClient,
    pub(super) machine_hardware_id: Uuid,
    pub(super) client_config: PathBuf,
    pub(super) control_root: PathBuf,
    pub(super) keys_directory: PathBuf,
    pub(super) journal_directory: PathBuf,
}

impl ClientFixture {
    /// Completes the fixture's one expected Enrollment step.
    ///
    /// # Panics
    ///
    /// Panics when Enrollment fails or does not issue credentials.
    pub async fn enroll(&self) {
        let step = require_ok(
            self.enrollment.step().await,
            "Enrollment step must complete",
        );
        assert_eq!(step, EnrollmentStep::Enrolled);
    }

    #[must_use]
    pub const fn enrollment(&self) -> &EnrollmentClient {
        &self.enrollment
    }

    #[must_use]
    pub fn control(&self) -> ControlClient {
        require_ok(
            ControlClient::prepare(
                ControlPaths::new(
                    self.client_config.clone(),
                    self.control_root.clone(),
                    self.keys_directory.join("device-token"),
                    self.journal_directory.clone(),
                ),
                self.machine_hardware_id,
            ),
            "Device control client must prepare",
        )
    }

    #[must_use]
    pub fn token(&self) -> Zeroizing<String> {
        Zeroizing::new(require_ok(
            fs::read_to_string(self.keys_directory.join("device-token")),
            "issued Device Token must be readable",
        ))
    }

    #[must_use]
    pub const fn machine_hardware_id(&self) -> Uuid {
        self.machine_hardware_id
    }

    #[must_use]
    pub fn journal_directory(&self) -> &Path {
        &self.journal_directory
    }

    #[must_use]
    pub fn journal_frame(&self, command_id: Uuid) -> PathBuf {
        self.journal_directory.join(format!("{command_id}.frame"))
    }

    #[must_use]
    pub fn credential_snapshot(&self) -> Vec<(PathBuf, Vec<u8>)> {
        [
            "gateway-key.pk8",
            "gateway-leaf.der",
            "gateway-chain.der",
            "device-token",
        ]
        .into_iter()
        .map(|name| {
            let path = self.keys_directory.join(name);
            let bytes = require_ok(fs::read(&path), "credential artifact must be readable");
            (path, bytes)
        })
        .collect()
    }
}
