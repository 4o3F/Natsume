use super::*;
use crate::{
    account::ServiceAccount,
    cli::ProbeCommand,
    health::HealthError,
    host::{InputError, MAX_INPUT_BYTES},
    preflight::{PreflightError, PreflightReport, PreflightWarning},
    probe::ProbeExit,
    process::{BootstrapExit, ProcessError},
    service::{ServiceAction, ServiceError},
};
use std::collections::{BTreeMap, VecDeque};
use zeroize::Zeroizing;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Call {
    Preflight,
    Read(PathBuf),
    Probe(ProbeCommand),
    Bootstrap,
    Systemctl(ServiceAction),
    Health(SocketAddr),
}

pub(super) struct FakeHost {
    pub now: OffsetDateTime,
    pub preflight: Option<Result<PreflightReport, PreflightError>>,
    pub warnings: Vec<PreflightWarning>,
    inputs: BTreeMap<PathBuf, Zeroizing<Vec<u8>>>,
    pub probes: VecDeque<(ProbeCommand, Result<ProbeExit, ProcessError>)>,
    pub bootstraps: VecDeque<Result<BootstrapExit, ProcessError>>,
    pub services: VecDeque<(ServiceAction, Result<(), ServiceError>)>,
    pub health: Option<Result<(), HealthError>>,
    pub bootstrap_root: Option<PathBuf>,
    pub calls: Vec<Call>,
}

impl FakeHost {
    pub fn new(now: OffsetDateTime) -> Self {
        Self {
            now,
            preflight: None,
            warnings: Vec::new(),
            inputs: BTreeMap::new(),
            probes: VecDeque::new(),
            bootstraps: VecDeque::new(),
            services: VecDeque::new(),
            health: None,
            bootstrap_root: None,
            calls: Vec::new(),
        }
    }
    pub fn input(&mut self, path: &str, bytes: &[u8]) {
        self.inputs
            .insert(path.into(), Zeroizing::new(bytes.to_vec()));
    }
    pub fn install_probes(&mut self, bootstrap: ProbeExit) {
        self.probes.extend([
            (ProbeCommand::Deployment, Ok(ProbeExit::Passed)),
            (ProbeCommand::Bootstrap, Ok(bootstrap)),
        ]);
        if matches!(
            bootstrap,
            ProbeExit::NotInitialized | ProbeExit::DatabaseMissing
        ) {
            self.bootstraps.push_back(Ok(BootstrapExit::Succeeded));
        }
    }
    pub fn assert_finished(&self) {
        assert!(self.probes.is_empty(), "unconsumed probe results");
        assert!(self.bootstraps.is_empty(), "unconsumed bootstrap results");
        assert!(self.services.is_empty(), "unconsumed systemctl results");
        assert!(self.health.is_none(), "unconsumed health result");
    }
}

impl Host for FakeHost {
    fn preflight(&mut self) -> Result<PreflightReport, PreflightError> {
        self.calls.push(Call::Preflight);
        self.preflight.take().unwrap_or_else(|| {
            Ok(PreflightReport {
                warnings: self.warnings.clone(),
                account: ServiceAccount {
                    name: "natsume-server".into(),
                    uid: rustix::process::geteuid().as_raw(),
                    gid: rustix::process::getegid().as_raw(),
                    home: config::STATE_DIRECTORY.into(),
                },
            })
        })
    }
    fn now(&self) -> OffsetDateTime {
        self.now
    }
    fn read_input(&mut self, path: &Path) -> Result<Zeroizing<Vec<u8>>, InputError> {
        self.calls.push(Call::Read(path.into()));
        let Some(bytes) = self.inputs.get(path) else {
            return Err(InputError::Read {
                source: io::Error::from(io::ErrorKind::NotFound),
            });
        };
        if bytes.len() as u64 > MAX_INPUT_BYTES {
            return Err(InputError::TooLarge);
        }
        Ok(bytes.clone())
    }
    fn probe(&mut self, command: ProbeCommand) -> Result<ProbeExit, ProcessError> {
        self.calls.push(Call::Probe(command));
        let Some((expected, result)) = self.probes.pop_front() else {
            panic!("unexpected probe call")
        };
        assert_eq!(command, expected, "wrong probe operation");
        result
    }
    fn bootstrap(&mut self) -> Result<BootstrapExit, ProcessError> {
        self.calls.push(Call::Bootstrap);
        if let Some(root) = &self.bootstrap_root {
            for path in [config::DATABASE, config::ROOT_KEY] {
                fs::write(
                    root.join(path.trim_start_matches('/')),
                    b"bootstrap-owned-state",
                )
                .map_err(|source| ProcessError::Spawn {
                    program: "test bootstrap state",
                    source,
                })?;
            }
        }
        let Some(result) = self.bootstraps.pop_front() else {
            panic!("unexpected bootstrap invocation")
        };
        result
    }
    fn systemctl(&mut self, action: ServiceAction) -> Result<(), ServiceError> {
        self.calls.push(Call::Systemctl(action));
        if let Some((expected, result)) = self.services.pop_front() {
            assert_eq!(action, expected, "wrong systemctl operation");
            result
        } else {
            Ok(())
        }
    }
    fn wait_for_health(
        &mut self,
        _control_ca: &Certificate,
        address: SocketAddr,
    ) -> Result<(), HealthError> {
        self.calls.push(Call::Health(address));
        self.health.take().unwrap_or(Ok(()))
    }
}
