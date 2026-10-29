//! Injectable operating-system boundary for the first-install wizard.

use crate::{
    account::{SERVICE_USER, ServiceAccount},
    cli::ProbeCommand,
    health::{self, HealthError},
    pki::Certificate,
    preflight::{self, PreflightError, PreflightReport, SystemFacts},
    probe::ProbeExit,
    process::{self, BootstrapExit, ProcessError},
    service::{self, ServiceAction, ServiceError},
};
use snafu::{OptionExt as _, ResultExt as _, Snafu, ensure};
use std::{
    fs::File,
    io::{self, Read as _},
    net::SocketAddr,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
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
    /// Reports whether the current installation transaction was interrupted.
    fn interrupted(&self) -> bool;
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
    account: Option<ServiceAccount>,
    account_name: String,
    runtime: tokio::runtime::Runtime,
    interrupted: Arc<AtomicBool>,
    signal_id: signal_hook::SigId,
}

impl SystemHost {
    /// Creates the runtime without looking up service accounts or evaluating host prerequisites.
    ///
    /// # Errors
    /// Returns runtime construction or interrupt-handler registration failures.
    pub fn new() -> Result<Self, HostError> {
        Self::with_account_name(SERVICE_USER)
    }

    fn with_account_name(account_name: &str) -> Result<Self, HostError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context(RuntimeSnafu)?;
        let interrupted = Arc::new(AtomicBool::new(false));
        let signal_id =
            signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(&interrupted))
                .context(SignalSnafu)?;
        Ok(Self {
            interrupted,
            signal_id,
            account: None,
            account_name: account_name.into(),
            runtime,
        })
    }
}

impl Drop for SystemHost {
    fn drop(&mut self) {
        signal_hook::low_level::unregister(self.signal_id);
    }
}

impl Host for SystemHost {
    fn interrupted(&self) -> bool {
        self.interrupted.load(Ordering::SeqCst)
    }

    fn preflight(&mut self) -> Result<PreflightReport, PreflightError> {
        self.account = None;
        let report = preflight::evaluate(SystemFacts::collect(&self.account_name))?;
        self.account = Some(report.account.clone());
        Ok(report)
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
        process::run_probe(
            self.account
                .as_ref()
                .context(process::NotPreflightedSnafu)?,
            command,
        )
    }
    fn bootstrap(&mut self) -> Result<BootstrapExit, ProcessError> {
        process::run_bootstrap(
            self.account
                .as_ref()
                .context(process::NotPreflightedSnafu)?,
        )
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
            || self.interrupted(),
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
    /// Installation cancellation could not obtain a SIGINT handler.
    #[snafu(display("cannot register setup interrupt handler"))]
    Signal {
        /// Original signal registration failure.
        source: io::Error,
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
        #[snafu(context(false), display("host construction failed"))]
        Host { source: HostError },
        #[snafu(context(false), display("input fixture I/O failed"))]
        Io { source: io::Error },
        #[snafu(context(false), display("input fixture read failed"))]
        Input { source: InputError },
    }

    fn host() -> Result<SystemHost, HostError> {
        SystemHost::new()
    }

    #[test]
    fn missing_service_account_does_not_prevent_host_construction() -> Result<(), TestError> {
        let name = "natsume-setup-nonexistent-test-account";
        assert!(matches!(
            ServiceAccount::lookup(name),
            Err(crate::account::AccountError::Missing)
        ));
        let mut host = SystemHost::with_account_name(name)?;
        assert!(host.account.is_none());
        assert!(matches!(
            host.probe(ProbeCommand::Deployment),
            Err(ProcessError::NotPreflighted)
        ));
        assert!(matches!(
            host.bootstrap(),
            Err(ProcessError::NotPreflighted)
        ));
        Ok(())
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
