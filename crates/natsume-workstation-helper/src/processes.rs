use std::{os::fd::OwnedFd, path::Path};

use natsume_workstation_ipc::ResourceControlError;
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
    signal_with(
        identity,
        signal,
        |pid| pidfd_open(pid, PidfdFlags::empty()),
        |pid| start_time(Path::new("/"), pid),
        pidfd_send_signal,
    )
}

fn signal_with(
    identity: Identity,
    signal: Signal,
    mut open: impl FnMut(Pid) -> rustix::io::Result<OwnedFd>,
    mut birth: impl FnMut(i32) -> Result<Option<u64>, ResourceControlError>,
    send: impl FnOnce(OwnedFd, Signal) -> rustix::io::Result<()>,
) -> Result<(), ResourceControlError> {
    let pid = Pid::from_raw(identity.pid)
        .filter(|pid| pid.as_raw_nonzero().get() > 1)
        .ok_or_else(|| rejected("managed process identity is invalid"))?;
    let mut retries = 0;
    let fd = loop {
        match open(pid) {
            Ok(fd) => break fd,
            Err(rustix::io::Errno::SRCH) => return Ok(()),
            Err(error) => {
                let current = birth(identity.pid).inspect_err(|_| {
                    tracing::warn!(
                        pid = identity.pid,
                        errno = error.raw_os_error(),
                        "pidfd open failed and process birth cannot be rechecked"
                    );
                })?;
                if current != Some(identity.start) {
                    return Ok(());
                }
                // Linux can lose PIDTYPE_TGID during exit before procfs loses
                // the captured birth. Resample briefly, never ignore EINVAL on
                // a still-current process or fall back to signalling by PID.
                if error == rustix::io::Errno::INVAL && retries < 2 {
                    retries += 1;
                    std::thread::sleep(std::time::Duration::from_millis(1));
                    continue;
                }
                tracing::warn!(
                    pid = identity.pid,
                    errno = error.raw_os_error(),
                    "managed process handle is unavailable for the captured birth"
                );
                return Err(unavailable("managed process handle is unavailable"));
            }
        }
    };
    // Open first, then validate birth. A death/reuse after this read cannot retarget
    // the pidfd, including reuse between reading the directory and opening it.
    if birth(identity.pid)? != Some(identity.start) {
        return Ok(());
    }
    match send(fd, signal) {
        Ok(()) | Err(rustix::io::Errno::SRCH) => Ok(()),
        Err(_) => Err(unavailable("managed process could not be stopped")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use snafu::Snafu;
    use std::{cell::Cell, process::Command};

    #[derive(Debug, Snafu)]
    enum TestError {
        #[snafu(context(false))]
        Io { source: std::io::Error },
        #[snafu(context(false))]
        Control { source: ResourceControlError },
        #[snafu(context(false))]
        Pid { source: std::num::TryFromIntError },
        #[snafu(display("test child is missing"))]
        MissingChild,
    }

    #[test]
    fn a_stale_birth_cannot_signal_a_replacement_process() -> Result<(), TestError> {
        let mut child = Command::new("sleep").arg("30").spawn()?;
        let pid = i32::try_from(child.id())?;
        let start = start_time(Path::new("/"), pid)?.ok_or(TestError::MissingChild)?;
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

    #[test]
    fn failed_open_is_harmless_only_when_the_captured_birth_is_gone_or_reused()
    -> Result<(), TestError> {
        let identity = Identity {
            pid: 123,
            start: 456,
        };
        for error in [
            rustix::io::Errno::INVAL,
            rustix::io::Errno::MFILE,
            rustix::io::Errno::PERM,
        ] {
            for current in [None, Some(identity.start + 1)] {
                signal_with(
                    identity,
                    Signal::KILL,
                    |pid| {
                        assert_eq!(pid.as_raw_nonzero().get(), identity.pid);
                        Err(error)
                    },
                    |pid| {
                        assert_eq!(pid, identity.pid);
                        Ok(current)
                    },
                    |_, _| panic!("failed open must never signal a PID replacement"),
                )?;
            }
        }
        Ok(())
    }

    #[test]
    fn a_live_or_uninspectable_birth_keeps_open_failures_closed() {
        let identity = Identity {
            pid: 123,
            start: 456,
        };
        for error in [
            rustix::io::Errno::INVAL,
            rustix::io::Errno::MFILE,
            rustix::io::Errno::PERM,
        ] {
            let attempts = Cell::new(0);
            let result = signal_with(
                identity,
                Signal::KILL,
                |_| {
                    attempts.set(attempts.get() + 1);
                    Err(error)
                },
                |_| Ok(Some(identity.start)),
                |_, _| panic!("unavailable live process must not be signalled by PID"),
            );
            assert!(matches!(result, Err(ResourceControlError::Unavailable(_))));
            assert_eq!(
                attempts.get(),
                if error == rustix::io::Errno::INVAL {
                    3
                } else {
                    1
                }
            );
        }
        assert!(
            signal_with(
                identity,
                Signal::KILL,
                |_| Err(rustix::io::Errno::INVAL),
                |_| Err(unavailable("injected birth inspection failure")),
                |_, _| panic!("uninspectable process must not be signalled"),
            )
            .is_err()
        );
    }

    #[test]
    fn a_transient_tgid_exit_resamples_the_birth_without_signalling_a_replacement()
    -> Result<(), TestError> {
        let identity = Identity {
            pid: 123,
            start: 456,
        };
        let attempts = Cell::new(0);
        signal_with(
            identity,
            Signal::KILL,
            |_| {
                attempts.set(attempts.get() + 1);
                Err(rustix::io::Errno::INVAL)
            },
            |_| {
                Ok(Some(if attempts.get() == 1 {
                    identity.start
                } else {
                    identity.start + 1
                }))
            },
            |_, _| panic!("replacement must never receive the old process signal"),
        )?;
        assert_eq!(attempts.get(), 2);
        Ok(())
    }

    #[test]
    fn successful_open_still_rechecks_birth_before_sending() -> Result<(), TestError> {
        let identity = Identity {
            pid: 123,
            start: 456,
        };
        let fd = std::fs::File::open("/dev/null")?;
        let mut fd = Some(OwnedFd::from(fd));
        signal_with(
            identity,
            Signal::KILL,
            |_| Ok(fd.take().unwrap_or_else(|| panic!("opener called once"))),
            |_| Ok(Some(identity.start + 1)),
            |_, _| panic!("opened descriptor with stale birth must never be signalled"),
        )?;
        Ok(())
    }
}
