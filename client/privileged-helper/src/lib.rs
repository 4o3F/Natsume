#![forbid(unsafe_code)]

mod hardware_identity;
mod home;
mod session;

use std::path::{Path, PathBuf};

use natsume_local_control_api::{
    ContestSessionObservation, DerivedMachineIdentity, GraphicalSession, HomeResetProgress,
    MachineIdentityError, SessionLockLevel,
};
use uuid::Uuid;

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
}

fn canonical_uuid(value: &str) -> Option<Uuid> {
    Uuid::parse_str(value)
        .ok()
        .filter(|uuid| uuid.hyphenated().to_string() == value)
}

#[zbus::interface(name = "org.natsume.Privileged1")]
impl PrivilegedService {
    #[zbus(name = "DeriveMachineIdentity")]
    fn derive_machine_identity(
        &self,
        fleet_namespace_uuid: &str,
    ) -> Result<DerivedMachineIdentity, MachineIdentityError> {
        let Some(namespace) = canonical_uuid(fleet_namespace_uuid) else {
            return Err(MachineIdentityError::InvalidArguments(
                "fleet namespace UUID must use canonical lowercase hyphenated form".to_owned(),
            ));
        };
        hardware_identity::derive_identity(&self.filesystem_root, namespace)
    }

    #[zbus(name = "HasHomeResetState")]
    fn has_home_reset_state(&self) -> zbus::fdo::Result<bool> {
        home::state_exists(&self.filesystem_root)
    }

    #[zbus(name = "QueryContestSession")]
    async fn query_contest_session(
        &self,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> zbus::fdo::Result<ContestSessionObservation> {
        session::observe(connection, &self.filesystem_root).await
    }

    #[zbus(name = "SetContestSessionLock")]
    async fn set_contest_session_lock(
        &mut self,
        target: GraphicalSession,
        level: SessionLockLevel,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> zbus::fdo::Result<()> {
        session::set_lock(connection, &self.filesystem_root, &target, level).await
    }

    #[zbus(name = "TerminateContestSession")]
    async fn terminate_contest_session(
        &mut self,
        target: GraphicalSession,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> zbus::fdo::Result<()> {
        session::terminate(connection, &self.filesystem_root, &target).await
    }

    #[zbus(name = "PrepareHomeReset")]
    async fn prepare_home_reset(
        &mut self,
        reset_epoch: u64,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> zbus::fdo::Result<()> {
        require_no_contest_session(connection, &self.filesystem_root).await?;
        home::prepare(&self.filesystem_root, reset_epoch)
    }

    #[zbus(name = "ApplyHomeReset")]
    async fn apply_home_reset(
        &mut self,
        reset_epoch: u64,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> zbus::fdo::Result<()> {
        require_no_contest_session(connection, &self.filesystem_root).await?;
        home::apply(&self.filesystem_root, reset_epoch)
    }

    #[zbus(name = "QueryHomeReset")]
    fn query_home_reset(&self) -> zbus::fdo::Result<Option<HomeResetProgress>> {
        home::query(&self.filesystem_root)
    }

    #[zbus(name = "VerifyHomeReset")]
    fn verify_home_reset(&mut self, reset_epoch: u64) -> zbus::fdo::Result<HomeResetProgress> {
        home::verify(&self.filesystem_root, reset_epoch)
    }

    #[zbus(name = "RecoverHomeReset")]
    async fn recover_home_reset(
        &mut self,
        reset_epoch: u64,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> zbus::fdo::Result<()> {
        require_no_contest_session(connection, &self.filesystem_root).await?;
        home::recover(&self.filesystem_root, reset_epoch)
    }
}

async fn require_no_contest_session(
    connection: &zbus::Connection,
    filesystem_root: &Path,
) -> zbus::fdo::Result<()> {
    let observation = session::observe(connection, filesystem_root).await?;
    if observation.state == natsume_local_control_api::ContestSessionState::None {
        Ok(())
    } else {
        Err(zbus::fdo::Error::Failed(
            "contestant graphical session must be absent during Home reset".to_owned(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use natsume_local_control_api::{PRIVILEGED1_PATH, Privileged1Proxy};
    use tempfile::TempDir;
    use tokio::net::UnixStream;

    use super::*;

    const TEST_NAMESPACE: &str = "12345678-1234-5678-9234-567812345678";

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

        let identity = match proxy.derive_machine_identity(TEST_NAMESPACE).await {
            Ok(identity) => identity,
            Err(error) => panic!("machine identity must round trip: {error}"),
        };

        assert_eq!(
            identity.machine_hardware_id,
            "7868c4db-ba77-52b9-a93c-f1ee2445e5f8"
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
    }

    #[test]
    fn namespace_validation_requires_exact_canonical_form() {
        assert!(canonical_uuid(TEST_NAMESPACE).is_some());
        assert!(canonical_uuid("12345678123456789234567812345678").is_none());
        assert!(canonical_uuid("12345678-1234-5678-9234-56781234567A").is_none());
        assert!(canonical_uuid("not-a-uuid").is_none());
    }
}
