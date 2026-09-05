use std::{
    ffi::{CString, OsStr},
    fs::{self, File, OpenOptions},
    io::{Read as _, Write as _},
    os::unix::fs::{MetadataExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
};

use natsume_local_control_api::{HomeResetPhase, HomeResetProgress};
use procfs::process::Process;
use rustix::{
    fs::{Gid, Uid, chown},
    mount::{MountFlags, UnmountFlags, mount, unmount},
};

const TEMPLATE_RELATIVE_PATH: &str = "usr/lib/natsume/home-templates/current/lower";
const STATE_RELATIVE_PATH: &str = "var/lib/natsume-privileged/home-reset";
const CONTEST_HOME_RELATIVE_PATH: &str = "home/contest";

fn service_error(message: &'static str) -> zbus::fdo::Error {
    zbus::fdo::Error::Failed(message.to_owned())
}

fn require_epoch(epoch: u64) -> zbus::fdo::Result<()> {
    if epoch == 0 || i64::try_from(epoch).is_err() {
        return Err(zbus::fdo::Error::InvalidArgs(
            "reset epoch must be in 1..=i64::MAX".to_owned(),
        ));
    }
    Ok(())
}

fn state_directory(root: &Path) -> PathBuf {
    root.join(STATE_RELATIVE_PATH)
}

pub(super) fn state_exists(root: &Path) -> zbus::fdo::Result<bool> {
    let mut entries = match fs::read_dir(state_directory(root)) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(_) => return Err(service_error("Home reset state cannot be inspected")),
    };
    match entries.next() {
        Some(Ok(_)) => Ok(true),
        Some(Err(_)) => Err(service_error("Home reset state cannot be inspected")),
        None => Ok(false),
    }
}

fn generation_directory(root: &Path, epoch: u64) -> PathBuf {
    state_directory(root)
        .join("generations")
        .join(epoch.to_string())
}

fn marker_path(root: &Path) -> PathBuf {
    state_directory(root).join("progress")
}

fn directory_metadata(path: &Path) -> Option<fs::Metadata> {
    fs::symlink_metadata(path)
        .ok()
        .filter(|metadata| metadata.file_type().is_dir())
}

fn ensure_directory(path: &Path) -> zbus::fdo::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(()),
        Ok(_) => Err(service_error("Home reset generation path is invalid")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => fs::create_dir(path)
            .map_err(|_| service_error("Home reset generation cannot be prepared")),
        Err(_) => Err(service_error("Home reset generation cannot be prepared")),
    }
}

fn sync_directory(path: &Path) -> zbus::fdo::Result<()> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| service_error("Home reset generation cannot be persisted"))
}

fn generation_layout_exists(root: &Path, epoch: u64) -> bool {
    let generation = generation_directory(root, epoch);
    directory_metadata(&generation).is_some()
        && directory_metadata(&generation.join("upper")).is_some()
        && directory_metadata(&generation.join("work")).is_some()
}

fn generation_is_complete(root: &Path, epoch: u64, home: &fs::Metadata) -> bool {
    if !generation_layout_exists(root, epoch) {
        return false;
    }
    directory_metadata(&generation_directory(root, epoch).join("upper")).is_some_and(|upper| {
        upper.uid() == home.uid()
            && upper.gid() == home.gid()
            && upper.mode() & 0o7777 == home.mode() & 0o7777
    })
}

fn remove_other_generations(root: &Path, current_epoch: u64) -> zbus::fdo::Result<()> {
    let generations = state_directory(root).join("generations");
    let current = current_epoch.to_string();
    let entries = fs::read_dir(&generations)
        .map_err(|_| service_error("Old Home reset generations cannot be removed"))?;
    for entry in entries {
        let entry =
            entry.map_err(|_| service_error("Old Home reset generations cannot be removed"))?;
        if entry.file_name().as_os_str() == OsStr::new(&current) {
            continue;
        }
        if !entry
            .file_type()
            .map_err(|_| service_error("Old Home reset generations cannot be removed"))?
            .is_dir()
        {
            return Err(service_error(
                "Old Home reset generations cannot be removed",
            ));
        }
        fs::remove_dir_all(entry.path())
            .map_err(|_| service_error("Old Home reset generations cannot be removed"))?;
    }
    sync_directory(&generations)
}

