//! Service-user child invocations with an explicit environment and fixed working directory.

use crate::{
    account::{SYSTEM_PATH, ServiceAccount},
    cli::ProbeCommand,
    config::{SERVER_BINARY, STATE_DIRECTORY},
    probe::ProbeExit,
};
use snafu::{OptionExt as _, ResultExt as _, Snafu};
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    os::unix::process::{CommandExt as _, ExitStatusExt as _},
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
};

/// Inspectable launch specification containing no credentials or private key material.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceInvocation {
    /// Executable selected by setup, never supplied by a credential prompt.
    pub program: PathBuf,
    /// Fixed operation arguments, excluding all secrets.
    pub args: Vec<OsString>,
    /// Complete child environment after clearing inherited variables.
    pub env: BTreeMap<OsString, OsString>,
    /// Non-root service identity passed to the standard library.
    pub uid: u32,
    /// Primary service group passed to the standard library.
    pub gid: u32,
    /// Packaged writable state directory used by the child.
    pub current_dir: PathBuf,
}

impl ServiceInvocation {
    /// Captures only the allowed environment, including TERM when present.
    #[must_use]
    pub fn new(account: &ServiceAccount, program: &Path, args: &[&OsStr]) -> Self {
        let mut env = BTreeMap::from([
            (OsString::from("PATH"), OsString::from(SYSTEM_PATH)),
            (OsString::from("HOME"), account.home.as_os_str().to_owned()),
            (OsString::from("LANG"), OsString::from("C.UTF-8")),
        ]);
        if let Some(term) = std::env::var_os("TERM") {
            env.insert("TERM".into(), term);
        }
        Self {
            program: program.into(),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            env,
            uid: account.uid,
            gid: account.gid,
            current_dir: STATE_DIRECTORY.into(),
        }
    }

    /// Builds the child; on root, std's uid/gid setup also clears supplementary groups.
    ///
    /// Relies on Unix `CommandExt` to perform privilege dropping before exec.
    #[must_use]
    pub fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command
            .args(&self.args)
            .env_clear()
            .envs(&self.env)
            .current_dir(&self.current_dir)
            .uid(self.uid)
            .gid(self.gid);
        command
    }
}

/// Constructs a service-user command using the same inspectable launch specification.
///
/// On root, std clears supplementary groups while applying the uid/gid change.
#[must_use]
pub fn service_command(account: &ServiceAccount, program: &Path, args: &[&OsStr]) -> Command {
    ServiceInvocation::new(account, program, args).command()
}

/// Runs a read-only setup probe with no stdin and inherited result channels.
///
/// # Errors
/// Returns spawn failures, signal termination, or an unknown probe status.
pub fn run_probe(account: &ServiceAccount, probe: ProbeCommand) -> Result<ProbeExit, ProcessError> {
    let status = service_command(
        account,
        Path::new("/proc/self/exe"),
        &[OsStr::new("probe"), OsStr::new(probe.argument())],
    )
    .stdin(Stdio::null())
    .stdout(Stdio::inherit())
    .stderr(Stdio::inherit())
    .status()
    .context(SpawnSnafu {
        program: "/proc/self/exe",
    })?;
    let code = exit_code(status)?;
    ProbeExit::from_code(code).context(UnknownProbeStatusSnafu { code })
}

/// Runs Server bootstrap with inherited TTY channels; setup never handles its credentials.
///
/// # Errors
/// Returns process creation failures or signal termination with its signal number.
pub fn run_bootstrap(account: &ServiceAccount) -> Result<BootstrapExit, ProcessError> {
    let status = service_command(
        account,
        Path::new(SERVER_BINARY),
        &[OsStr::new("bootstrap")],
    )
    .stdin(Stdio::inherit())
    .stdout(Stdio::inherit())
    .stderr(Stdio::inherit())
    .status()
    .context(SpawnSnafu {
        program: SERVER_BINARY,
    })?;
    Ok(match exit_code(status)? {
        0 => BootstrapExit::Succeeded,
        code => BootstrapExit::Failed { code },
    })
}

fn exit_code(status: ExitStatus) -> Result<i32, ProcessError> {
    status.code().context(SignaledSnafu {
        signal: status.signal(),
    })
}

/// Normal bootstrap termination, separate from launch and signal failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootstrapExit {
    /// Server completed credential initialization.
    Succeeded,
    /// Server rejected or failed initialization.
    Failed {
        /// Original nonzero Server exit code.
        code: i32,
    },
}

/// Service-user process launch or termination failure.
#[derive(Debug, Snafu)]
pub enum ProcessError {
    /// The operating system could not launch the child.
    #[snafu(display("cannot launch service command {program}"))]
    Spawn {
        /// Fixed executable being launched.
        program: &'static str,
        /// Operating system launch cause.
        source: std::io::Error,
    },
    /// The child did not terminate with an exit code.
    #[snafu(display("service command terminated by signal {signal:?}"))]
    Signaled {
        /// Signal number when supplied by the OS.
        signal: Option<i32>,
    },
    /// The child returned a status outside the shared probe protocol.
    #[snafu(display("unknown probe exit code: {code}"))]
    UnknownProbeStatus {
        /// Unrecognized child exit code.
        code: i32,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn termination_preserves_exit_code_or_signal() -> Result<(), ProcessError> {
        assert_eq!(exit_code(ExitStatus::from_raw(7 << 8))?, 7);
        assert!(matches!(
            exit_code(ExitStatus::from_raw(15)),
            Err(ProcessError::Signaled { signal: Some(15) })
        ));
        Ok(())
    }

    #[test]
    fn invocation_has_only_allowed_context() {
        let account = ServiceAccount {
            name: "natsume-server".into(),
            uid: 123,
            gid: 456,
            home: STATE_DIRECTORY.into(),
        };
        let invocation = ServiceInvocation::new(
            &account,
            Path::new(SERVER_BINARY),
            &[OsStr::new("bootstrap")],
        );
        assert_eq!(invocation.program, Path::new(SERVER_BINARY));
        assert_eq!(invocation.args, [OsString::from("bootstrap")]);
        assert_eq!((invocation.uid, invocation.gid), (123, 456));
        assert_eq!(invocation.current_dir, Path::new(STATE_DIRECTORY));
        let mut expected = BTreeMap::from([
            ("PATH".into(), SYSTEM_PATH.into()),
            ("HOME".into(), OsString::from(STATE_DIRECTORY)),
            ("LANG".into(), "C.UTF-8".into()),
        ]);
        if let Some(term) = std::env::var_os("TERM") {
            expected.insert("TERM".into(), term);
        }
        assert_eq!(invocation.env, expected);
        let command = invocation.command();
        assert_eq!(command.get_program(), invocation.program);
        assert_eq!(command.get_args().collect::<Vec<_>>(), invocation.args);
    }
}
