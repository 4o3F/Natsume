#![forbid(unsafe_code)]

mod admission;
mod boot;
mod display;
mod gdm_registration;
mod hardware_identity;
mod home;
mod login;
mod processes;
mod runtime;
mod session;
mod waiting;

pub use admission::pam_gate;

/// Withdraws contest permission during service shutdown, including crash cleanup.
///
/// # Errors
/// Fails if not root or the runtime directory is unavailable.
pub fn close_admission() -> Result<(), natsume_local_control_api::ResourceControlError> {
    if !rustix::process::geteuid().is_root() {
        return Err(session::rejected("admission withdrawal requires root"));
    }
    admission::close(std::path::Path::new("/"))
}
pub use login::{GreeterStatus, probe_greeter, run_gdm_client, run_prepare};
pub use session::probe_desktop;

use std::path::PathBuf;

use natsume_local_control_api::{
    DerivedMachineIdentity, GraphicalSession, GraphicalSessionObservation, HomeResetProgress,
    MachineIdentityError, ManagedSessionsObservation, ResourceControlError, SessionRole,
};

/// Closed root capabilities exposed to the Device Daemon.
pub struct PrivilegedService {
    filesystem_root: PathBuf,
}

impl PrivilegedService {
    /// Creates the production collector rooted at the host filesystem.
    #[must_use]
    pub fn production() -> Self {
        Self {
            filesystem_root: PathBuf::from("/"),
        }
    }

    /// Starts the independent startup observer after this service owns its bus name.
    /// Existing Home/identity capabilities remain available during observation outages.
    #[must_use]
    pub fn observe_gdm_registration(&self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(gdm_registration::observe(self.filesystem_root.clone()))
    }
}

async fn bounded_session_operation<T>(
    operation: impl std::future::Future<Output = Result<T, ResourceControlError>>,
) -> Result<T, ResourceControlError> {
    tokio::time::timeout(std::time::Duration::from_secs(8), operation)
        .await
        .map_err(|_| session::unavailable("session operation timed out; resample before retry"))?
}

#[zbus::interface(name = "org.natsume.Privileged1")]
impl PrivilegedService {
    #[zbus(name = "DeriveMachineIdentity")]
    fn derive_machine_identity(&self) -> Result<DerivedMachineIdentity, MachineIdentityError> {
        hardware_identity::derive_identity(&self.filesystem_root)
    }