fn prepare_generation(root: &Path, epoch: u64, home: &fs::Metadata) -> zbus::fdo::Result<()> {
    let state = state_directory(root);
    if directory_metadata(&state).is_none() {
        return Err(service_error("Home reset state directory is unavailable"));
    }
    let generations = state.join("generations");
    ensure_directory(&generations)?;
    let generation = generations.join(epoch.to_string());
    ensure_directory(&generation)?;
    let upper = generation.join("upper");
    let work = generation.join("work");
    ensure_directory(&upper)?;
    ensure_directory(&work)?;

    let upper_metadata = directory_metadata(&upper)
        .ok_or_else(|| service_error("Home reset generation cannot be prepared"))?;
    if upper_metadata.uid() != home.uid() || upper_metadata.gid() != home.gid() {
        chown(
            &upper,
            Some(Uid::from_raw(home.uid())),
            Some(Gid::from_raw(home.gid())),
        )
        .map_err(|_| service_error("Home reset generation cannot be prepared"))?;
    }
    fs::set_permissions(&upper, fs::Permissions::from_mode(home.mode() & 0o7777))
        .map_err(|_| service_error("Home reset generation cannot be prepared"))?;

    for directory in [&upper, &work, &generation, &generations, &state] {
        sync_directory(directory)?;
    }
    Ok(())
}

fn phase_name(phase: HomeResetPhase) -> &'static str {
    match phase {
        HomeResetPhase::Prepared => "prepared",
        HomeResetPhase::Applied => "applied",
        HomeResetPhase::Verified => "verified",
        HomeResetPhase::RecoveryRequired => "recovery_required",
    }
}

fn parse_phase(value: &str) -> Option<HomeResetPhase> {
    match value {
        "prepared" => Some(HomeResetPhase::Prepared),
        "applied" => Some(HomeResetPhase::Applied),
        "verified" => Some(HomeResetPhase::Verified),
        "recovery_required" => Some(HomeResetPhase::RecoveryRequired),
        _ => None,
    }
}

fn read_progress(root: &Path) -> zbus::fdo::Result<Option<HomeResetProgress>> {
    let mut file = match File::open(marker_path(root)) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(service_error("Home reset progress is unreadable")),
    };
    let mut encoded = String::new();
    file.read_to_string(&mut encoded)
        .map_err(|_| service_error("Home reset progress is unreadable"))?;
    let mut lines = encoded.lines();
    let epoch = lines
        .next()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|epoch| *epoch != 0 && i64::try_from(*epoch).is_ok())
        .ok_or_else(|| service_error("Home reset progress is invalid"))?;
    let phase = lines
        .next()
        .and_then(parse_phase)
        .ok_or_else(|| service_error("Home reset progress is invalid"))?;
    if lines.next().is_some() {
        return Err(service_error("Home reset progress is invalid"));
    }
    Ok(Some(HomeResetProgress {
        reset_epoch: epoch,
        phase,
    }))
}

fn write_progress(root: &Path, epoch: u64, phase: HomeResetPhase) -> zbus::fdo::Result<()> {
    let directory = state_directory(root);
    if directory_metadata(&directory).is_none() {
        return Err(service_error("Home reset progress cannot be persisted"));
    }
    let temporary = directory.join("progress.tmp");
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&temporary)
        .map_err(|_| service_error("Home reset progress cannot be persisted"))?;
    write!(file, "{}\n{}\n", epoch, phase_name(phase))
        .map_err(|_| service_error("Home reset progress cannot be persisted"))?;
    file.sync_all()
        .map_err(|_| service_error("Home reset progress cannot be persisted"))?;
    fs::rename(temporary, marker_path(root))
        .map_err(|_| service_error("Home reset progress cannot be persisted"))?;
    File::open(directory)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| service_error("Home reset progress cannot be persisted"))
}

fn require_target_progress(root: &Path, epoch: u64) -> zbus::fdo::Result<HomeResetPhase> {
    let progress =
        read_progress(root)?.ok_or_else(|| service_error("Home reset has not been prepared"))?;
    if progress.reset_epoch != epoch {
        return Err(service_error("another Home reset epoch is in progress"));
    }
    Ok(progress.phase)
}

