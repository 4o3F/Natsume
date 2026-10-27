//! Fixed native reset scope. Runs synchronously under the closed PAM window;
//! no worker or controlled-UID child may outlive the maintenance guard.
use std::{
    fs::{self, File},
    io::Read as _,
    os::unix::fs::MetadataExt as _,
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use natsume_workstation_ipc::ResourceControlError;
use rustix::fs::{AtFlags, Dir, FileType, Mode, OFlags, ResolveFlags, openat2};
use snafu::{ResultExt as _, Snafu, ensure};

const COMPLETION: &str = "cleanup-complete";
const SCOPES: [&str; 4] = ["tmp", "var/tmp", "run/lock", "var/crash"];

#[derive(Debug, Snafu)]
enum Error {
    #[snafu(display("native reset filesystem operation failed"))]
    Io { source: std::io::Error },
    #[snafu(display("native reset descriptor operation failed"))]
    Descriptor { source: rustix::io::Errno },
    #[snafu(display("native reset encountered unsafe state"))]
    Unsafe,
    #[snafu(display("native reset cleanup deadline expired"))]
    Deadline,
    #[snafu(display("contestant keyring cleanup was denied or failed"))]
    Keyring,
    #[snafu(display("contestant key cleanup failed"))]
    KeyCleanup { source: super::keys::Error },
}

type Result<T> = std::result::Result<T, Error>;

fn boundary<T>(result: Result<T>) -> std::result::Result<T, ResourceControlError> {
    result.map_err(|error| {
        tracing::warn!(%error, "native reset refused completion");
        match error {
            Error::Unsafe | Error::Keyring | Error::KeyCleanup { .. } => {
                super::rejected("native reset scope is unsafe or key cleanup was denied")
            }
            _ => super::unavailable("native reset cleanup is incomplete"),
        }
    })
}

fn trusted(metadata: &fs::Metadata, owner: u32) -> bool {
    metadata.uid() == owner && metadata.mode() & 0o022 == 0
}

fn policy(root: &Path, owner: u32) -> Result<()> {
    let mut directory = File::open(root).context(IoSnafu)?;
    ensure!(
        trusted(&directory.metadata().context(IoSnafu)?, owner),
        UnsafeSnafu
    );
    for component in ["etc", "natsume"] {
        directory = File::from(
            openat2(
                &directory,
                component,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
                Mode::empty(),
                ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
            )
            .context(DescriptorSnafu)?,
        );
        ensure!(
            trusted(&directory.metadata().context(IoSnafu)?, owner),
            UnsafeSnafu
        );
    }
    let mut file = File::from(
        openat2(
            &directory,
            "contest-reset-policy",
            OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
        )
        .context(DescriptorSnafu)?,
    );
    let metadata = file.metadata().context(IoSnafu)?;
    ensure!(
        metadata.is_file() && trusted(&metadata, owner) && metadata.len() == 2,
        UnsafeSnafu
    );
    let mut encoded = Vec::new();
    file.by_ref()
        .take(3)
        .read_to_end(&mut encoded)
        .context(IoSnafu)?;
    ensure!(encoded == b"1\n", UnsafeSnafu);
    Ok(())
}

pub(crate) fn require_policy(root: &Path) -> std::result::Result<(), ResourceControlError> {
    // The fixed service and PAM gate run as root; use the same trusted-owner
    // boundary as Home template validation for explicitly supplied fixture roots.
    boundary(policy(root, rustix::process::geteuid().as_raw()))
}

fn parse_completion(encoded: &str) -> Result<(u8, u64)> {
    let mut fields = encoded.split('\n');
    let version = match fields.next() {
        Some("1") => 1,
        Some("2") => 2,
        _ => return UnsafeSnafu.fail(),
    };
    let epoch = fields.next().and_then(|value| value.parse::<u64>().ok());
    let Some(epoch) = epoch.filter(|epoch| *epoch > 0 && i64::try_from(*epoch).is_ok()) else {
        return UnsafeSnafu.fail();
    };
    ensure!(encoded == format!("{version}\n{epoch}\n"), UnsafeSnafu);
    Ok((version, epoch))
}

pub(super) fn complete(root: &Path, epoch: u64) -> std::result::Result<bool, ResourceControlError> {
    let path = super::state_directory(root).join(COMPLETION);
    let file = match rustix::fs::open(
        &path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(fd) => File::from(fd),
        Err(rustix::io::Errno::NOENT) => return Ok(false),
        Err(source) => return boundary(Err(Error::Descriptor { source })),
    };
    boundary((|| {
        let metadata = file.metadata().context(IoSnafu)?;
        ensure!(
            metadata.is_file()
                && metadata.len() <= 32
                && trusted(&metadata, rustix::process::geteuid().as_raw()),
            UnsafeSnafu
        );
        let mut encoded = String::new();
        file.take(33)
            .read_to_string(&mut encoded)
            .context(IoSnafu)?;
        Ok(parse_completion(&encoded)? == (2, epoch))
    })())
}

fn before(deadline: Instant) -> Result<()> {
    ensure!(Instant::now() < deadline, DeadlineSnafu);
    Ok(())
}

// NO_XDEV rejects even same-device bind mounts. Every lookup is relative to an
// already-open directory, never a contestant-provided path or symlink target.
fn walk(
    directory: &File,
    uid: u32,
    removing: bool,
    safe_to_remove: bool,
    deadline: Instant,
) -> Result<()> {
    let entries = Dir::read_from(directory).context(DescriptorSnafu)?;
    for entry in entries {
        before(deadline)?;
        let entry = entry.context(DescriptorSnafu)?;
        let name = entry.file_name();
        if name == c"." || name == c".." {
            continue;
        }
        let object = File::from(
            openat2(
                directory,
                name,
                OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
                ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
            )
            .context(DescriptorSnafu)?,
        );
        let metadata = object.metadata().context(IoSnafu)?;
        let owned = metadata.uid() == uid;
        ensure!(!removing || owned, UnsafeSnafu);
        ensure!(!owned || safe_to_remove, UnsafeSnafu);
        if metadata.is_dir() {
            // Discover foreign trees, but never erase through an ancestor a
            // live foreign UID can rename or populate. Such contestant data is
            // an explicit maintenance failure, not silently out of scope.
            let safe_child = safe_to_remove
                && (metadata.uid() == 0 || owned)
                && (metadata.mode() & 0o022 == 0
                    || (metadata.uid() == 0 && metadata.mode() & 0o1000 != 0));
            let child = File::from(
                openat2(
                    directory,
                    name,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                    ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
                )
                .context(DescriptorSnafu)?,
            );
            let actual = child.metadata().context(IoSnafu)?;
            ensure!(
                (actual.dev(), actual.ino()) == (metadata.dev(), metadata.ino()),
                UnsafeSnafu
            );
            walk(&child, uid, removing || owned, safe_child, deadline)?;
        }
        if owned {
            // Reject a replacement between enumeration and removal. The closed
            // UID/PAM boundary prevents contestant races; shared sticky roots
            // and nonwritable directories exclude other unprivileged renames.
            let actual = rustix::fs::statat(directory, name, AtFlags::SYMLINK_NOFOLLOW)
                .context(DescriptorSnafu)?;
            ensure!(
                actual.st_ino == metadata.ino()
                    && actual.st_dev == metadata.dev()
                    && actual.st_uid == uid,
                UnsafeSnafu
            );
            let flags = if metadata.is_dir() {
                AtFlags::REMOVEDIR
            } else {
                AtFlags::empty()
            };
            rustix::fs::unlinkat(directory, name, flags).context(DescriptorSnafu)?;
        } else {
            ensure!(!removing, UnsafeSnafu);
        }
    }
    // Persist deletions before the durable epoch proof can suppress a retry
    // after a power loss (notably disk-backed /var/tmp and /var/crash).
    directory.sync_all().context(IoSnafu)?;
    Ok(())
}

fn scope(root: &Path, relative: &str) -> Result<Option<File>> {
    let mut directory = File::open(root).context(IoSnafu)?;
    let components: Vec<_> = relative.split('/').collect();
    for (index, component) in components.iter().enumerate() {
        let fd = match openat2(
            &directory,
            *component,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
        ) {
            Ok(fd) => fd,
            Err(rustix::io::Errno::NOENT)
                if relative == "var/crash" && index + 1 == components.len() =>
            {
                return Ok(None);
            }
            Err(source) => return Err(Error::Descriptor { source }),
        };
        directory = File::from(fd);
        let metadata = directory.metadata().context(IoSnafu)?;
        ensure!(
            metadata.uid() == 0
                && (metadata.mode() & 0o022 == 0
                    || (index + 1 == components.len() && metadata.mode() & 0o1000 != 0)),
            UnsafeSnafu
        );
    }
    Ok(Some(directory))
}

fn allowed_mount(root: &Path, point: &Path) -> bool {
    ["home/teams", "tmp", "var/tmp", "run/lock", "var/crash"]
        .iter()
        .any(|scope| point.starts_with(root.join(scope)) && point != root.join(scope))
}

fn fuse_mounts(
    root: &Path,
    uid: u32,
) -> std::result::Result<Vec<procfs::process::MountInfo>, ResourceControlError> {
    use procfs::FromRead as _;
    let mounts = procfs::process::MountInfos::from_file(root.join("proc/self/mountinfo"))
        .map_err(|_| super::unavailable("native reset mount state is unavailable"))?;
    let mut selected = Vec::new();
    for mount in &mounts {
        let owner = mount
            .super_options
            .get("user_id")
            .and_then(Option::as_deref);
        if (mount.fs_type == "fuse" || mount.fs_type.starts_with("fuse."))
            && owner == Some(uid.to_string().as_str())
        {
            if !allowed_mount(root, &mount.mount_point)
                || mounts.iter().any(|other| {
                    other.mnt_id != mount.mnt_id && other.mount_point == mount.mount_point
                })
                || mounts.iter().any(|other| {
                    other.mount_point != mount.mount_point
                        && other.mount_point.starts_with(&mount.mount_point)
                        && !((other.fs_type == "fuse" || other.fs_type.starts_with("fuse."))
                            && other
                                .super_options
                                .get("user_id")
                                .and_then(Option::as_deref)
                                == Some(uid.to_string().as_str()))
                })
            {
                return Err(super::rejected("contestant FUSE mount scope is unsafe"));
            }
            selected.push(mount.clone());
        }
    }
    selected.sort_by_key(|mount| std::cmp::Reverse(mount.mount_point.components().count()));
    Ok(selected)
}

fn unmount_fuse(root: &Path, uid: u32) -> std::result::Result<(), ResourceControlError> {
    use procfs::FromRead as _;
    for mount in fuse_mounts(root, uid)? {
        // Prove each parent is non-symlinked and cannot be changed by another
        // unprivileged UID before using the kernel's ordinary pathname unmount.
        let relative = mount
            .mount_point
            .strip_prefix(root)
            .map_err(|_| super::rejected("FUSE path is outside reset scope"))?;
        let parent = relative
            .parent()
            .ok_or_else(|| super::rejected("FUSE mount parent is invalid"))?;
        let mut directory =
            File::open(root).map_err(|_| super::unavailable("FUSE mount parent is unavailable"))?;
        for component in parent.components() {
            directory = File::from(
                openat2(
                    &directory,
                    component.as_os_str(),
                    OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
                    Mode::empty(),
                    ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
                )
                .map_err(|_| super::rejected("FUSE mount parent is unsafe"))?,
            );
            let metadata = directory
                .metadata()
                .map_err(|_| super::unavailable("FUSE mount parent is unavailable"))?;
            if !matches!(metadata.uid(), 0) && metadata.uid() != uid
                || metadata.mode() & 0o022 != 0
                    && !(metadata.uid() == 0 && metadata.mode() & 0o1000 != 0)
            {
                return Err(super::rejected("FUSE mount parent is unsafe"));
            }
        }
        let name = relative
            .file_name()
            .ok_or_else(|| super::rejected("FUSE mount name is invalid"))?;
        let target = openat2(
            &directory,
            name,
            OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
        )
        .map_err(|_| super::rejected("FUSE mount target is unsafe"))?;
        let stat = rustix::fs::fstat(target)
            .map_err(|_| super::unavailable("FUSE target is unavailable"))?;
        if FileType::from_raw_mode(stat.st_mode) == FileType::Symlink {
            return Err(super::rejected("FUSE mount target is unsafe"));
        }
        rustix::mount::unmount(&mount.mount_point, rustix::mount::UnmountFlags::empty()).map_err(
            |_| super::unavailable("contestant FUSE mount cannot be ordinarily unmounted"),
        )?;
    }
    if !fuse_mounts(root, uid)?.is_empty() {
        return Err(super::unavailable("contestant FUSE mounts remain"));
    }
    let mounts = procfs::process::MountInfos::from_file(root.join("proc/self/mountinfo"))
        .map_err(|_| super::unavailable("native reset mount state is unavailable"))?;
    let home = root.join("home/teams");
    let generations = super::state_directory(root).join("generations");
    if mounts.iter().any(|mount| {
        (mount.mount_point.starts_with(&home) && mount.mount_point != home)
            || mount.mount_point.starts_with(&generations)
    }) {
        return Err(super::rejected(
            "Home reset contains an unexpected nested mount",
        ));
    }
    Ok(())
}

fn keys(uid: u32, gid: u32, deadline: Instant) -> Result<()> {
    before(deadline)?;
    let child = Command::new("/usr/bin/setpriv")
        .args([
            "--no-new-privs",
            "--inh-caps=-all",
            "--ambient-caps=-all",
            "--clear-groups",
            "--reuid",
            &uid.to_string(),
            "--regid",
            &gid.to_string(),
            "/usr/lib/natsume/natsume-privileged-helper",
            "cleanup-keyrings",
        ])
        .env_clear()
        .current_dir("/")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context(IoSnafu)?;
    wait_keyring_worker(child, deadline)
}

/// Fixed worker entry: the long-running root service never changes credentials
/// or session keyrings. No subprocesses are created by this controlled-UID worker.
pub(crate) fn keyring_worker(root: &Path) -> std::result::Result<(), ResourceControlError> {
    let account = crate::session::account(root, "teams")?;
    let status = procfs::process::Process::myself()
        .and_then(|process| process.status())
        .map_err(|_| super::unavailable("keyring worker credentials cannot be inspected"))?;
    if status.ruid != account.uid
        || status.euid != account.uid
        || status.suid != account.uid
        || status.rgid != account.gid
        || status.egid != account.gid
        || status.sgid != account.gid
        || !status.groups.is_empty()
        || status.capinh != 0
        || status.capprm != 0
        || status.capeff != 0
        || status.capamb != Some(0)
        || status.nonewprivs != Some(1)
    {
        return Err(super::rejected(
            "keyring worker credentials are not isolated",
        ));
    }
    boundary(super::keys::reset(account.uid).context(KeyCleanupSnafu))
}

fn wait_keyring_worker(mut child: Child, deadline: Instant) -> Result<()> {
    // setpriv execs the worker directly: exactly one owned child, no shell.
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            Ok(None) => break DeadlineSnafu.fail(),
            Err(source) => break Err(Error::Io { source }),
        }
    };
    let status = match status {
        Ok(status) => status,
        Err(error) => {
            let _ = child.kill();
            child.wait().context(IoSnafu)?;
            return Err(error);
        }
    };
    ensure!(status.success(), KeyringSnafu);
    Ok(())
}

