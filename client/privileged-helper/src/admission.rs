use std::{
    env,
    fs::{self, File},
    io::Write as _,
    os::unix::fs::MetadataExt as _,
    path::{Path, PathBuf},
};

use natsume_local_control_api::{ResourceControlError, SessionRole};
use procfs::process::Process;
use rustix::fs::{Mode, OFlags};

use crate::{
    home,
    processes::{Identity, start_time},
    session::{account_name, read_boot_id, rejected, unavailable, valid_boot_id},
};

const RUNTIME: &str = "run/natsume-privileged";
const PERMIT: &str = "contest-permit";
const WORKERS: &str = "contest-workers";
const WORKER_EXE: &str = "/usr/libexec/gdm-session-worker";

/// Shared by the service and fixed root login entry; never held by `pam_exec`.
pub(crate) struct MutationGuard {
    _lock: File,
}

/// Kept across Home mutation after permission withdrawal and worker drainage.
pub(crate) struct MaintenanceGuard {
    _lock: File,
}

#[derive(Debug, PartialEq, Eq)]
struct Worker {
    boot: String,
    pid: i32,
    start: u64,
}

fn runtime(root: &Path) -> PathBuf {
    root.join(RUNTIME)
}

fn checked_directory(path: &Path) -> Result<(), ResourceControlError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| unavailable("login admission directory is unavailable"))?;
    if !metadata.is_dir()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o077 != 0
    {
        return Err(rejected("login admission directory is unsafe"));
    }
    Ok(())
}

fn lock_file(root: &Path, name: &str) -> Result<File, ResourceControlError> {
    checked_directory(&runtime(root))?;
    let file = File::from(
        rustix::fs::open(
            runtime(root).join(name),
            OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )
        .map_err(|_| unavailable("login admission lock is unavailable"))?,
    );
    let metadata = file
        .metadata()
        .map_err(|_| unavailable("login admission lock is unavailable"))?;
    if !metadata.is_file()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o077 != 0
    {
        return Err(rejected("login admission lock is unsafe"));
    }
    Ok(file)
}

pub(crate) fn mutation(root: &Path) -> Result<MutationGuard, ResourceControlError> {
    let lock = lock_file(root, "session-mutation.lock")?;
    lock.try_lock()
        .map_err(|_| unavailable("another graphical mutation is in progress"))?;
    Ok(MutationGuard { _lock: lock })
}

pub(crate) fn close(root: &Path) -> Result<(), ResourceControlError> {
    checked_directory(&runtime(root))?;
    match fs::remove_file(runtime(root).join(PERMIT)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(unavailable("contest login permission cannot be withdrawn")),
    }
}