pub(super) fn prepare(root: &Path, epoch: u64) -> zbus::fdo::Result<()> {
    require_epoch(epoch)?;
    let template = root.join(TEMPLATE_RELATIVE_PATH);
    if !template.is_dir() {
        return Err(service_error("managed Home template is unavailable"));
    }
    let contest_home = root.join(CONTEST_HOME_RELATIVE_PATH);
    let home_metadata = fs::metadata(&contest_home)
        .ok()
        .filter(std::fs::Metadata::is_dir)
        .ok_or_else(|| service_error("contestant Home is unavailable"))?;
    if let Some(progress) = read_progress(root)? {
        if progress.reset_epoch == epoch {
            return match progress.phase {
                HomeResetPhase::Prepared => prepare_generation(root, epoch, &home_metadata),
                HomeResetPhase::Applied | HomeResetPhase::Verified
                    if generation_is_complete(root, epoch, &home_metadata) =>
                {
                    Ok(())
                }
                HomeResetPhase::Applied | HomeResetPhase::Verified => {
                    write_progress(root, epoch, HomeResetPhase::RecoveryRequired)?;
                    Err(service_error("Home reset recovery is required"))
                }
                HomeResetPhase::RecoveryRequired => {
                    Err(service_error("Home reset recovery is required"))
                }
            };
        }
        if progress.phase != HomeResetPhase::Verified {
            return Err(service_error("another Home reset epoch is in progress"));
        }
    }
    prepare_generation(root, epoch, &home_metadata)?;
    write_progress(root, epoch, HomeResetPhase::Prepared)
}

pub(super) fn query(root: &Path) -> zbus::fdo::Result<Option<HomeResetProgress>> {
    read_progress(root)
}

fn mounted_generation(root: &Path, epoch: u64) -> zbus::fdo::Result<bool> {
    let process = Process::myself().map_err(|_| service_error("mount state is unavailable"))?;
    let mounts = process
        .mountinfo()
        .map_err(|_| service_error("mount state is unavailable"))?;
    let mount_point = root.join(CONTEST_HOME_RELATIVE_PATH);
    let mut at_target = mounts
        .iter()
        .filter(|mount| mount.mount_point == mount_point);
    let Some(mount) = at_target.next() else {
        return Ok(false);
    };
    Ok(at_target.next().is_none() && natsume_generation(root, mount) == Some(epoch))
}

fn natsume_generation(root: &Path, mount: &procfs::process::MountInfo) -> Option<u64> {
    if mount.mount_point != root.join(CONTEST_HOME_RELATIVE_PATH) || mount.fs_type != "overlay" {
        return None;
    }
    let lower = mount
        .super_options
        .get("lowerdir")
        .and_then(Option::as_deref)?;
    if Path::new(lower) != root.join(TEMPLATE_RELATIVE_PATH) {
        return None;
    }
    let generations = state_directory(root).join("generations");
    let upper = Path::new(
        mount
            .super_options
            .get("upperdir")
            .and_then(Option::as_deref)?,
    );
    let mut relative = upper.strip_prefix(&generations).ok()?.components();
    let epoch = relative.next()?.as_os_str().to_str()?.parse::<u64>().ok()?;
    if epoch == 0
        || i64::try_from(epoch).is_err()
        || relative.next()?.as_os_str() != "upper"
        || relative.next().is_some()
        || mount
            .super_options
            .get("workdir")
            .and_then(Option::as_deref)
            .map(Path::new)
            != Some(generations.join(epoch.to_string()).join("work").as_path())
    {
        return None;
    }
    Some(epoch)
}

fn mount_generation(root: &Path, epoch: u64) -> zbus::fdo::Result<()> {
    if !generation_layout_exists(root, epoch) {
        return Err(service_error("Home reset generation is unavailable"));
    }
    if root != Path::new("/") {
        return Ok(());
    }
    if mounted_generation(root, epoch)? {
        return Ok(());
    }
    let mount_point = root.join(CONTEST_HOME_RELATIVE_PATH);
    let process = Process::myself().map_err(|_| service_error("mount state is unavailable"))?;
    let mounts = process
        .mountinfo()
        .map_err(|_| service_error("mount state is unavailable"))?;
    let mut at_target = mounts
        .iter()
        .filter(|mount| mount.mount_point == mount_point);
    if let Some(existing) = at_target.next() {
        if at_target.next().is_some() || natsume_generation(root, existing).is_none() {
            return Err(service_error("contestant Home contains an unmanaged mount"));
        }
        unmount(&mount_point, UnmountFlags::empty())
            .map_err(|_| service_error("contestant Home cannot be unmounted"))?;
    }

    let generation = generation_directory(root, epoch);
    let options = format!(
        "lowerdir={},upperdir={},workdir={}",
        root.join(TEMPLATE_RELATIVE_PATH).display(),
        generation.join("upper").display(),
        generation.join("work").display()
    );
    let options = CString::new(options)
        .map_err(|_| service_error("contestant Home mount options are invalid"))?;
    // TODO(WP10): validate the fixed overlay layout on the signed client image.
    mount(
        "overlay",
        mount_point,
        "overlay",
        MountFlags::empty(),
        Some(options.as_c_str()),
    )
    .map_err(|_| service_error("contestant Home cannot be mounted"))
}

