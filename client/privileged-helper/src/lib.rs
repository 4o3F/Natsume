#![forbid(unsafe_code)]

pub mod hardware_identity;

use std::path::PathBuf;

use natsume_local_control_api::SanitizedHardwareClaim;
use procfs::process::MountInfo;
use uuid::Uuid;

/// The sole D-Bus interface implemented by the privileged helper in WP3.
pub struct PrivilegedService {
    filesystem_root: PathBuf,
    fixture_mountinfo: Option<Vec<MountInfo>>,
}

impl PrivilegedService {
    /// Creates the production collector rooted at the host filesystem.
    #[must_use]
    pub fn production() -> Self {
        Self {
            filesystem_root: PathBuf::from("/"),
            fixture_mountinfo: None,
        }
    }

    fn collect_claim(&self, namespace: Uuid) -> SanitizedHardwareClaim {
        match self.fixture_mountinfo.as_deref() {
            Some(mountinfo) => hardware_identity::derive_claim_with_mountinfo(
                &self.filesystem_root,
                mountinfo,
                namespace,
            ),
            None => hardware_identity::derive_claim(&self.filesystem_root, namespace),
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
    #[zbus(name = "CollectHardwareCandidates")]
    fn collect_hardware_candidates(
        &self,
        fleet_namespace_uuid: &str,
    ) -> zbus::fdo::Result<SanitizedHardwareClaim> {
        let Some(namespace) = canonical_uuid(fleet_namespace_uuid) else {
            return Err(zbus::fdo::Error::InvalidArgs(
                "fleet namespace UUID must use canonical lowercase hyphenated form".to_owned(),
            ));
        };
        Ok(self.collect_claim(namespace))
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::symlink, path::Path};

    use natsume_local_control_api::{PRIVILEGED1_PATH, Privileged1Proxy};
    use tempfile::TempDir;
    use tokio::net::UnixStream;

    use super::*;

    const TEST_NAMESPACE: &str = "12345678-1234-5678-9234-567812345678";

    impl PrivilegedService {
        fn fixture(filesystem_root: &Path, mountinfo: Vec<MountInfo>) -> Self {
            Self {
                filesystem_root: filesystem_root.to_owned(),
                fixture_mountinfo: Some(mountinfo),
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
        write_fixture(
            fixture.path(),
            "sys/devices/pci/block/sda/sda2/partition",
            b"2\n",
        );
        write_fixture(fixture.path(), "sys/devices/pci/block/sda/dev", b"8:0\n");
        write_fixture(
            fixture.path(),
            "run/udev/data/b8:0",
            b"E:ID_SERIAL_SHORT=DISK_99\n",
        );
        let dev_block = fixture.path().join("sys/dev/block");
        if let Err(error) = fs::create_dir_all(&dev_block) {
            panic!("sysfs block fixture must be created: {error}");
        }
        if let Err(error) = symlink("../../devices/pci/block/sda/sda2", dev_block.join("8:2")) {
            panic!("sysfs block symlink must be created: {error}");
        }
        let mount = match MountInfo::from_line("36 25 8:2 / / rw,relatime - ext4 /dev/sda2 rw") {
            Ok(mount) => mount,
            Err(error) => panic!("mountinfo fixture must parse: {error}"),
        };
        let service = PrivilegedService::fixture(fixture.path(), vec![mount]);
        (fixture, service)
    }

    #[tokio::test]
    async fn generated_proxy_round_trips_the_real_single_method_service() {
        let (_fixture, service) = service_fixture();
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
            panic!("peer server connection must be built");
        };
        let Ok(client) = client else {
            panic!("peer client connection must be built");
        };
        let proxy = match Privileged1Proxy::new(&client).await {
            Ok(proxy) => proxy,
            Err(error) => panic!("generated proxy must be built: {error}"),
        };

        let claim = match proxy.collect_hardware_candidates(TEST_NAMESPACE).await {
            Ok(claim) => claim,
            Err(error) => panic!("hardware claim must round trip: {error}"),
        };

        assert_eq!(claim.decision, "derived");
        assert_eq!(
            claim.machine_hardware_id.as_deref(),
            Some("a9aa9d04-3ece-5567-8260-910930ff5e03")
        );
        assert_eq!(claim.present_slot_count, 3);
        assert!(claim.collection_complete);
        assert_eq!(claim.candidates.len(), 3);
    }

    #[test]
    fn namespace_validation_requires_exact_canonical_form() {
        assert!(canonical_uuid(TEST_NAMESPACE).is_some());
        assert!(canonical_uuid("12345678123456789234567812345678").is_none());
        assert!(canonical_uuid("12345678-1234-5678-9234-56781234567A").is_none());
        assert!(canonical_uuid("not-a-uuid").is_none());
    }
}
