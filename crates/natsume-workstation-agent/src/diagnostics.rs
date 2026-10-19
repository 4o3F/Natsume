//! Historical, non-authoritative evidence from fixed boot-local report files.

use std::{fs::File, io::Read, net::IpAddr, path::Path};

use natsume_workstation_ipc::diagnostics::{
    DIAGNOSTIC_DIRECTORY, DIAGNOSTIC_FAILURE_NAME, DIAGNOSTIC_FORMAT_VERSION,
    DIAGNOSTIC_REPORT_NAME, DiagnosticReport, MAX_DIAGNOSTIC_EVENTS, MAX_DIAGNOSTIC_REPORT_BYTES,
};
use natsume_workstation_protocol::{CONTROL_SUBPROTOCOL, is_valid_error_code_token};
use qrcode::{Color, EcLevel, QrCode};
use rustix::{
    fd::{AsFd, OwnedFd},
    fs::{FileType, Mode, OFlags, fstat, open, openat},
    io::Errno,
};
use slint::{Rgb8Pixel, SharedPixelBuffer};
use uuid::{Uuid, Variant};

pub const MAX_QR_PAYLOAD_BYTES: usize = 2300;
const MAX_PASSWD_BYTES: usize = 64 * 1024;
const QUIET_ZONE: usize = 4;
const MODULE_PIXELS: usize = 4;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiagnosticEvidence {
    pub latest: Result<DiagnosticReport, ReportReadError>,
    pub last_failure: Result<DiagnosticReport, ReportReadError>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReportReadError {
    Missing,
    Unavailable,
    Untrusted,
    TooLarge,
    Invalid,
    UnsupportedVersion,
    WrongBoot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReportSource {
    Latest,
    LastFailure,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QrError {
    InvalidReport,
    TooLarge,
    EncodingFailed,
}

impl DiagnosticEvidence {
    /// Select one entire historical report; never infer current availability.
    #[must_use]
    pub fn selected_report(&self) -> Option<(&DiagnosticReport, ReportSource)> {
        self.latest
            .as_ref()
            .ok()
            .map(|report| (report, ReportSource::Latest))
            .or_else(|| {
                self.last_failure
                    .as_ref()
                    .ok()
                    .map(|report| (report, ReportSource::LastFailure))
            })
    }
}

/// Read only the fixed reports, not configuration, credentials, or live authority.
#[must_use]
pub fn read_evidence(boot_id: &str) -> DiagnosticEvidence {
    let directory = (|| {
        let owner = production_owner()?;
        let run = trusted_directory(Path::new("/run"), 0, None)?;
        let name = Path::new(DIAGNOSTIC_DIRECTORY)
            .file_name()
            .ok_or(ReportReadError::Untrusted)?;
        let directory = openat(&run, name, directory_flags(), Mode::empty()).map_err(open_error)?;
        check_handle(&directory, FileType::Directory, owner, Some(0o755))?;
        Ok((directory, owner))
    })();
    match directory {
        Ok((directory, owner)) => read_from_directory(&directory, owner, boot_id),
        Err(error) => DiagnosticEvidence {
            latest: Err(error),
            last_failure: Err(error),
        },
    }
}

fn directory_flags() -> OFlags {
    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK
}

fn file_flags() -> OFlags {
    OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK
}

fn open_error(error: Errno) -> ReportReadError {
    match error {
        Errno::NOENT => ReportReadError::Missing,
        Errno::LOOP | Errno::NOTDIR => ReportReadError::Untrusted,
        _ => ReportReadError::Unavailable,
    }
}

fn check_handle(
    handle: &impl AsFd,
    kind: FileType,
    owner: u32,
    permissions: Option<u32>,
) -> Result<(), ReportReadError> {
    let stat = fstat(handle).map_err(|_| ReportReadError::Unavailable)?;
    if FileType::from_raw_mode(stat.st_mode) != kind
        || stat.st_uid != owner
        || permissions.map_or(stat.st_mode & 0o7022 != 0, |mode| {
            stat.st_mode & 0o7777 != mode
        })
    {
        return Err(ReportReadError::Untrusted);
    }
    Ok(())
}

fn trusted_directory(
    path: &Path,
    owner: u32,
    permissions: Option<u32>,
) -> Result<OwnedFd, ReportReadError> {
    let handle = open(path, directory_flags(), Mode::empty()).map_err(open_error)?;
    check_handle(&handle, FileType::Directory, owner, permissions)?;
    Ok(handle)
}

fn production_owner() -> Result<u32, ReportReadError> {
    let etc = trusted_directory(Path::new("/etc"), 0, None)?;
    let handle = openat(&etc, "passwd", file_flags(), Mode::empty()).map_err(open_error)?;
    check_handle(&handle, FileType::RegularFile, 0, None)?;
    let bytes = bounded_read(File::from(handle), MAX_PASSWD_BYTES)?;
    passwd_owner(&bytes)
}

fn passwd_owner(bytes: &[u8]) -> Result<u32, ReportReadError> {
    let text = std::str::from_utf8(bytes).map_err(|_| ReportReadError::Untrusted)?;
    let mut owner = None;
    for line in text.lines() {
        let mut fields = line.split(':');
        if fields.next() != Some("natsume") {
            continue;
        }
        let fields: Vec<_> = fields.collect();
        if fields.len() != 6 || owner.is_some() {
            return Err(ReportReadError::Untrusted);
        }
        let uid = fields[1];
        if uid.is_empty() || !uid.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(ReportReadError::Untrusted);
        }
        owner = Some(uid.parse().map_err(|_| ReportReadError::Untrusted)?);
    }
    owner.ok_or(ReportReadError::Unavailable)
}

fn read_from_directory(directory: &impl AsFd, owner: u32, boot_id: &str) -> DiagnosticEvidence {
    DiagnosticEvidence {
        latest: read_report(directory, DIAGNOSTIC_REPORT_NAME, owner, boot_id),
        last_failure: read_report(directory, DIAGNOSTIC_FAILURE_NAME, owner, boot_id),
    }
}

fn read_report(
    directory: &impl AsFd,
    name: &str,
    owner: u32,
    boot_id: &str,
) -> Result<DiagnosticReport, ReportReadError> {
    let handle = openat(directory, name, file_flags(), Mode::empty()).map_err(open_error)?;
    check_handle(&handle, FileType::RegularFile, owner, Some(0o644))?;
    let stat = fstat(&handle).map_err(|_| ReportReadError::Unavailable)?;
    if usize::try_from(stat.st_size).map_or(true, |size| size > MAX_DIAGNOSTIC_REPORT_BYTES) {
        return Err(ReportReadError::TooLarge);
    }
    // The descriptor pins the inode; take also bounds reads if the file grows.
    let bytes = bounded_read(File::from(handle), MAX_DIAGNOSTIC_REPORT_BYTES)?;
    decode_report(&bytes, boot_id)
}

fn bounded_read(reader: impl Read, maximum: usize) -> Result<Vec<u8>, ReportReadError> {
    let mut bytes = Vec::new();
    reader
        .take((maximum + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| ReportReadError::Unavailable)?;
    if bytes.len() > maximum {
        return Err(ReportReadError::TooLarge);
    }
    Ok(bytes)
}

fn decode_report(bytes: &[u8], boot_id: &str) -> Result<DiagnosticReport, ReportReadError> {
    let report: DiagnosticReport =
        serde_json::from_slice(bytes).map_err(|_| ReportReadError::Invalid)?;
    // This shared nested type is a D-Bus type without deny_unknown_fields.
    // Check its keys locally without changing the shared protocol surface.
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| ReportReadError::Invalid)?;
    if let Some(sources) = value
        .pointer("/identity/sources")
        .and_then(serde_json::Value::as_object)
        && sources.keys().any(|key| {
            !matches!(
                key.as_str(),
                "dmi_system_uuid" | "dmi_board_serial" | "first_disk_serial"
            )
        })
    {
        return Err(ReportReadError::Invalid);
    }
    validate_report(&report)?;
    if !canonical_uuid(boot_id, 4) || report.boot_id.as_deref() != Some(boot_id) {
        return Err(ReportReadError::WrongBoot);
    }
    Ok(report)
}

fn canonical_uuid(value: &str, version: usize) -> bool {
    Uuid::parse_str(value).is_ok_and(|id| {
        id.hyphenated().to_string() == value
            && id.get_variant() == Variant::RFC4122
            && id.get_version_num() == version
    })
}

fn valid_instance(value: &str) -> bool {
    if value.len() == 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return true;
    }
    // The publisher's entropy-unavailable fallback is decimal PID-boot_ms.
    value.len() <= 31
        && value.split_once('-').is_some_and(|(pid, millis)| {
            !pid.is_empty()
                && !millis.is_empty()
                && pid
                    .bytes()
                    .chain(millis.bytes())
                    .all(|byte| byte.is_ascii_digit())
                && pid.parse::<u32>().is_ok_and(|pid| pid > 0)
                && millis.parse::<u64>().is_ok()
        })
}

fn valid_interface(value: &str) -> bool {
    !value.is_empty()
        && value.len() < 16
        && !matches!(value, "." | "..")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

fn valid_ip(value: &str) -> bool {
    value.len() <= 45 && value.parse::<IpAddr>().is_ok()
}

fn validate_report(report: &DiagnosticReport) -> Result<(), ReportReadError> {
    if report.format_version != DIAGNOSTIC_FORMAT_VERSION {
        return Err(ReportReadError::UnsupportedVersion);
    }
    let connection = &report.connection;
    if report.events.len() > MAX_DIAGNOSTIC_EVENTS
        || report
            .boot_id
            .as_deref()
            .is_some_and(|id| !canonical_uuid(id, 4))
        || !valid_instance(&report.daemon_instance_id)
        || report.daemon_version.len() > 64
        || semver::Version::parse(&report.daemon_version).is_err()
        || report.control_protocol != CONTROL_SUBPROTOCOL
        || report
            .identity
            .machine_hardware_id
            .as_deref()
            .is_some_and(|id| !canonical_uuid(id, 5))
        || report
            .identity
            .valid_source_count
            .is_some_and(|count| count > 3)
        || connection
            .device_id
            .as_deref()
            .is_some_and(|id| !canonical_uuid(id, 7))
        || connection
            .endpoint_ip
            .as_deref()
            .is_some_and(|ip| !valid_ip(ip))
        || connection
            .local_ip
            .as_deref()
            .is_some_and(|ip| !valid_ip(ip))
        || connection
            .network
            .next_hop
            .as_deref()
            .is_some_and(|ip| !valid_ip(ip))
        || connection
            .network
            .interface
            .as_deref()
            .is_some_and(|name| !valid_interface(name))
        || connection
            .peer_error_code
            .as_deref()
            .is_some_and(|code| !is_valid_error_code_token(code))
        || connection
            .trust_root_sha256
            .as_deref()
            .is_some_and(|digest| {
                digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
    {
        return Err(ReportReadError::Invalid);
    }
    Ok(())
}

/// Re-serialize only reviewed typed fields, omitting event history if needed.
///
/// # Errors
/// Returns a fixed error for unsafe reports or reports that cannot fit offline QR.
pub fn qr_payload(report: &DiagnosticReport) -> Result<String, QrError> {
    validate_report(report).map_err(|_| QrError::InvalidReport)?;
    let payload = serde_json::to_string(report).map_err(|_| QrError::EncodingFailed)?;
    if payload.len() <= MAX_QR_PAYLOAD_BYTES {
        return Ok(payload);
    }
    let mut reduced = report.clone();
    reduced.events.clear();
    reduced.events_truncated = true;
    let payload = serde_json::to_string(&reduced).map_err(|_| QrError::EncodingFailed)?;
    if payload.len() > MAX_QR_PAYLOAD_BYTES {
        return Err(QrError::TooLarge);
    }
    Ok(payload)
}

fn qr_pixels(payload: &str) -> Result<SharedPixelBuffer<Rgb8Pixel>, QrError> {
    if payload.len() > MAX_QR_PAYLOAD_BYTES {
        return Err(QrError::TooLarge);
    }
    let qr = QrCode::with_error_correction_level(payload.as_bytes(), EcLevel::L)
        .map_err(|_| QrError::EncodingFailed)?;
    let width = qr.width();
    let side = (width + QUIET_ZONE * 2) * MODULE_PIXELS;
    let dimension = u32::try_from(side).map_err(|_| QrError::EncodingFailed)?;
    let mut buffer = SharedPixelBuffer::<Rgb8Pixel>::new(dimension, dimension);
    for (index, pixel) in buffer.make_mut_slice().iter_mut().enumerate() {
        let x = index % side / MODULE_PIXELS;
        let y = index / side / MODULE_PIXELS;
        let dark = x >= QUIET_ZONE
            && y >= QUIET_ZONE
            && x < width + QUIET_ZONE
            && y < width + QUIET_ZONE
            && qr[(x - QUIET_ZONE, y - QUIET_ZONE)] == Color::Dark;
        *pixel = if dark {
            Rgb8Pixel::new(0x11, 0x11, 0x11)
        } else {
            Rgb8Pixel::new(0xf5, 0xf5, 0xf3)
        };
    }
    Ok(buffer)
}

/// Render ECC L with an integer pixel grid and a four-module quiet zone.
///
/// # Errors
/// Returns a fixed error if the payload exceeds the cap or QR encoding fails.
pub fn qr_image(payload: &str) -> Result<slint::Image, QrError> {
    qr_pixels(payload).map(slint::Image::from_rgb8)
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        io::{Cursor, Seek, SeekFrom, Write},
        os::unix::fs::{PermissionsExt, symlink},
    };

    use natsume_workstation_ipc::diagnostics::{
        ConnectionDiagnostic, DiagnosticCode, DiagnosticEvent, DiagnosticStage, DiagnosticStatus,
        IdentityDiagnostic,
    };
    use natsume_workstation_ipc::{HardwareIdentitySources, HardwareSourceStatus};

    use super::*;

    const BOOT: &str = "7138aed1-5921-4c4b-8e35-110806942def";
    const OTHER_BOOT: &str = "db6192f8-2a15-43e2-9bb1-7d5db84372fe";

    fn report() -> DiagnosticReport {
        DiagnosticReport {
            format_version: 1,
            boot_id: Some(BOOT.to_owned()),
            daemon_instance_id: "a123456789abcdef0123456789abcdef".to_owned(),
            daemon_pid: 321,
            daemon_version: "2.8.0".to_owned(),
            control_protocol: CONTROL_SUBPROTOCOL.to_owned(),
            sequence: 17,
            captured_at_unix_ms: None,
            captured_at_boot_ms: Some(1234),
            stage: DiagnosticStage::Starting,
            last_completed_stage: None,
            status: DiagnosticStatus::Starting,
            failure: None,
            identity: IdentityDiagnostic::default(),
            connection: ConnectionDiagnostic::default(),
            events: vec![],
            events_truncated: false,
        }
    }

    fn populated_report() -> DiagnosticReport {
        let mut report = report();
        report.identity.sources = Some(HardwareIdentitySources {
            dmi_system_uuid: HardwareSourceStatus::Present,
            dmi_board_serial: HardwareSourceStatus::Unavailable,
            first_disk_serial: HardwareSourceStatus::Present,
        });
        report.identity.valid_source_count = Some(2);
        report.identity.machine_hardware_id =
            Some("0c0fef01-1126-5297-a522-92cfe494ce48".to_owned());
        report.connection.device_id = Some("01951479-498a-7f3d-929d-fd118db192ba".to_owned());
        report.connection.endpoint_ip = Some("2001:db8::1".to_owned());
        report.connection.local_ip = Some("192.0.2.2".to_owned());
        report.connection.network.next_hop = Some("192.0.2.1".to_owned());
        report.connection.network.interface = Some("enp1s0.10".to_owned());
        report.connection.peer_error_code = Some("FUTURE_PEER_CODE_7".to_owned());
        report.connection.trust_root_sha256 = Some("ab".repeat(32));
        report
    }

    fn fixture() -> (tempfile::TempDir, u32) {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("fixture: {error}"));
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o755))
            .unwrap_or_else(|error| panic!("permissions: {error}"));
        (directory, rustix::process::geteuid().as_raw())
    }

    fn write_report(directory: &Path, name: &str, report: &DiagnosticReport) {
        let bytes = serde_json::to_vec(report).unwrap_or_else(|error| panic!("serialize: {error}"));
        fs::write(directory.join(name), bytes).unwrap_or_else(|error| panic!("write: {error}"));
        fs::set_permissions(directory.join(name), fs::Permissions::from_mode(0o644))
            .unwrap_or_else(|error| panic!("permissions: {error}"));
    }

    fn read_fixture(directory: &Path, owner: u32) -> DiagnosticEvidence {
        match trusted_directory(directory, owner, Some(0o755)) {
            Ok(handle) => read_from_directory(&handle, owner, BOOT),
            Err(error) => DiagnosticEvidence {
                latest: Err(error),
                last_failure: Err(error),
            },
        }
    }

    #[test]
    fn valid_synthetic_schema_and_unknowns_roundtrip() {
        for report in [report(), populated_report()] {
            let (directory, owner) = fixture();
            write_report(directory.path(), DIAGNOSTIC_REPORT_NAME, &report);
            let evidence = read_fixture(directory.path(), owner);
            assert_eq!(evidence.latest, Ok(report.clone()));
            assert_eq!(evidence.last_failure, Err(ReportReadError::Missing));
            let decoded: DiagnosticReport = serde_json::from_str(
                &qr_payload(&report).unwrap_or_else(|error| panic!("payload: {error:?}")),
            )
            .unwrap_or_else(|error| panic!("decode: {error}"));
            assert_eq!(decoded, report);
        }
        let mut fallback = report();
        fallback.daemon_instance_id = "321-1234".to_owned();
        assert_eq!(validate_report(&fallback), Ok(()));
    }

    #[test]
    fn missing_files_directory_and_passwd_lookup_are_typed() {
        let (directory, owner) = fixture();
        let missing = read_fixture(&directory.path().join("missing"), owner);
        assert_eq!(missing.latest, Err(ReportReadError::Missing));
        let empty = read_fixture(directory.path(), owner);
        assert_eq!(empty.latest, Err(ReportReadError::Missing));
        assert_eq!(empty.last_failure, Err(ReportReadError::Missing));
        assert!(empty.selected_report().is_none());
        assert_eq!(
            passwd_owner(
                b"root:x:0:0:root:/root:/bin/sh\nnatsume:x:123:123::/var/lib/natsume:/bin/false\n"
            ),
            Ok(123)
        );
        assert_eq!(
            passwd_owner(b"root:x:0:0:root:/root:/bin/sh\n"),
            Err(ReportReadError::Unavailable)
        );
        for bytes in [
            b"natsume:x:no:1::/:/bin/sh\n".as_slice(),
            b"natsume:x:1:1::/:/bin/sh\nnatsume:x:2:2::/:/bin/sh\n",
            b"natsume:x:1\n",
        ] {
            assert_eq!(passwd_owner(bytes), Err(ReportReadError::Untrusted));
        }
    }

    #[test]
    fn symlinks_permissions_and_wrong_owner_are_rejected() {
        let (directory, owner) = fixture();
        let link = directory.path().join("linked-directory");
        symlink(directory.path(), &link).unwrap_or_else(|error| panic!("symlink: {error}"));
        assert_eq!(
            read_fixture(&link, owner).latest,
            Err(ReportReadError::Untrusted)
        );
        write_report(directory.path(), "target", &report());
        symlink("target", directory.path().join(DIAGNOSTIC_REPORT_NAME))
            .unwrap_or_else(|error| panic!("symlink: {error}"));
        assert_eq!(
            read_fixture(directory.path(), owner).latest,
            Err(ReportReadError::Untrusted)
        );
        fs::remove_file(directory.path().join(DIAGNOSTIC_REPORT_NAME))
            .unwrap_or_else(|error| panic!("remove: {error}"));
        write_report(directory.path(), DIAGNOSTIC_REPORT_NAME, &report());
        for mode in [0o600, 0o666, 0o4644] {
            fs::set_permissions(
                directory.path().join(DIAGNOSTIC_REPORT_NAME),
                fs::Permissions::from_mode(mode),
            )
            .unwrap_or_else(|error| panic!("permissions: {error}"));
            assert_eq!(
                read_fixture(directory.path(), owner).latest,
                Err(ReportReadError::Untrusted)
            );
        }
        write_report(directory.path(), DIAGNOSTIC_REPORT_NAME, &report());
        let handle = trusted_directory(directory.path(), owner, Some(0o755))
            .unwrap_or_else(|error| panic!("directory: {error:?}"));
        assert_eq!(
            read_report(&handle, DIAGNOSTIC_REPORT_NAME, owner.wrapping_add(1), BOOT),
            Err(ReportReadError::Untrusted)
        );
        assert_eq!(
            read_fixture(directory.path(), owner.wrapping_add(1)).latest,
            Err(ReportReadError::Untrusted)
        );
        for mode in [0o700, 0o775, 0o777, 0o1755] {
            fs::set_permissions(directory.path(), fs::Permissions::from_mode(mode))
                .unwrap_or_else(|error| panic!("permissions: {error}"));
            assert_eq!(
                read_fixture(directory.path(), owner).latest,
                Err(ReportReadError::Untrusted)
            );
        }
    }

    #[test]
    fn fifo_and_nonregular_files_do_not_block() {
        let (directory, owner) = fixture();
        rustix::fs::mknodat(
            rustix::fs::CWD,
            directory.path().join(DIAGNOSTIC_REPORT_NAME),
            FileType::Fifo,
            Mode::from_raw_mode(0o644),
            0,
        )
        .unwrap_or_else(|error| panic!("fifo: {error}"));
        assert_eq!(
            read_fixture(directory.path(), owner).latest,
            Err(ReportReadError::Untrusted)
        );
        fs::create_dir(directory.path().join(DIAGNOSTIC_FAILURE_NAME))
            .unwrap_or_else(|error| panic!("directory: {error}"));
        assert_eq!(
            read_fixture(directory.path(), owner).last_failure,
            Err(ReportReadError::Untrusted)
        );
        let device = open("/dev/null", file_flags(), Mode::empty())
            .unwrap_or_else(|error| panic!("device: {error}"));
        assert_eq!(
            check_handle(&device, FileType::RegularFile, owner, None),
            Err(ReportReadError::Untrusted)
        );
    }

    #[test]
    fn oversized_and_growing_reads_are_bounded() {
        let (directory, owner) = fixture();
        write_report(directory.path(), DIAGNOSTIC_REPORT_NAME, &report());
        let path = directory.path().join(DIAGNOSTIC_REPORT_NAME);
        fs::write(&path, vec![b' '; MAX_DIAGNOSTIC_REPORT_BYTES + 1])
            .unwrap_or_else(|error| panic!("write: {error}"));
        assert_eq!(
            read_fixture(directory.path(), owner).latest,
            Err(ReportReadError::TooLarge)
        );
        assert_eq!(
            bounded_read(
                Cursor::new(vec![0; MAX_DIAGNOSTIC_REPORT_BYTES]),
                MAX_DIAGNOSTIC_REPORT_BYTES
            )
            .map(|bytes| bytes.len()),
            Ok(MAX_DIAGNOSTIC_REPORT_BYTES)
        );
        // Open/stat a short inode, then grow it before reading: the bound is not
        // based on the earlier stat or EOF and remains enforced on this handle.
        let mut file = File::options()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap_or_else(|error| panic!("open: {error}"));
        file.set_len(1)
            .unwrap_or_else(|error| panic!("truncate: {error}"));
        assert_eq!(file.metadata().map(|metadata| metadata.len()).ok(), Some(1));
        file.write_all(&vec![b' '; MAX_DIAGNOSTIC_REPORT_BYTES * 2])
            .unwrap_or_else(|error| panic!("grow: {error}"));
        file.seek(SeekFrom::Start(0))
            .unwrap_or_else(|error| panic!("seek: {error}"));
        assert_eq!(
            bounded_read(file, MAX_DIAGNOSTIC_REPORT_BYTES),
            Err(ReportReadError::TooLarge)
        );
        assert_eq!(
            bounded_read(std::io::repeat(b'x'), MAX_DIAGNOSTIC_REPORT_BYTES),
            Err(ReportReadError::TooLarge)
        );
        assert_eq!(
            bounded_read(std::io::repeat(b'x'), MAX_PASSWD_BYTES),
            Err(ReportReadError::TooLarge)
        );
    }

    #[test]
    fn pinned_directory_and_file_survive_path_replacement() {
        let (directory, owner) = fixture();
        let original = populated_report();
        write_report(directory.path(), DIAGNOSTIC_REPORT_NAME, &original);
        let handle = trusted_directory(directory.path(), owner, Some(0o755))
            .unwrap_or_else(|error| panic!("open: {error:?}"));
        let file = openat(&handle, DIAGNOSTIC_REPORT_NAME, file_flags(), Mode::empty())
            .unwrap_or_else(|error| panic!("open: {error}"));
        fs::rename(
            directory.path().join(DIAGNOSTIC_REPORT_NAME),
            directory.path().join("old"),
        )
        .unwrap_or_else(|error| panic!("rename: {error}"));
        symlink("missing", directory.path().join(DIAGNOSTIC_REPORT_NAME))
            .unwrap_or_else(|error| panic!("symlink: {error}"));
        let bytes = bounded_read(File::from(file), MAX_DIAGNOSTIC_REPORT_BYTES)
            .unwrap_or_else(|error| panic!("read: {error:?}"));
        assert_eq!(decode_report(&bytes, BOOT), Ok(original));
        assert_eq!(
            read_from_directory(&handle, owner, BOOT).latest,
            Err(ReportReadError::Untrusted)
        );
        let moved = directory.path().with_extension("moved");
        fs::rename(directory.path(), &moved).unwrap_or_else(|error| panic!("rename: {error}"));
        fs::create_dir(directory.path()).unwrap_or_else(|error| panic!("replacement: {error}"));
        write_report(directory.path(), DIAGNOSTIC_REPORT_NAME, &report());
        assert_eq!(
            read_from_directory(&handle, owner, BOOT).latest,
            Err(ReportReadError::Untrusted)
        );
        fs::remove_dir_all(moved).unwrap_or_else(|error| panic!("cleanup: {error}"));
    }

    #[test]
    fn malformed_unknown_schema_and_unsafe_strings_are_rejected() {
        assert_eq!(
            decode_report(b"not JSON", BOOT),
            Err(ReportReadError::Invalid)
        );
        let base = serde_json::to_value(populated_report())
            .unwrap_or_else(|error| panic!("serialize: {error}"));
        for (pointer, replacement) in [
            ("/boot_id", "7138AED1-5921-4C4B-8E35-110806942DEF"),
            ("/identity/machine_hardware_id", BOOT),
            ("/connection/device_id", BOOT),
            ("/connection/endpoint_ip", "https://secret.invalid"),
            ("/connection/local_ip", "127.0.0.1\nexception"),
            ("/connection/network/next_hop", "fe80::1%eth0"),
            ("/connection/network/interface", "../../secret"),
            ("/connection/network/interface", "interface-too-long"),
            ("/connection/network/interface", ".."),
            ("/connection/trust_root_sha256", "secret"),
            ("/connection/peer_error_code", "arbitrary exception text"),
            ("/daemon_instance_id", "exception text"),
            ("/daemon_instance_id", "4294967296-1"),
            ("/daemon_version", "exception text"),
            ("/control_protocol", "another.protocol"),
            ("/status", "healthy_now"),
            ("/stage", "invented_stage"),
            ("/identity/sources/dmi_system_uuid", "raw_secret"),
        ] {
            let mut value = base.clone();
            *value
                .pointer_mut(pointer)
                .unwrap_or_else(|| panic!("pointer: {pointer}")) = replacement.into();
            let bytes =
                serde_json::to_vec(&value).unwrap_or_else(|error| panic!("serialize: {error}"));
            assert_eq!(
                decode_report(&bytes, BOOT),
                Err(ReportReadError::Invalid),
                "{pointer}: {replacement}"
            );
        }
        for pointer in [
            "",
            "/identity",
            "/identity/sources",
            "/connection",
            "/connection/network",
        ] {
            let mut value = base.clone();
            value
                .pointer_mut(pointer)
                .and_then(serde_json::Value::as_object_mut)
                .unwrap_or_else(|| panic!("object: {pointer}"))
                .insert("exception".to_owned(), "SECRET_SENTINEL".into());
            let bytes =
                serde_json::to_vec(&value).unwrap_or_else(|error| panic!("serialize: {error}"));
            assert_eq!(
                decode_report(&bytes, BOOT),
                Err(ReportReadError::Invalid),
                "{pointer}"
            );
        }
        let mut unsafe_report = report();
        unsafe_report.daemon_version = "2.8.0+".to_owned() + &"a".repeat(65);
        assert_eq!(qr_payload(&unsafe_report), Err(QrError::InvalidReport));
    }

    #[test]
    fn wrong_boot_version_and_event_bounds_are_rejected() {
        let mut candidate = report();
        candidate.boot_id = Some(OTHER_BOOT.to_owned());
        let encode = |report: &DiagnosticReport| {
            serde_json::to_vec(report).unwrap_or_else(|error| panic!("serialize: {error}"))
        };
        assert_eq!(
            decode_report(&encode(&candidate), BOOT),
            Err(ReportReadError::WrongBoot)
        );
        candidate.boot_id = None;
        assert_eq!(
            decode_report(&encode(&candidate), BOOT),
            Err(ReportReadError::WrongBoot)
        );
        candidate = report();
        assert_eq!(
            decode_report(&encode(&candidate), "unknown"),
            Err(ReportReadError::WrongBoot)
        );
        candidate.format_version = 2;
        assert_eq!(
            decode_report(&encode(&candidate), BOOT),
            Err(ReportReadError::UnsupportedVersion)
        );
        candidate = report();
        candidate.events = vec![event(); MAX_DIAGNOSTIC_EVENTS];
        assert_eq!(
            decode_report(&encode(&candidate), BOOT),
            Ok(candidate.clone())
        );
        candidate.events.push(event());
        assert_eq!(
            decode_report(&encode(&candidate), BOOT),
            Err(ReportReadError::Invalid)
        );
    }

    fn event() -> DiagnosticEvent {
        DiagnosticEvent {
            at_boot_ms: Some(u64::MAX),
            stage: DiagnosticStage::AuthorityInstallation,
            status: DiagnosticStatus::Retrying,
            code: Some(DiagnosticCode::ControlManifestSerializationFailed),
        }
    }

    #[test]
    fn latest_and_last_failure_are_separate_whole_reports() {
        let (directory, owner) = fixture();
        let latest = report();
        let mut failure = populated_report();
        failure.daemon_instance_id = "b123456789abcdef0123456789abcdef".to_owned();
        failure.status = DiagnosticStatus::Stopped;
        write_report(directory.path(), DIAGNOSTIC_REPORT_NAME, &latest);
        write_report(directory.path(), DIAGNOSTIC_FAILURE_NAME, &failure);
        let evidence = read_fixture(directory.path(), owner);
        assert_eq!(
            evidence.selected_report(),
            Some((&latest, ReportSource::Latest))
        );
        assert_eq!(evidence.last_failure, Ok(failure.clone()));
        for error in [
            ReportReadError::Missing,
            ReportReadError::Unavailable,
            ReportReadError::Untrusted,
            ReportReadError::TooLarge,
            ReportReadError::Invalid,
            ReportReadError::UnsupportedVersion,
            ReportReadError::WrongBoot,
        ] {
            let fallback = DiagnosticEvidence {
                latest: Err(error),
                last_failure: Ok(failure.clone()),
            };
            assert_eq!(
                fallback.selected_report(),
                Some((&failure, ReportSource::LastFailure))
            );
        }
    }

    #[test]
    fn maximal_metadata_fits_capacity_after_event_fallback() {
        use natsume_workstation_ipc::MachineIdentityQuality;
        use natsume_workstation_ipc::diagnostics::{
            DiagnosticConnectionPurpose, DiagnosticFailure, DiagnosticHandshakeStep,
            DiagnosticLinkState, DiagnosticRouteState,
        };

        let mut candidate = populated_report();
        candidate.daemon_version = "2.8.0+".to_owned() + &"a".repeat(58);
        candidate.sequence = u64::MAX;
        candidate.captured_at_unix_ms = Some(u64::MAX);
        candidate.captured_at_boot_ms = Some(u64::MAX);
        candidate.last_completed_stage = Some(DiagnosticStage::AuthorityInstallation);
        candidate.stage = DiagnosticStage::ControlConfiguration;
        candidate.status = DiagnosticStatus::WaitingForReview;
        candidate.failure = Some(DiagnosticFailure {
            code: DiagnosticCode::ControlManifestSerializationFailed,
            first_at_boot_ms: Some(u64::MAX),
            last_at_boot_ms: Some(u64::MAX),
            consecutive_count: u64::MAX,
        });
        candidate.identity.quality = Some(MachineIdentityQuality::Strong);
        let connection = &mut candidate.connection;
        let longest_ip = "ffff:ffff:ffff:ffff:ffff:ffff:255.255.255.255";
        connection.endpoint_ip = Some(longest_ip.to_owned());
        connection.local_ip = Some(longest_ip.to_owned());
        connection.endpoint_port = Some(u16::MAX);
        connection.network.next_hop = Some(longest_ip.to_owned());
        connection.network.interface = Some("interface123456".to_owned());
        connection.network.sampled_at_boot_ms = Some(u64::MAX);
        connection.network.route = DiagnosticRouteState::NoMatchingObservedRoute;
        connection.network.carrier = DiagnosticLinkState::Down;
        connection.network.administratively_up = Some(false);
        connection.peer_error_code = Some("A".repeat(64));
        connection.attempt = u64::MAX;
        connection.attempt_duration_ms = Some(u64::MAX);
        connection.retry_delay_ms = Some(u64::MAX);
        connection.http_status = Some(u16::MAX);
        connection.purpose = Some(DiagnosticConnectionPurpose::Enrollment);
        connection.handshake_step = Some(DiagnosticHandshakeStep::AuthorityInstalled);
        assert_eq!(validate_report(&candidate), Ok(()));
        candidate.events = vec![event(); MAX_DIAGNOSTIC_EVENTS];
        let payload =
            qr_payload(&candidate).unwrap_or_else(|error| panic!("bounded metadata: {error:?}"));
        assert!(payload.len() <= MAX_QR_PAYLOAD_BYTES);
        candidate.events.clear();
        candidate.events_truncated = true;
        assert_eq!(
            serde_json::from_str::<DiagnosticReport>(&payload).ok(),
            Some(candidate)
        );
    }

    #[test]
    fn qr_fallback_capacity_and_four_module_quiet_zone() {
        let mut original = populated_report();
        original.events = vec![event(); MAX_DIAGNOSTIC_EVENTS];
        assert!(
            serde_json::to_vec(&original).map_or(0, |bytes| bytes.len()) > MAX_QR_PAYLOAD_BYTES
        );
        let payload = qr_payload(&original).unwrap_or_else(|error| panic!("payload: {error:?}"));
        assert!(payload.len() <= MAX_QR_PAYLOAD_BYTES);
        let mut reduced = original.clone();
        reduced.events.clear();
        reduced.events_truncated = true;
        assert_eq!(
            serde_json::from_str::<DiagnosticReport>(&payload).ok(),
            Some(reduced)
        );
        assert_eq!(original.events.len(), MAX_DIAGNOSTIC_EVENTS);
        let buffer = qr_pixels(&payload).unwrap_or_else(|error| panic!("pixels: {error:?}"));
        let qr = QrCode::with_error_correction_level(payload.as_bytes(), EcLevel::L)
            .unwrap_or_else(|error| panic!("qr: {error}"));
        let side = buffer.width() as usize;
        assert_eq!(side, (qr.width() + 8) * MODULE_PIXELS);
        for y in 0..side {
            for x in 0..side {
                let mx = x / MODULE_PIXELS;
                let my = y / MODULE_PIXELS;
                let expected_dark = mx >= 4
                    && my >= 4
                    && mx < qr.width() + 4
                    && my < qr.width() + 4
                    && qr[(mx - 4, my - 4)] == Color::Dark;
                assert_eq!(
                    buffer.as_slice()[y * side + x],
                    if expected_dark {
                        Rgb8Pixel::new(0x11, 0x11, 0x11)
                    } else {
                        Rgb8Pixel::new(0xf5, 0xf5, 0xf3)
                    }
                );
            }
        }
        assert!(qr_image(&payload).is_ok());
        assert!(qr_image(&"x".repeat(MAX_QR_PAYLOAD_BYTES)).is_ok());
        assert!(matches!(
            qr_image(&"x".repeat(MAX_QR_PAYLOAD_BYTES + 1)),
            Err(QrError::TooLarge)
        ));
    }
}
