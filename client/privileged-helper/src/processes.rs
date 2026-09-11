use std::path::Path;

use natsume_local_control_api::ResourceControlError;
use procfs::{ProcError, process::Process};
use rustix::process::{Pid, PidfdFlags, Signal, pidfd_open, pidfd_send_signal};

use crate::session::{rejected, unavailable};

/// PID alone is never authority to signal a process that may have been replaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Identity {
    pub(crate) pid: i32,
    pub(crate) start: u64,
}

fn optional<T>(result: procfs::ProcResult<T>) -> Result<Option<T>, ResourceControlError> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(ProcError::NotFound(_)) => Ok(None),
        Err(_) => Err(unavailable("managed process cannot be inspected")),
    }
}

pub(crate) fn start_time(root: &Path, pid: i32) -> Result<Option<u64>, ResourceControlError> {
    optional(Process::new_with_root(root.join("proc").join(pid.to_string())).and_then(|p| p.stat()))
        .map(|stat| stat.map(|stat| stat.starttime))
}

pub(crate) fn owned_by(root: &Path, uid: u32) -> Result<Vec<Identity>, ResourceControlError> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(root.join("proc"))
        .map_err(|_| unavailable("managed processes are unavailable"))?
    {
        let entry = entry.map_err(|_| unavailable("managed processes are unavailable"))?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<i32>().ok())
        else {
            continue;
        };
        let Some(process) = optional(Process::new_with_root(entry.path()))? else {
            continue;
        };
        let Some(stat) = optional(process.stat())? else {
            continue;
        };
        let Some(status) = optional(process.status())? else {
            continue;
        };
        if [status.ruid, status.euid, status.suid, status.fuid].contains(&uid) {
            found.push(Identity {
                pid,
                start: stat.starttime,
            });
        }
    }
    Ok(found)
}

pub(crate) fn signal(identity: Identity, signal: Signal) -> Result<(), ResourceControlError> {
    let pid = Pid::from_raw(identity.pid)
        .filter(|pid| pid.as_raw_nonzero().get() > 1)
        .ok_or_else(|| rejected("managed process identity is invalid"))?;
    let fd = match pidfd_open(pid, PidfdFlags::empty()) {
        Ok(fd) => fd,
        Err(rustix::io::Errno::SRCH) => return Ok(()),
        Err(_) => return Err(unavailable("managed process handle is unavailable")),
    };
    // Open first, then validate birth. A death/reuse after this read cannot retarget
    // the pidfd, including reuse between reading the directory and opening it.
    if start_time(Path::new("/"), identity.pid)? != Some(identity.start) {
        return Ok(());
    }
    match pidfd_send_signal(fd, signal) {
        Ok(()) | Err(rustix::io::Errno::SRCH) => Ok(()),
        Err(_) => Err(unavailable("managed process could not be stopped")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{error::Error, process::Command};

    #[test]
    fn a_stale_birth_cannot_signal_a_replacement_process() -> Result<(), Box<dyn Error>> {
        let mut child = Command::new("sleep").arg("30").spawn()?;
        let pid = i32::try_from(child.id())?;
        let start = start_time(Path::new("/"), pid)?.ok_or("child missing")?;
        signal(
            Identity {
                pid,
                start: start + 1,
            },
            Signal::KILL,
        )?;
        assert!(child.try_wait()?.is_none());
        signal(Identity { pid, start }, Signal::KILL)?;
        assert!(!child.wait()?.success());
        signal(Identity { pid, start }, Signal::KILL)?;
        Ok(())
    }
}
