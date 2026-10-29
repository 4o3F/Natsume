//! Injectable operating-system boundary for the first-install wizard.

use crate::{
    account::{AccountError, SERVICE_USER, ServiceAccount},
    cli::ProbeCommand,
    health::{self, HealthError},
    pki::Certificate,
    preflight::{self, PreflightError, PreflightReport, SystemFacts},
    probe::ProbeExit,
    process::{self, BootstrapExit, ProcessError},
    service::{self, ServiceAction, ServiceError},
};
use snafu::{ResultExt as _, Snafu, ensure};
use std::{
    fs::File,
    io::{self, Read as _},
    net::SocketAddr,
    path::Path,
    time::Duration,
};
use time::OffsetDateTime;
use zeroize::Zeroizing;

/// Maximum size of administrator-supplied certificates and private keys.
pub const MAX_INPUT_BYTES: u64 = 1024 * 1024;
/// Number of startup checks before reporting the final health failure.
const HEALTH_ATTEMPTS: usize = 20;
/// Delay between startup checks while the Server opens its listener.
const HEALTH_INTERVAL: Duration = Duration::from_millis(1500);

/// System operations invoked by the wizard, excluding protected deployment-file publication.
pub trait Host {
    /// Checks host eligibility and reports the packaged service identity.
    ///
    /// # Errors
    /// Returns a failed first-install prerequisite.
    fn preflight(&mut self) -> Result<PreflightReport, PreflightError>;
    /// Returns the time used for certificate and deployment validation.
    fn now(&self) -> OffsetDateTime;
    /// Reads a regular administrator input file with a one MiB limit.
    ///
    /// # Errors
    /// Returns unsafe file types, excessive size, or I/O failures.
    fn read_input(&mut self, path: &Path) -> Result<Zeroizing<Vec<u8>>, InputError>;
    /// Runs a read-only probe as the service user.
    ///
    /// # Errors
    /// Returns process launch or termination failures.
    fn probe(&mut self, command: ProbeCommand) -> Result<ProbeExit, ProcessError>;
    /// Runs interactive Server credential initialization as the service user.
    ///
    /// # Errors
    /// Returns process launch or termination failures.
    fn bootstrap(&mut self) -> Result<BootstrapExit, ProcessError>;
    /// Applies one fixed systemd operation.
    ///
    /// # Errors
    /// Returns systemctl launch or exit failures.
    fn systemctl(&mut self, action: ServiceAction) -> Result<(), ServiceError>;
    /// Waits for a healthy HTTPS endpoint pinned to the supplied Control CA.
    ///
    /// # Errors
    /// Returns the last bounded health-check failure.
    fn wait_for_health(
        &mut self,
        control_ca: &Certificate,
        address: SocketAddr,
    ) -> Result<(), HealthError>;
}

/// Production adapter retaining the non-root service identity and a single-thread runtime.
#[derive(Debug)]
pub struct SystemHost {
    account: ServiceAccount,
    runtime: tokio::runtime::Runtime,
}

impl SystemHost {
    /// Resolves the service account and creates the runtime without evaluating wizard prerequisites.
    ///
    /// # Errors
    /// Returns service-account lookup or runtime construction failures.
    pub fn new() -> Result<Self, HostError> {
        let account = ServiceAccount::lookup(SERVICE_USER).context(AccountSnafu)?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context(RuntimeSnafu)?;
        Ok(Self { account, runtime })
    }

    /// Borrows the packaged service identity used for file ownership and child processes.
    #[must_use]
    pub const fn account(&self) -> &ServiceAccount {
        &self.account
    }
}

impl Host for SystemHost {
    fn preflight(&mut self) -> Result<PreflightReport, PreflightError> {
        preflight::evaluate(SystemFacts::collect())
    }
    fn now(&self) -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }
    fn read_input(&mut self, path: &Path) -> Result<Zeroizing<Vec<u8>>, InputError> {
        let file = File::from(
            rustix::fs::open(
                path,
                rustix::fs::OFlags::RDONLY
                    | rustix::fs::OFlags::NONBLOCK
                    | rustix::fs::OFlags::CLOEXEC,
                rustix::fs::Mode::empty(),
            )
            .map_err(io::Error::from)
            .context(ReadSnafu)?,
        );
        ensure!(
            file.metadata().context(ReadSnafu)?.is_file(),
            NotRegularSnafu
        );
        let mut bytes = Zeroizing::new(Vec::new());
        file.take(MAX_INPUT_BYTES + 1)
            .read_to_end(&mut bytes)
            .context(ReadSnafu)?;
        ensure!(bytes.len() as u64 <= MAX_INPUT_BYTES, TooLargeSnafu);
        Ok(bytes)
    }
    fn probe(&mut self, command: ProbeCommand) -> Result<ProbeExit, ProcessError> {
        process::run_probe(&self.account, command)
    }
    fn bootstrap(&mut self) -> Result<BootstrapExit, ProcessError> {
        process::run_bootstrap(&self.account)
    }
    fn systemctl(&mut self, action: ServiceAction) -> Result<(), ServiceError> {
        service::systemctl(action)
    }
    fn wait_for_health(
        &mut self,
        control_ca: &Certificate,
        address: SocketAddr,
    ) -> Result<(), HealthError> {
        self.runtime.block_on(health::wait_for_health(
            control_ca,
            address,
            HEALTH_ATTEMPTS,
            HEALTH_INTERVAL,
        ))
    }
}