pub(crate) fn open(root: &Path) -> Result<(), ResourceControlError> {
    let lock = lock_file(root, "contest-admission.lock")?;
    lock.try_lock()
        .map_err(|_| unavailable("contest admission is busy"))?;
    home::window::require_closed(root)?;
    // This exclusive lock has no surviving publisher. Discard only its private
    // unfinished temporary file, so a crash before rename can be recovered.
    let pending = runtime(root).join(PERMIT).with_extension("pending");
    match fs::symlink_metadata(&pending) {
        Ok(metadata)
            if metadata.is_file() && metadata.uid() == rustix::process::geteuid().as_raw() =>
        {
            fs::remove_file(pending)
                .map_err(|_| unavailable("unfinished permission cannot be removed"))?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Ok(_) => return Err(rejected("unfinished permission is unsafe")),
        Err(_) => return Err(unavailable("unfinished permission cannot be inspected")),
    }
    write_atomic(&runtime(root).join(PERMIT), &read_boot_id(root)?)
}

pub(crate) fn require_open(root: &Path) -> Result<(), ResourceControlError> {
    let permit = fs::read_to_string(runtime(root).join(PERMIT))
        .map_err(|_| rejected("contest login permission is closed"))?;
    home::window::require_closed(root)?;
    if permit != read_boot_id(root)? {
        return Err(rejected("contest login permission is stale"));
    }
    Ok(())
}

fn write_atomic(path: &Path, value: &str) -> Result<(), ResourceControlError> {
    let temporary = path.with_extension("pending");
    let fd = rustix::fs::open(
        &temporary,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::RUSR | Mode::WUSR,
    )
    .map_err(|_| unavailable("login admission record cannot be prepared"))?;
    let mut file = File::from(fd);
    file.write_all(value.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|_| unavailable("login admission record cannot be written"))?;
    fs::rename(temporary, path)
        .map_err(|_| unavailable("login admission record cannot be published"))
}

impl Worker {
    fn parse(value: &str) -> Option<Self> {
        let mut lines = value.lines();
        let boot = lines.next()?.to_owned();
        let pid = lines.next()?.parse::<i32>().ok()?;
        let start = lines.next()?.parse::<u64>().ok()?;
        (valid_boot_id(&boot) && pid > 1 && start > 0 && lines.next().is_none()).then_some(Self {
            boot,
            pid,
            start,
        })
    }

    fn encode(&self) -> String {
        format!("{}\n{}\n{}\n", self.boot, self.pid, self.start)
    }
}

fn live_workers(root: &Path) -> Result<Vec<Identity>, ResourceControlError> {
    let mut live = Vec::new();
    let directory = runtime(root).join(WORKERS);
    checked_directory(&directory)?;
    let boot = read_boot_id(root)?;
    for entry in fs::read_dir(&directory)
        .map_err(|_| unavailable("PAM worker registrations are unavailable"))?
    {
        let entry = entry.map_err(|_| unavailable("PAM worker registration is unavailable"))?;
        let metadata = entry
            .file_type()
            .map_err(|_| unavailable("PAM worker registration is unavailable"))?;
        if !metadata.is_file() {
            return Err(rejected("PAM worker registration is invalid"));
        }
        let worker = fs::read_to_string(entry.path())
            .ok()
            .and_then(|value| Worker::parse(&value))
            .filter(|worker| entry.file_name() == worker.pid.to_string().as_str())
            .ok_or_else(|| rejected("PAM worker registration is invalid"))?;
        if worker.boot == boot && start_time(root, worker.pid)? == Some(worker.start) {
            live.push(Identity {
                pid: worker.pid,
                start: worker.start,
            });
            continue;
        }
        fs::remove_file(entry.path())
            .map_err(|_| unavailable("stale PAM worker registration cannot be removed"))?;
    }
    Ok(live)
}

fn no_live_workers(root: &Path) -> Result<(), ResourceControlError> {
    if live_workers(root)?.is_empty() {
        Ok(())
    } else {
        Err(unavailable("a registered PAM worker is still running"))
    }
}

pub(crate) fn stop_workers(
    root: &Path,
    signal: rustix::process::Signal,
) -> Result<(), ResourceControlError> {
    let lock = lock_file(root, "contest-admission.lock")?;
    lock.try_lock()
        .map_err(|_| unavailable("a PAM admission check is in progress"))?;
    if runtime(root)
        .join(PERMIT)
        .try_exists()
        .map_err(|_| unavailable("contest permission cannot be inspected"))?
    {
        return Err(rejected("contest admission must be closed before drainage"));
    }
    let workers = live_workers(root)?;
    drop(lock);
    for identity in workers {
        crate::processes::signal(identity, signal)?;
    }
    Ok(())
}

pub(crate) fn maintenance(root: &Path) -> Result<MaintenanceGuard, ResourceControlError> {
    let lock = lock_file(root, "contest-admission.lock")?;
    lock.try_lock()
        .map_err(|_| unavailable("a PAM admission check is in progress"))?;
    if runtime(root)
        .join(PERMIT)
        .try_exists()
        .map_err(|_| unavailable("contest permission cannot be inspected"))?
    {
        return Err(rejected(
            "contest admission must be closed before maintenance",
        ));
    }
    no_live_workers(root)?;
    Ok(MaintenanceGuard { _lock: lock })
}

/// A timed-out API client may leave a worker before logind knows the session.
/// Resampling only logind must not authorize a second contest login transaction.
pub(crate) fn require_no_workers(root: &Path) -> Result<(), ResourceControlError> {
    let lock = lock_file(root, "contest-admission.lock")?;
    lock.try_lock()
        .map_err(|_| unavailable("a PAM admission check is in progress"))?;
    no_live_workers(root)
}

/// Called only by stock `pam_exec`; PAM supplies policy input, never a worker PID.
///
/// # Errors
/// Rejects invalid callers, closed admission and unavailable worker evidence.
pub fn pam_gate() -> Result<(), ResourceControlError> {
    let user = env::var("PAM_USER").map_err(|_| rejected("PAM user is unavailable"))?;
    let phase = env::var("PAM_TYPE").map_err(|_| rejected("PAM phase is unavailable"))?;
    if user != account_name(SessionRole::Contest) || phase == "close_session" {
        return Ok(());
    }
    if !matches!(phase.as_str(), "auth" | "account" | "open_session")
        || !rustix::process::getuid().is_root()
        || !rustix::process::geteuid().is_root()
    {
        return Err(rejected("PAM gate invocation is invalid"));
    }
    let parent = rustix::process::getppid().ok_or_else(|| rejected("PAM worker is absent"))?;
    let process = Process::new(parent.as_raw_nonzero().get())
        .map_err(|_| rejected("PAM worker is unavailable"))?;
    let status = process
        .status()
        .map_err(|_| rejected("PAM worker is unavailable"))?;
    if status.ruid != 0
        || status.euid != 0
        || process
            .exe()
            .map_err(|_| rejected("PAM worker is unavailable"))?
            != Path::new(WORKER_EXE)
    {
        return Err(rejected("PAM caller is not the fixed GDM worker"));
    }
    let root = Path::new("/");
    let lock = lock_file(root, "contest-admission.lock")?;
    lock.try_lock_shared()
        .map_err(|_| rejected("contest admission is closed for maintenance"))?;
    require_open(root)?;
    home::require_template(root)?;
    checked_directory(&runtime(root).join(WORKERS))?;
    let worker = Worker {
        boot: read_boot_id(root)?,
        pid: process.pid,
        start: process
            .stat()
            .map_err(|_| rejected("PAM worker is unavailable"))?
            .starttime,
    };
    if rustix::process::getppid() != Some(parent) {
        return Err(rejected("PAM worker changed during admission"));
    }
    write_atomic(
        &runtime(root).join(WORKERS).join(worker.pid.to_string()),
        &worker.encode(),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;

    const BOOT: &str = "550e8400-e29b-41d4-a716-446655440000";

    fn fixture() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        for path in [runtime(root.path()), runtime(root.path()).join(WORKERS)] {
            fs::create_dir_all(&path)?;
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        }
        fs::create_dir_all(root.path().join("proc/sys/kernel/random"))?;
        fs::write(root.path().join("proc/sys/kernel/random/boot_id"), BOOT)?;
        Ok(root)
    }

    #[test]
    fn permits_are_boot_bound_and_exclusive_maintenance_blocks_admission()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = fixture()?;
        open(root.path())?;
        require_open(root.path())?;
        assert!(maintenance(root.path()).is_err());
        fs::write(
            root.path().join("proc/sys/kernel/random/boot_id"),
            "550e8400-e29b-41d4-a716-446655440001",
        )?;
        assert!(require_open(root.path()).is_err());
        close(root.path())?;
        let guard = maintenance(root.path())?;
        assert!(open(root.path()).is_err());
        drop(guard);
        open(root.path())?;
        Ok(())
    }

    #[test]
    fn free_flock_does_not_prove_worker_exit_and_pid_reuse_is_distinguished()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = fixture()?;
        let process = Process::myself()?;
        let mut worker = Worker {
            boot: BOOT.to_owned(),
            pid: process.pid,
            start: process.stat()?.starttime,
        };
        let proc_dir = root.path().join("proc").join(worker.pid.to_string());
        fs::create_dir(&proc_dir)?;
        fs::copy("/proc/self/stat", proc_dir.join("stat"))?;
        let record = runtime(root.path())
            .join(WORKERS)
            .join(worker.pid.to_string());
        fs::write(&record, worker.encode())?;
        assert!(maintenance(root.path()).is_err());
        assert!(require_no_workers(root.path()).is_err());
        worker.start += 1;
        fs::write(&record, worker.encode())?;
        drop(maintenance(root.path())?);
        assert!(!record.exists());
        fs::write(&record, "corrupt")?;
        assert!(maintenance(root.path()).is_err());
        assert!(record.exists());
        Ok(())
    }

    #[test]
    fn interrupted_permission_publication_can_be_retried_after_home_verification()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = fixture()?;
        fs::write(
            runtime(root.path()).join("contest-permit.pending"),
            "partial",
        )?;
        assert!(require_open(root.path()).is_err());
        open(root.path())?;
        require_open(root.path())?;
        Ok(())
    }

    #[test]
    fn root_mutations_share_a_cross_process_lock() -> Result<(), Box<dyn std::error::Error>> {
        let root = fixture()?;
        let guard = mutation(root.path())?;
        assert!(mutation(root.path()).is_err());
        drop(guard);
        drop(mutation(root.path())?);
        Ok(())
    }
}