pub(super) fn apply(root: &Path, epoch: u64) -> zbus::fdo::Result<()> {
    require_epoch(epoch)?;
    match require_target_progress(root, epoch)? {
        HomeResetPhase::Prepared | HomeResetPhase::RecoveryRequired => {
            mount_generation(root, epoch)?;
            write_progress(root, epoch, HomeResetPhase::Applied)
        }
        HomeResetPhase::Applied | HomeResetPhase::Verified => Ok(()),
    }
}

fn record_verification(
    root: &Path,
    epoch: u64,
    mounted: bool,
) -> zbus::fdo::Result<HomeResetProgress> {
    let phase = require_target_progress(root, epoch)?;
    if phase == HomeResetPhase::Prepared || !mounted {
        write_progress(root, epoch, HomeResetPhase::RecoveryRequired)?;
        return Ok(HomeResetProgress {
            reset_epoch: epoch,
            phase: HomeResetPhase::RecoveryRequired,
        });
    }
    if phase != HomeResetPhase::Verified {
        remove_other_generations(root, epoch)?;
        write_progress(root, epoch, HomeResetPhase::Verified)?;
    }
    Ok(HomeResetProgress {
        reset_epoch: epoch,
        phase: HomeResetPhase::Verified,
    })
}

pub(super) fn verify(root: &Path, epoch: u64) -> zbus::fdo::Result<HomeResetProgress> {
    require_epoch(epoch)?;
    record_verification(root, epoch, mounted_generation(root, epoch)?)
}