/// Failure to read an administrator input without retaining its secret contents.
#[derive(Debug, Snafu)]
pub enum InputError {
    /// The input could not be opened or read.
    #[snafu(display("cannot read input file"))]
    Read {
        /// Original operating-system error without input contents.
        source: io::Error,
    },
    /// Special files are not supported as certificate or private-key inputs.
    #[snafu(display("input must be a regular file"))]
    NotRegular,
    /// The input exceeds the bounded parser budget.
    #[snafu(display("input exceeds the one MiB size limit"))]
    TooLarge,
}

/// Failure to construct the production operating-system adapter.
#[derive(Debug, Snafu)]
pub enum HostError {
    /// The packaged service identity could not be resolved.
    #[snafu(display("cannot resolve setup service account"))]
    Account {
        /// Original account lookup failure.
        source: AccountError,
    },
    /// Async health checks could not obtain a runtime.
    #[snafu(display("cannot create setup runtime"))]
    Runtime {
        /// Original runtime construction failure.
        source: io::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::symlink};

    #[derive(Debug, Snafu)]
    enum TestError {
        #[snafu(context(false), display("input fixture I/O failed"))]
        Io { source: io::Error },
        #[snafu(context(false), display("input fixture read failed"))]
        Input { source: InputError },
    }

    fn host() -> Result<SystemHost, io::Error> {
        Ok(SystemHost {
            account: ServiceAccount {
                name: SERVICE_USER.into(),
                uid: 1,
                gid: 1,
                home: crate::config::STATE_DIRECTORY.into(),
            },
            runtime: tokio::runtime::Builder::new_current_thread().build()?,
        })
    }

    #[test]
    fn input_symlink_to_regular_file_is_readable() -> Result<(), TestError> {
        let root = tempfile::tempdir()?;
        let target = root.path().join("certificate.pem");
        let link = root.path().join("certificate-link.pem");
        fs::write(&target, b"administrator input")?;
        symlink(&target, &link)?;
        assert_eq!(
            host()?.read_input(&link)?.as_slice(),
            b"administrator input"
        );
        Ok(())
    }

    #[test]
    fn input_symlink_to_directory_is_rejected() -> Result<(), TestError> {
        let root = tempfile::tempdir()?;
        let link = root.path().join("directory-link");
        symlink(root.path(), &link)?;
        assert!(matches!(
            host()?.read_input(&link),
            Err(InputError::NotRegular)
        ));
        Ok(())
    }

    #[test]
    fn input_fifo_and_symlink_to_fifo_are_rejected_without_blocking() -> Result<(), TestError> {
        let root = tempfile::tempdir()?;
        let fifo = root.path().join("input-fifo");
        let link = root.path().join("fifo-link");
        rustix::fs::mknodat(
            rustix::fs::CWD,
            &fifo,
            rustix::fs::FileType::Fifo,
            rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
            0,
        )
        .map_err(io::Error::from)?;
        symlink(&fifo, &link)?;
        let mut host = host()?;
        for path in [&fifo, &link] {
            assert!(matches!(host.read_input(path), Err(InputError::NotRegular)));
        }
        Ok(())
    }

    #[test]
    fn input_size_limit_applies_to_symlink_targets() -> Result<(), TestError> {
        let root = tempfile::tempdir()?;
        let target = root.path().join("input");
        let link = root.path().join("input-link");
        let file = File::create(&target)?;
        symlink(&target, &link)?;
        let mut host = host()?;
        file.set_len(MAX_INPUT_BYTES)?;
        assert_eq!(host.read_input(&link)?.len() as u64, MAX_INPUT_BYTES);
        file.set_len(MAX_INPUT_BYTES + 1)?;
        assert!(matches!(host.read_input(&link), Err(InputError::TooLarge)));
        Ok(())
    }
}