pub(super) fn run(root: &Path, epoch: u64) -> std::result::Result<(), ResourceControlError> {
    require_policy(root)?;
    if complete(root, epoch)? {
        return Ok(());
    }
    let account = crate::session::account(root, "teams")?;
    let deadline = Instant::now() + Duration::from_secs(1);
    unmount_fuse(root, account.uid)?;
    boundary((|| {
        for relative in SCOPES {
            before(deadline)?;
            if let Some(directory) = scope(root, relative)? {
                walk(&directory, account.uid, false, true, deadline)?;
            }
        }
        keys(account.uid, account.gid, deadline)
    })())?;
    // This proof precedes Home Verified and gate publication. A crash after it
    // may replay mount verification, but must never erase new-generation state.
    super::write_state(root, COMPLETION, &format!("2\n{epoch}\n"))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt as _, symlink};

    #[derive(Debug, Snafu)]
    pub(crate) enum TestError {
        #[snafu(context(false))]
        Io { source: std::io::Error },
        #[snafu(context(false))]
        Control { source: ResourceControlError },
        #[snafu(context(false))]
        Bus { source: zbus::Error },
        #[snafu(context(false))]
        Descriptor { source: rustix::io::Errno },
    }

    pub(crate) fn policy_fixture(root: &Path) -> std::result::Result<(), TestError> {
        fs::create_dir_all(root.join("etc/natsume"))?;
        fs::write(root.join("etc/natsume/contest-reset-policy"), b"1\n")?;
        Ok(())
    }

    #[test]
    fn policy_requires_exact_bounded_regular_trusted_file_and_parents()
    -> std::result::Result<(), TestError> {
        let root = tempfile::tempdir()?;
        policy_fixture(root.path())?;
        let owner = rustix::process::geteuid().as_raw();
        assert!(policy(root.path(), owner).is_ok());
        let marker = root.path().join("etc/natsume/contest-reset-policy");
        for content in [
            "",
            "1",
            "1\n\n",
            "2\n",
            "1\r\n",
            "111111111111111111111111111111111111",
        ] {
            fs::write(&marker, content)?;
            assert!(policy(root.path(), owner).is_err());
        }
        fs::write(&marker, b"1\n")?;
        for path in [
            &marker,
            &root.path().join("etc/natsume"),
            &root.path().join("etc"),
        ] {
            let mode = fs::metadata(path)?.mode() & 0o7777;
            fs::set_permissions(path, fs::Permissions::from_mode(mode | 0o022))?;
            assert!(policy(root.path(), owner).is_err());
            fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
        }
        assert!(policy(root.path(), owner.wrapping_add(1)).is_err());
        let target = root.path().join("target");
        fs::rename(&marker, &target)?;
        symlink(&target, &marker)?;
        assert!(policy(root.path(), owner).is_err());
        fs::remove_file(&marker)?;
        fs::rename(&target, &marker)?;
        let directory = root.path().join("etc/natsume");
        fs::rename(&directory, &target)?;
        symlink(&target, &directory)?;
        assert!(policy(root.path(), owner).is_err());
        Ok(())
    }

    #[test]
    fn completion_records_are_versioned_exact_and_epoch_bound() -> std::result::Result<(), TestError>
    {
        let root = tempfile::tempdir()?;
        fs::create_dir_all(super::super::state_directory(root.path()))?;
        assert!(!complete(root.path(), 7)?);
        for invalid in [
            "",
            "7\nverified\n",
            "3\n7\n",
            "1\n0\n",
            "1\n07\n",
            "1\n7",
            "1\n7\nextra\n",
            "1\n9223372036854775808\n",
        ] {
            super::super::write_state(root.path(), COMPLETION, invalid)?;
            assert!(complete(root.path(), 7).is_err());
        }
        super::super::write_state(root.path(), COMPLETION, "1\n7\n")?;
        assert!(!complete(root.path(), 7)?);
        assert!(!complete(root.path(), 8)?);
        super::super::write_state(root.path(), COMPLETION, "2\n7\n")?;
        assert!(complete(root.path(), 7)?);
        assert!(!complete(root.path(), 8)?);
        let path = super::super::state_directory(root.path()).join(COMPLETION);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o666))?;
        assert!(complete(root.path(), 7).is_err());
        fs::remove_file(&path)?;
        symlink(root.path().join("outside"), path)?;
        assert!(complete(root.path(), 7).is_err());
        Ok(())
    }

    #[test]
    fn temporary_cleanup_is_nofollow_idempotent_and_deadline_bound()
    -> std::result::Result<(), TestError> {
        let root = tempfile::tempdir()?;
        let outside = tempfile::tempdir()?;
        fs::write(outside.path().join("keep"), "foreign target")?;
        let uid = rustix::process::geteuid().as_raw();
        let directory = File::open(root.path())?;
        fs::create_dir(root.path().join("owned"))?;
        fs::write(root.path().join("owned/file"), "old")?;
        symlink(outside.path(), root.path().join("link"))?;
        rustix::fs::mknodat(
            &directory,
            "fifo",
            FileType::Fifo,
            Mode::RUSR | Mode::WUSR,
            0,
        )?;
        let socket = std::os::unix::net::UnixListener::bind(root.path().join("socket"))?;
        assert!(walk(&directory, uid, false, true, Instant::now()).is_err());
        assert!(root.path().join("owned/file").exists());
        assert!(
            walk(
                &directory,
                uid,
                false,
                true,
                Instant::now() + Duration::from_secs(1)
            )
            .is_ok()
        );
        assert!(
            walk(
                &directory,
                uid,
                false,
                true,
                Instant::now() + Duration::from_secs(1)
            )
            .is_ok()
        );
        assert_eq!(
            fs::read_to_string(outside.path().join("keep"))?,
            "foreign target"
        );
        assert_eq!(fs::read_dir(root.path())?.count(), 0);
        drop(socket);
        Ok(())
    }

    #[test]
    fn temporary_cleanup_rejects_writable_directories_and_preserves_foreign_objects()
    -> std::result::Result<(), TestError> {
        let root = tempfile::tempdir()?;
        let directory = File::open(root.path())?;
        let uid = rustix::process::geteuid().as_raw();
        fs::create_dir(root.path().join("unsafe"))?;
        fs::set_permissions(
            root.path().join("unsafe"),
            fs::Permissions::from_mode(0o777),
        )?;
        fs::write(root.path().join("unsafe/keep"), "keep")?;
        assert!(
            walk(
                &directory,
                uid,
                false,
                true,
                Instant::now() + Duration::from_secs(1)
            )
            .is_err()
        );
        assert!(root.path().join("unsafe/keep").exists());
        fs::set_permissions(
            root.path().join("unsafe"),
            fs::Permissions::from_mode(0o700),
        )?;
        // Asking for a different UID must not erase this user's private tree.
        assert!(
            walk(
                &directory,
                uid.wrapping_add(1),
                false,
                true,
                Instant::now() + Duration::from_secs(1)
            )
            .is_ok()
        );
        assert!(root.path().join("unsafe/keep").exists());
        if rustix::process::geteuid().is_root() {
            let contestant = 2000;
            rustix::fs::chown(
                root.path().join("unsafe"),
                Some(rustix::fs::Uid::from_raw(contestant)),
                None,
            )?;
            assert!(
                walk(
                    &directory,
                    contestant,
                    false,
                    true,
                    Instant::now() + Duration::from_secs(1)
                )
                .is_err()
            );
            assert!(root.path().join("unsafe/keep").exists());
        }
        Ok(())
    }

    #[test]
    fn contestant_entries_under_foreign_or_mutable_ancestors_fail_without_deletion()
    -> std::result::Result<(), TestError> {
        if !rustix::process::geteuid().is_root() {
            tracing::info!("foreign-UID fixture requires root; VM acceptance exercises this case");
            return Ok(());
        }
        let root = tempfile::tempdir()?;
        let directory = File::open(root.path())?;
        let teams = rustix::fs::Uid::from_raw(2000);
        let foreign = rustix::fs::Uid::from_raw(2001);
        fs::create_dir(root.path().join("shared"))?;
        fs::set_permissions(
            root.path().join("shared"),
            fs::Permissions::from_mode(0o777),
        )?;
        rustix::fs::chown(root.path().join("shared"), Some(foreign), None)?;
        fs::write(root.path().join("shared/teams"), "must not silently skip")?;
        rustix::fs::chown(root.path().join("shared/teams"), Some(teams), None)?;
        fs::write(root.path().join("shared/foreign"), "keep")?;
        rustix::fs::chown(root.path().join("shared/foreign"), Some(foreign), None)?;
        assert!(
            walk(
                &directory,
                2000,
                false,
                true,
                Instant::now() + Duration::from_secs(1)
            )
            .is_err()
        );
        assert_eq!(
            fs::read_to_string(root.path().join("shared/teams"))?,
            "must not silently skip"
        );
        assert_eq!(
            fs::read_to_string(root.path().join("shared/foreign"))?,
            "keep"
        );
        fs::remove_file(root.path().join("shared/teams"))?;
        fs::set_permissions(
            root.path().join("shared"),
            fs::Permissions::from_mode(0o700),
        )?;
        assert!(
            walk(
                &directory,
                2000,
                false,
                true,
                Instant::now() + Duration::from_secs(1)
            )
            .is_ok()
        );
        assert_eq!(
            fs::read_to_string(root.path().join("shared/foreign"))?,
            "keep"
        );
        fs::create_dir(root.path().join("mutable-root"))?;
        fs::set_permissions(
            root.path().join("mutable-root"),
            fs::Permissions::from_mode(0o777),
        )?;
        fs::write(
            root.path().join("mutable-root/teams"),
            "keep on unsafe ancestry",
        )?;
        rustix::fs::chown(root.path().join("mutable-root/teams"), Some(teams), None)?;
        assert!(
            walk(
                &directory,
                2000,
                false,
                true,
                Instant::now() + Duration::from_secs(1)
            )
            .is_err()
        );
        assert!(root.path().join("mutable-root/teams").exists());
        Ok(())
    }

    #[test]
    fn descriptor_walk_refuses_mount_crossing() -> std::result::Result<(), TestError> {
        let root = File::open("/")?;
        let fd = openat2(
            &root,
            "proc",
            OFlags::PATH | OFlags::DIRECTORY,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
        );
        assert!(matches!(fd, Err(rustix::io::Errno::XDEV)));
        // Exercise real same-device bind boundaries too when the host exposes
        // one; this does not mount/unmount anything in the developer namespace.
        if let Ok(mounts) = procfs::process::Process::myself().and_then(|p| p.mountinfo()) {
            for mount in &mounts {
                if let (Some(parent), Some(name)) =
                    (mount.mount_point.parent(), mount.mount_point.file_name())
                    && let (Ok(parent), Ok(metadata)) =
                        (File::open(parent), fs::symlink_metadata(&mount.mount_point))
                    && parent.metadata()?.dev() == metadata.dev()
                {
                    let fd = openat2(
                        &parent,
                        name,
                        OFlags::PATH | OFlags::NOFOLLOW,
                        Mode::empty(),
                        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
                    );
                    assert!(matches!(fd, Err(rustix::io::Errno::XDEV)));
                }
            }
        }
        Ok(())
    }

    #[test]
    fn denied_and_timed_out_key_commands_fail_and_are_reaped_before_retry()
    -> std::result::Result<(), TestError> {
        let denied = Command::new("/bin/false").spawn()?;
        assert!(matches!(
            wait_keyring_worker(denied, Instant::now() + Duration::from_secs(1)),
            Err(Error::Keyring)
        ));
        for _ in 0..2 {
            let child = Command::new("/bin/sleep").arg("30").spawn()?;
            let pid = child.id();
            assert!(matches!(
                wait_keyring_worker(child, Instant::now() + Duration::from_millis(10)),
                Err(Error::Deadline)
            ));
            assert!(!Path::new("/proc").join(pid.to_string()).exists());
        }
        Ok(())
    }

    #[test]
    fn fuse_selection_requires_fixed_scope_uid_and_no_stacked_or_foreign_submount()
    -> std::result::Result<(), TestError> {
        let root = tempfile::tempdir()?;
        fs::create_dir_all(root.path().join("proc/self"))?;
        let mountinfo = root.path().join("proc/self/mountinfo");
        let line = |id, relative: &str, owner| {
            format!(
                "{id} 1 0:50 / {} rw - fuse.test test rw,user_id={owner},group_id=3000\n",
                root.path().join(relative).display()
            )
        };
        fs::write(&mountinfo, line(10, "tmp/old", 2000))?;
        assert_eq!(fuse_mounts(root.path(), 2000)?.len(), 1);
        assert!(fuse_mounts(root.path(), 2001)?.is_empty());
        fs::write(&mountinfo, line(10, "home/waiting/old", 2000))?;
        assert!(fuse_mounts(root.path(), 2000).is_err());
        fs::write(
            &mountinfo,
            format!("{}{}", line(10, "tmp/old", 2000), line(11, "tmp/old", 2001)),
        )?;
        assert!(fuse_mounts(root.path(), 2000).is_err());
        fs::write(
            &mountinfo,
            format!(
                "{}{}",
                line(10, "tmp/old", 2000),
                line(11, "tmp/old/child", 2001)
            ),
        )?;
        assert!(fuse_mounts(root.path(), 2000).is_err());
        fs::write(
            &mountinfo,
            format!(
                "{}{}",
                line(10, "tmp/old", 2000),
                line(11, "tmp/old/child", 2000)
            ),
        )?;
        let selected = fuse_mounts(root.path(), 2000)?;
        assert_eq!(selected.first().map(|mount| mount.mnt_id), Some(11));
        Ok(())
    }
}