pub(super) fn recover(root: &Path, epoch: u64) -> zbus::fdo::Result<()> {
    require_epoch(epoch)?;
    match read_progress(root)? {
        Some(progress)
            if progress.reset_epoch == epoch
                && progress.phase == HomeResetPhase::Verified
                && mounted_generation(root, epoch)? =>
        {
            Ok(())
        }
        Some(progress) if progress.reset_epoch == epoch => {
            mount_generation(root, epoch)?;
            write_progress(root, epoch, HomeResetPhase::Applied)
        }
        Some(_) => Err(service_error("another Home reset epoch is in progress")),
        None if generation_layout_exists(root, epoch) => {
            write_progress(root, epoch, HomeResetPhase::Prepared)?;
            mount_generation(root, epoch)?;
            write_progress(root, epoch, HomeResetPhase::Applied)
        }
        None => Err(service_error("Home reset has not been prepared")),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        os::unix::fs::{MetadataExt as _, PermissionsExt as _, symlink},
        path::Path,
    };

    use natsume_local_control_api::HomeResetPhase;
    use procfs::process::MountInfo;
    use tempfile::TempDir;

    use super::{
        apply, generation_directory, natsume_generation, prepare, query, record_verification,
        recover, state_directory, verify,
    };

    fn fixture() -> TempDir {
        let fixture =
            TempDir::new().unwrap_or_else(|error| panic!("fixture creation failed: {error}"));
        let lower = fixture
            .path()
            .join("usr/lib/natsume/home-templates/current/lower");
        fs::create_dir_all(&lower)
            .unwrap_or_else(|error| panic!("template fixture failed: {error}"));
        fs::set_permissions(&lower, fs::Permissions::from_mode(0o750))
            .unwrap_or_else(|error| panic!("template metadata fixture failed: {error}"));
        let home = fixture.path().join("home/contest");
        fs::create_dir_all(&home)
            .unwrap_or_else(|error| panic!("contestant Home fixture failed: {error}"));
        fs::set_permissions(&home, fs::Permissions::from_mode(0o700))
            .unwrap_or_else(|error| panic!("contestant Home metadata fixture failed: {error}"));
        fs::create_dir_all(state_directory(fixture.path()))
            .unwrap_or_else(|error| panic!("Home reset state fixture failed: {error}"));
        fixture
    }

    #[test]
    fn prepared_upper_inherits_the_contestant_home_metadata() {
        let fixture = fixture();
        prepare(fixture.path(), 7).unwrap_or_else(|error| panic!("prepare failed: {error}"));
        let home = fs::metadata(fixture.path().join("home/contest"))
            .unwrap_or_else(|error| panic!("contestant Home metadata failed: {error}"));
        let upper = fs::metadata(generation_directory(fixture.path(), 7).join("upper"))
            .unwrap_or_else(|error| panic!("upper metadata failed: {error}"));

        assert_eq!(upper.uid(), home.uid());
        assert_eq!(upper.gid(), home.gid());
        assert_eq!(upper.mode() & 0o7777, home.mode() & 0o7777);
    }

    #[test]
    fn home_reset_is_prepare_apply_verify_and_idempotent() {
        let fixture = fixture();
        prepare(fixture.path(), 7).unwrap_or_else(|error| panic!("prepare failed: {error}"));
        prepare(fixture.path(), 7)
            .unwrap_or_else(|error| panic!("replayed prepare failed: {error}"));
        let prepared = query(fixture.path())
            .unwrap_or_else(|error| panic!("query failed: {error}"))
            .unwrap_or_else(|| panic!("prepared reset must be observable"));
        assert_eq!(prepared.reset_epoch, 7);
        assert_eq!(prepared.phase, HomeResetPhase::Prepared);
        apply(fixture.path(), 7).unwrap_or_else(|error| panic!("apply failed: {error}"));
        let progress = record_verification(fixture.path(), 7, true)
            .unwrap_or_else(|error| panic!("verification recording failed: {error}"));

        assert_eq!(progress.reset_epoch, 7);
        assert_eq!(progress.phase, HomeResetPhase::Verified);
        apply(fixture.path(), 7).unwrap_or_else(|error| panic!("replayed apply failed: {error}"));
        let replay = record_verification(fixture.path(), 7, true)
            .unwrap_or_else(|error| panic!("replayed verification failed: {error}"));
        assert_eq!(replay.phase, HomeResetPhase::Verified);
    }

    #[test]
    fn verified_reset_removes_the_previous_generation() {
        let fixture = fixture();
        prepare(fixture.path(), 7).unwrap_or_else(|error| panic!("prepare failed: {error}"));
        apply(fixture.path(), 7).unwrap_or_else(|error| panic!("apply failed: {error}"));
        record_verification(fixture.path(), 7, true)
            .unwrap_or_else(|error| panic!("verification failed: {error}"));

        prepare(fixture.path(), 8).unwrap_or_else(|error| panic!("prepare failed: {error}"));
        apply(fixture.path(), 8).unwrap_or_else(|error| panic!("apply failed: {error}"));
        record_verification(fixture.path(), 8, true)
            .unwrap_or_else(|error| panic!("verification failed: {error}"));

        assert!(!generation_directory(fixture.path(), 7).exists());
        assert!(generation_directory(fixture.path(), 8).is_dir());
    }

    #[test]
    fn prepared_replay_rebuilds_a_missing_generation() {
        let fixture = fixture();
        prepare(fixture.path(), 8).unwrap_or_else(|error| panic!("prepare failed: {error}"));
        fs::remove_dir_all(generation_directory(fixture.path(), 8))
            .unwrap_or_else(|error| panic!("generation removal failed: {error}"));

        prepare(fixture.path(), 8)
            .unwrap_or_else(|error| panic!("prepared replay failed: {error}"));

        assert!(
            generation_directory(fixture.path(), 8)
                .join("upper")
                .is_dir()
        );
        assert!(
            generation_directory(fixture.path(), 8)
                .join("work")
                .is_dir()
        );
    }

    #[test]
    fn applied_replay_marks_an_incomplete_generation_for_recovery() {
        let fixture = fixture();
        prepare(fixture.path(), 8).unwrap_or_else(|error| panic!("prepare failed: {error}"));
        apply(fixture.path(), 8).unwrap_or_else(|error| panic!("apply failed: {error}"));
        fs::remove_dir(generation_directory(fixture.path(), 8).join("work"))
            .unwrap_or_else(|error| panic!("work directory removal failed: {error}"));

        assert!(prepare(fixture.path(), 8).is_err());
        let progress = query(fixture.path())
            .unwrap_or_else(|error| panic!("query failed: {error}"))
            .unwrap_or_else(|| panic!("recovery state must be observable"));
        assert_eq!(progress.phase, HomeResetPhase::RecoveryRequired);
    }

    #[test]
    fn symlinked_generation_tree_is_rejected() {
        let fixture = fixture();
        let outside = fixture.path().join("outside");
        fs::create_dir(&outside).unwrap_or_else(|error| panic!("outside fixture failed: {error}"));
        symlink(
            &outside,
            state_directory(fixture.path()).join("generations"),
        )
        .unwrap_or_else(|error| panic!("symlink fixture failed: {error}"));

        assert!(prepare(fixture.path(), 8).is_err());
        assert!(!outside.join("8").exists());
    }

    #[test]
    fn applied_marker_is_not_mount_evidence_in_a_fixture() {
        let fixture = fixture();
        prepare(fixture.path(), 8).unwrap_or_else(|error| panic!("prepare failed: {error}"));
        apply(fixture.path(), 8).unwrap_or_else(|error| panic!("apply failed: {error}"));

        let progress =
            verify(fixture.path(), 8).unwrap_or_else(|error| panic!("verify failed: {error}"));

        assert_eq!(progress.phase, HomeResetPhase::RecoveryRequired);
    }

    #[test]
    fn recovery_resumes_a_prepared_generation() {
        let fixture = fixture();
        prepare(fixture.path(), 9).unwrap_or_else(|error| panic!("prepare failed: {error}"));
        recover(fixture.path(), 9).unwrap_or_else(|error| panic!("recover failed: {error}"));

        let progress = record_verification(fixture.path(), 9, true)
            .unwrap_or_else(|error| panic!("verification recording failed: {error}"));
        assert_eq!(progress.phase, HomeResetPhase::Verified);
    }

    #[test]
    fn lost_mount_requires_recovery_before_verification_can_complete_again() {
        let fixture = fixture();
        prepare(fixture.path(), 12).unwrap_or_else(|error| panic!("prepare failed: {error}"));
        apply(fixture.path(), 12).unwrap_or_else(|error| panic!("apply failed: {error}"));
        let lost = record_verification(fixture.path(), 12, false)
            .unwrap_or_else(|error| panic!("failed verification failed: {error}"));
        assert_eq!(lost.phase, HomeResetPhase::RecoveryRequired);

        recover(fixture.path(), 12).unwrap_or_else(|error| panic!("recover failed: {error}"));
        let recovered = record_verification(fixture.path(), 12, true)
            .unwrap_or_else(|error| panic!("reverification failed: {error}"));
        assert_eq!(recovered.phase, HomeResetPhase::Verified);
    }

    #[test]
    fn a_new_epoch_waits_for_the_current_epoch_to_finish() {
        let fixture = fixture();
        prepare(fixture.path(), 10).unwrap_or_else(|error| panic!("prepare failed: {error}"));

        assert!(prepare(fixture.path(), 11).is_err());
    }

    #[test]
    fn only_a_natsume_overlay_generation_is_replaceable() {
        let managed = MountInfo::from_line(
            "36 25 0:42 / /home/contest rw,relatime - overlay overlay rw,lowerdir=/usr/lib/natsume/home-templates/current/lower,upperdir=/var/lib/natsume-privileged/home-reset/generations/7/upper,workdir=/var/lib/natsume-privileged/home-reset/generations/7/work",
        )
        .unwrap_or_else(|error| panic!("managed mount fixture failed: {error}"));
        assert_eq!(natsume_generation(Path::new("/"), &managed), Some(7));

        let unknown =
            MountInfo::from_line("36 25 8:2 / /home/contest rw,relatime - ext4 /dev/sda2 rw")
                .unwrap_or_else(|error| panic!("unknown mount fixture failed: {error}"));
        assert_eq!(natsume_generation(Path::new("/"), &unknown), None);

        let foreign_overlay = MountInfo::from_line(
            "36 25 0:43 / /home/contest rw,relatime - overlay overlay rw,lowerdir=/srv/other,upperdir=/var/lib/natsume-privileged/home-reset/generations/7/upper,workdir=/var/lib/natsume-privileged/home-reset/generations/7/work",
        )
        .unwrap_or_else(|error| panic!("foreign overlay fixture failed: {error}"));
        assert_eq!(natsume_generation(Path::new("/"), &foreign_overlay), None);
    }
}