    /// Retries local mount recovery without granting any remote foreground target.
    ///
    /// # Errors
    /// Keeps admission closed on invalid state, incomplete drainage or mount failure.
    #[zbus(name = "RecoverLocalHome")]
    pub async fn recover_local_home(
        &mut self,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<(), ResourceControlError> {
        bounded_session_operation(home::window::restore_home(
            &self.filesystem_root,
            connection,
        ))
        .await
    }

    #[zbus(name = "IsHomeReady")]
    fn is_home_ready(&self) -> bool {
        admission::require_open(&self.filesystem_root).is_ok()
            && home::require_template(&self.filesystem_root).is_ok()
            && admission::require_open(&self.filesystem_root).is_ok()
    }

    #[zbus(name = "HasHomeResetState")]
    fn has_home_reset_state(&self) -> Result<bool, ResourceControlError> {
        home::state_exists(&self.filesystem_root)
    }

    #[zbus(name = "QueryManagedSessions")]
    async fn query_managed_sessions(
        &self,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<ManagedSessionsObservation, ResourceControlError> {
        bounded_session_operation(login::observe(connection, &self.filesystem_root)).await
    }

    #[zbus(name = "PrepareBootSessions")]
    async fn prepare_boot_sessions(
        &mut self,
        waiting: GraphicalSession,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<bool, ResourceControlError> {
        bounded_session_operation(boot::prepare(connection, &self.filesystem_root, &waiting)).await
    }

    #[zbus(name = "PrepareSession")]
    async fn prepare_session(
        &mut self,
        role: SessionRole,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<GraphicalSessionObservation, ResourceControlError> {
        bounded_session_operation(login::start(connection, &self.filesystem_root, role)).await
    }

    #[zbus(name = "RecoverWaitingSession")]
    async fn recover_waiting_session(
        &mut self,
        expected: Option<GraphicalSession>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<bool, ResourceControlError> {
        bounded_session_operation(waiting::rebuild(
            connection,
            &self.filesystem_root,
            expected.as_ref(),
        ))
        .await
    }

    #[zbus(name = "ResumeWaitingRecovery")]
    async fn resume_waiting_recovery(
        &mut self,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<bool, ResourceControlError> {
        bounded_session_operation(waiting::resume(connection, &self.filesystem_root)).await
    }

    #[zbus(name = "ActivateSession")]
    async fn activate_session(
        &mut self,
        role: SessionRole,
        target: GraphicalSession,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<(), ResourceControlError> {
        let _mutation = admission::mutation(&self.filesystem_root)?;
        if role == SessionRole::Contest {
            admission::require_open(&self.filesystem_root)?;
        }
        bounded_session_operation(session::activate(
            connection,
            &self.filesystem_root,
            role,
            &target,
        ))
        .await
    }

    #[zbus(name = "CloseContestAdmission")]
    async fn close_contest_admission(
        &mut self,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<(), ResourceControlError> {
        bounded_session_operation(login::close_contest(connection, &self.filesystem_root)).await
    }

    #[zbus(name = "TerminateContestSession")]
    async fn terminate_contest_session(
        &mut self,
        target: GraphicalSession,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<(), ResourceControlError> {
        let _mutation = admission::mutation(&self.filesystem_root)?;
        bounded_session_operation(session::terminate(
            connection,
            &self.filesystem_root,
            &target,
        ))
        .await
    }

    #[zbus(name = "PrepareHomeReset")]
    async fn prepare_home_reset(
        &mut self,
        reset_epoch: u64,
        waiting: GraphicalSession,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<(), ResourceControlError> {
        bounded_session_operation(home::window::prepare(
            &self.filesystem_root,
            connection,
            reset_epoch,
            &waiting,
        ))
        .await
    }

    #[zbus(name = "ApplyHomeReset")]
    async fn apply_home_reset(
        &mut self,
        reset_epoch: u64,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<(), ResourceControlError> {
        bounded_session_operation(home::window::apply(
            &self.filesystem_root,
            connection,
            reset_epoch,
        ))
        .await
    }

    #[zbus(name = "QueryHomeReset")]
    fn query_home_reset(&self) -> Result<Option<HomeResetProgress>, ResourceControlError> {
        home::window::query(&self.filesystem_root)
    }

    #[zbus(name = "VerifyHomeReset")]
    fn verify_home_reset(
        &mut self,
        reset_epoch: u64,
    ) -> Result<HomeResetProgress, ResourceControlError> {
        home::window::verify(&self.filesystem_root, reset_epoch)
    }

    #[zbus(name = "RecoverHomeReset")]
    async fn recover_home_reset(
        &mut self,
        reset_epoch: u64,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<(), ResourceControlError> {
        bounded_session_operation(home::window::recover(
            &self.filesystem_root,
            connection,
            reset_epoch,
        ))
        .await
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt as _, path::Path};

    use natsume_local_control_api::{PRIVILEGED1_PATH, Privileged1Proxy};
    use tempfile::TempDir;
    use tokio::net::UnixStream;

    use super::*;

    impl PrivilegedService {
        fn fixture(filesystem_root: &Path) -> Self {
            Self {
                filesystem_root: filesystem_root.to_owned(),
            }
        }
    }

    fn tempdir() -> TempDir {
        match TempDir::new() {
            Ok(directory) => directory,
            Err(error) => panic!("fixture directory must be created: {error}"),
        }
    }

    fn write_fixture(root: &Path, relative: &str, bytes: &[u8]) {
        let path = root.join(relative);
        let Some(parent) = path.parent() else {
            panic!("fixture path must have a parent");
        };
        if let Err(error) = fs::create_dir_all(parent) {
            panic!("fixture parent must be created: {error}");
        }
        if let Err(error) = fs::write(path, bytes) {
            panic!("fixture file must be written: {error}");
        }
    }

    fn service_fixture() -> (TempDir, PrivilegedService) {
        let fixture = tempdir();
        write_fixture(
            fixture.path(),
            "sys/class/dmi/id/product_uuid",
            b"550E8400-E29B-41D4-A716-446655440000\n",
        );
        write_fixture(
            fixture.path(),
            "sys/class/dmi/id/board_serial",
            b"BOARD-42\n",
        );
        let dev_block = fixture.path().join("sys/dev/block");
        if let Err(error) = fs::create_dir_all(&dev_block) {
            panic!("sysfs block fixture must be created: {error}");
        }
        let service = PrivilegedService::fixture(fixture.path());
        (fixture, service)
    }

    #[tokio::test]
    #[expect(
        clippy::too_many_lines,
        reason = "One real peer-bus round trip checks the fixed capabilities and retained waiting budget together"
    )]
    async fn generated_proxy_round_trips_the_real_service() {
        let (fixture, service) = service_fixture();
        let streams = UnixStream::pair();
        let Ok((server_stream, client_stream)) = streams else {
            panic!("unix socketpair must be created");
        };
        let guid = zbus::Guid::generate();
        let server_builder = match zbus::connection::Builder::unix_stream(server_stream)
            .server(guid)
            .and_then(|builder| builder.p2p().serve_at(PRIVILEGED1_PATH, service))
        {
            Ok(builder) => builder,
            Err(error) => panic!("server builder must be configured: {error}"),
        };
        let client_builder = zbus::connection::Builder::unix_stream(client_stream).p2p();
        let (server, client) = tokio::join!(server_builder.build(), client_builder.build());
        let Ok(_server) = server else {
            panic!("peer server connection must be built: {server:?}");
        };
        let Ok(client) = client else {
            panic!("peer client connection must be built: {client:?}");
        };
        let proxy = match Privileged1Proxy::new(&client).await {
            Ok(proxy) => proxy,
            Err(error) => panic!("generated proxy must be built: {error}"),
        };

        let identity = match proxy.derive_machine_identity().await {
            Ok(identity) => identity,
            Err(error) => panic!("machine identity must round trip: {error}"),
        };

        assert_eq!(
            identity.machine_hardware_id,
            "939d8a7f-642f-583e-ad40-ba40c44b6b0e"
        );
        assert_eq!(
            identity.quality,
            natsume_local_control_api::MachineIdentityQuality::Strong
        );
        assert!(
            !proxy
                .has_home_reset_state()
                .await
                .unwrap_or_else(|error| panic!("empty Home state query failed: {error}"))
        );
        write_fixture(
            fixture.path(),
            "var/lib/natsume-privileged/home-reset/progress",
            b"7\nverified\n",
        );
        assert!(
            proxy
                .has_home_reset_state()
                .await
                .unwrap_or_else(|error| panic!("present Home state query failed: {error}"))
        );
        assert!(
            !proxy
                .resume_waiting_recovery()
                .await
                .unwrap_or_else(|e| panic!("resume: {e}"))
        );
        let boot = "550e8400-e29b-41d4-a716-446655440000";
        let spent = format!("1\n{boot}\nw1\nfinished\n60\n");
        write_fixture(
            fixture.path(),
            "proc/sys/kernel/random/boot_id",
            boot.as_bytes(),
        );
        write_fixture(fixture.path(), "proc/uptime", b"100.00 0.00\n");
        write_fixture(
            fixture.path(),
            "run/natsume-privileged/waiting-recovery",
            spent.as_bytes(),
        );
        fs::set_permissions(
            fixture.path().join("run/natsume-privileged"),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap_or_else(|e| panic!("runtime permissions: {e}"));
        let replacement = Some(GraphicalSession {
            boot_id: boot.to_owned(),
            logind_session_id: "w2".to_owned(),
        });
        // No mock logind exists on this peer bus. A spent replay must return
        // without trying to capture or terminate the newly supplied session.
        assert!(
            !proxy
                .recover_waiting_session(&replacement)
                .await
                .unwrap_or_else(|e| panic!("spent replay: {e}"))
        );
        assert!(
            !proxy
                .resume_waiting_recovery()
                .await
                .unwrap_or_else(|e| panic!("spent resume: {e}"))
        );
        assert_eq!(
            fs::read_to_string(
                fixture
                    .path()
                    .join("run/natsume-privileged/waiting-recovery")
            )
            .unwrap_or_else(|e| panic!("record: {e}")),
            spent
        );
    }
}
