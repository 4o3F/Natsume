#![forbid(unsafe_code)]
#![cfg(target_os = "linux")]

mod admission;
mod boot;
mod display;
mod gateway;
mod gdm_registration;
mod hardware_identity;
mod home;
mod login;
mod processes;
mod runtime;
mod session;
mod waiting;

pub use admission::pam_gate;

/// Clears the fixed contestant's keyrings after an isolated UID transition.
///
/// # Errors
/// Rejects nonisolated credentials or any inaccessible/protected keyring.
pub fn cleanup_keyrings() -> Result<(), natsume_workstation_ipc::ResourceControlError> {
    home::cleanup::keyring_worker(std::path::Path::new("/"))
}

/// Withdraws contest permission during service shutdown, including crash cleanup.
///
/// # Errors
/// Fails if not root or the runtime directory is unavailable.
pub fn close_admission() -> Result<(), natsume_workstation_ipc::ResourceControlError> {
    if !rustix::process::geteuid().is_root() {
        return Err(session::rejected("admission withdrawal requires root"));
    }
    admission::close(std::path::Path::new("/"))
}
pub use login::{GreeterStatus, probe_greeter, run_gdm_client, run_prepare};
pub use session::probe_desktop;

use std::path::PathBuf;

use natsume_workstation_ipc::{
    GraphicalSession, GraphicalSessionObservation, HomeResetProgress, MachineIdentityError,
    MachineIdentityReport, ManagedSessionsObservation, ResourceControlError, SessionRole,
};

/// Closed root capabilities exposed to the Device Daemon.
pub struct PrivilegedService {
    filesystem_root: PathBuf,
    home_reset_task: Option<home::window::ResetTask>,
}

impl PrivilegedService {
    /// Creates the production collector rooted at the host filesystem.
    #[must_use]
    pub fn production() -> Self {
        Self {
            filesystem_root: PathBuf::from("/"),
            home_reset_task: None,
        }
    }

    /// Starts the independent startup observer after this service owns its bus name.
    /// Existing Home/identity capabilities remain available during observation outages.
    #[must_use]
    pub fn observe_gdm_registration(&self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(gdm_registration::observe(self.filesystem_root.clone()))
    }
}

async fn bounded_session_operation<T>(
    operation: impl std::future::Future<Output = Result<T, ResourceControlError>>,
) -> Result<T, ResourceControlError> {
    bounded_session_attempt(operation).await.0
}

async fn bounded_session_attempt<T>(
    operation: impl std::future::Future<Output = Result<T, ResourceControlError>>,
) -> (Result<T, ResourceControlError>, bool) {
    match tokio::time::timeout(std::time::Duration::from_secs(8), operation).await {
        Ok(result) => (result, false),
        Err(_) => (
            Err(session::unavailable(
                "session operation timed out; resample before retry",
            )),
            true,
        ),
    }
}

fn duration_micros(duration: std::time::Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

fn local_result<T>(result: &Result<T, ResourceControlError>, timed_out: bool) -> &'static str {
    if timed_out {
        "timeout"
    } else if result.is_ok() {
        "ok"
    } else {
        "error"
    }
}

fn trace_home_request<T>(
    epoch: u64,
    operation: &'static str,
    elapsed: std::time::Duration,
    result: &Result<T, ResourceControlError>,
    timed_out: bool,
    worked: bool,
) {
    if worked || result.is_err() {
        tracing::info!(
            reset_epoch = epoch,
            operation,
            elapsed_us = duration_micros(elapsed),
            result = local_result(result, timed_out),
            "home reset completed"
        );
    }
}

#[zbus::interface(name = "org.natsume.Privileged1")]
impl PrivilegedService {
    #[zbus(name = "ConfigureLocalGateway")]
    fn configure_local_gateway(
        &mut self,
        gateway_hostname: &str,
    ) -> Result<(), ResourceControlError> {
        gateway::configure(&self.filesystem_root, gateway_hostname)
    }

    #[expect(
        clippy::unnecessary_wraps,
        reason = "The typed D-Bus contract retains MachineIdentityError for transport failures"
    )]
    #[zbus(name = "DeriveMachineIdentity")]
    fn derive_machine_identity(&self) -> Result<MachineIdentityReport, MachineIdentityError> {
        Ok(hardware_identity::derive_identity(&self.filesystem_root))
    }

    /// Retries local mount recovery without granting any remote foreground target.
    ///
    /// # Errors
    /// Keeps admission closed on invalid state, incomplete drainage or mount failure.
    #[zbus(name = "RecoverLocalHome")]
    pub async fn recover_local_home(
        &mut self,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<(), ResourceControlError> {
        let started = tokio::time::Instant::now();
        let mut observation = home::window::RecoveryObservation::default();
        let (result, timed_out) = bounded_session_attempt(home::window::restore_home(
            &self.filesystem_root,
            connection,
            &mut observation,
            &mut self.home_reset_task,
        ))
        .await;
        let elapsed = started.elapsed();
        tracing::info!(
            reset_epoch = observation.reset_epoch,
            action = observation.action.map(home::window::RecoveryAction::name),
            elapsed_us = duration_micros(elapsed),
            result = local_result(&result, timed_out),
            "home recovery"
        );
        result
    }

    #[zbus(name = "IsHomeReady")]
    fn is_home_ready(&self) -> bool {
        admission::require_open(&self.filesystem_root).is_ok()
            && home::require_template(&self.filesystem_root).is_ok()
            && admission::require_open(&self.filesystem_root).is_ok()
    }

    #[zbus(name = "HasHomeResetState")]
    fn has_home_reset_state(&self) -> Result<bool, ResourceControlError> {
        home::state_exists(&self.filesystem_root)
    }

    #[zbus(name = "QueryManagedSessions")]
    async fn query_managed_sessions(
        &self,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<ManagedSessionsObservation, ResourceControlError> {
        bounded_session_operation(login::observe(connection, &self.filesystem_root)).await
    }

    #[zbus(name = "ValidateWaitingSession")]
    async fn validate_waiting_session(
        &self,
        session: GraphicalSession,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<bool, ResourceControlError> {
        bounded_session_operation(login::validate_waiting_session(
            connection,
            &self.filesystem_root,
            &session,
        ))
        .await
    }

    #[zbus(name = "PrepareBootSessions")]
    async fn prepare_boot_sessions(
        &mut self,
        waiting: GraphicalSession,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<bool, ResourceControlError> {
        bounded_session_operation(boot::prepare(connection, &self.filesystem_root, &waiting)).await
    }

    #[zbus(name = "PrepareSession")]
    async fn prepare_session(
        &mut self,
        role: SessionRole,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<GraphicalSessionObservation, ResourceControlError> {
        bounded_session_operation(login::start(connection, &self.filesystem_root, role)).await
    }

    #[zbus(name = "RecoverWaitingSession")]
    async fn recover_waiting_session(
        &mut self,
        expected: Option<GraphicalSession>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<bool, ResourceControlError> {
        bounded_session_operation(waiting::rebuild(
            connection,
            &self.filesystem_root,
            expected.as_ref(),
        ))
        .await
    }

    #[zbus(name = "ResumeWaitingRecovery")]
    async fn resume_waiting_recovery(
        &mut self,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<bool, ResourceControlError> {
        bounded_session_operation(waiting::resume(connection, &self.filesystem_root)).await
    }

    #[zbus(name = "ActivateSession")]
    async fn activate_session(
        &mut self,
        role: SessionRole,
        target: GraphicalSession,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<(), ResourceControlError> {
        let started = tokio::time::Instant::now();
        let mut activate_us = None;
        let mut timed_out = false;
        let result = async {
            let _mutation = admission::mutation(&self.filesystem_root)?;
            if role == SessionRole::Contest {
                admission::require_open(&self.filesystem_root)?;
            }
            let activate_started = tokio::time::Instant::now();
            let (result, hit) = bounded_session_attempt(session::activate(
                connection,
                &self.filesystem_root,
                role,
                &target,
            ))
            .await;
            activate_us = Some(activate_started.elapsed());
            timed_out = hit;
            result
        }
        .await;
        let elapsed = started.elapsed();
        tracing::info!(
            operation = "activate",
            foreground_target = session::role_name(role),
            elapsed_us = duration_micros(elapsed),
            activate_us = activate_us.map(duration_micros),
            result = local_result(&result, timed_out),
            "session switch completed"
        );
        result
    }

    #[zbus(name = "CloseContestAdmission")]
    async fn close_contest_admission(
        &mut self,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<(), ResourceControlError> {
        bounded_session_operation(login::close_contest(connection, &self.filesystem_root)).await
    }

    #[zbus(name = "TerminateContestSession")]
    async fn terminate_contest_session(
        &mut self,
        target: GraphicalSession,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<(), ResourceControlError> {
        let _mutation = admission::mutation(&self.filesystem_root)?;
        bounded_session_operation(session::terminate(
            connection,
            &self.filesystem_root,
            &target,
        ))
        .await
    }

    #[zbus(name = "RequestPowerOff")]
    async fn request_power_off(
        &mut self,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<(), ResourceControlError> {
        bounded_session_operation(session::power_off(connection)).await
    }

    #[zbus(name = "PrepareHomeReset")]
    async fn prepare_home_reset(
        &mut self,
        reset_epoch: u64,
        waiting: GraphicalSession,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<(), ResourceControlError> {
        let started = tokio::time::Instant::now();
        let (result, timed_out) = bounded_session_attempt(home::window::prepare(
            &self.filesystem_root,
            connection,
            reset_epoch,
            &waiting,
        ))
        .await;
        trace_home_request(
            reset_epoch,
            "prepare_request",
            started.elapsed(),
            &result,
            timed_out,
            true,
        );
        result
    }

    #[zbus(name = "ApplyHomeReset")]
    async fn apply_home_reset(
        &mut self,
        reset_epoch: u64,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<(), ResourceControlError> {
        let started = tokio::time::Instant::now();
        let mut worked = false;
        let (result, timed_out) = bounded_session_attempt(home::window::apply(
            &self.filesystem_root,
            connection,
            reset_epoch,
            &mut worked,
        ))
        .await;
        trace_home_request(
            reset_epoch,
            "apply_request",
            started.elapsed(),
            &result,
            timed_out,
            worked,
        );
        result
    }

    #[zbus(name = "QueryHomeReset")]
    fn query_home_reset(&self) -> Result<Option<HomeResetProgress>, ResourceControlError> {
        let progress = home::window::query(&self.filesystem_root)?;
        Ok(self
            .home_reset_task
            .as_ref()
            .and_then(home::window::ResetTask::progress)
            .or(progress))
    }

    #[zbus(name = "VerifyHomeReset")]
    async fn verify_home_reset(
        &mut self,
        reset_epoch: u64,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<HomeResetProgress, ResourceControlError> {
        let started = tokio::time::Instant::now();
        let mut worked = false;
        let (result, timed_out) = bounded_session_attempt(home::window::verify(
            &self.filesystem_root,
            connection,
            reset_epoch,
            &mut worked,
            &mut self.home_reset_task,
        ))
        .await;
        trace_home_request(
            reset_epoch,
            "verify_request",
            started.elapsed(),
            &result,
            timed_out,
            worked,
        );
        result
    }

    #[zbus(name = "RecoverHomeReset")]
    async fn recover_home_reset(
        &mut self,
        reset_epoch: u64,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<(), ResourceControlError> {
        let started = tokio::time::Instant::now();
        let (result, timed_out) = bounded_session_attempt(home::window::recover(
            &self.filesystem_root,
            connection,
            reset_epoch,
            &mut self.home_reset_task,
        ))
        .await;
        trace_home_request(
            reset_epoch,
            "recover_request",
            started.elapsed(),
            &result,
            timed_out,
            true,
        );
        result
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::{fs, os::unix::fs::PermissionsExt as _, path::Path};

    use natsume_workstation_ipc::{
        HardwareSourceStatus, MachineIdentityOutcome, PRIVILEGED1_PATH, Privileged1Proxy,
    };
    use tempfile::TempDir;
    use tokio::net::UnixStream;

    use super::*;

    use std::{
        collections::BTreeMap,
        sync::{Arc, Mutex, MutexGuard},
    };
    use tracing::{
        Event, Subscriber,
        field::{Field, Visit},
    };
    use tracing_subscriber::{Layer, layer::Context, prelude::*};

    static SUBSCRIBER_LOCK: Mutex<()> = Mutex::new(());
    pub(crate) struct SubscriberGuard {
        _lock: MutexGuard<'static, ()>,
        _dispatch: tracing::Dispatch,
    }
    pub(crate) fn subscriber_guard() -> SubscriberGuard {
        SubscriberGuard {
            _lock: SUBSCRIBER_LOCK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            _dispatch: tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default()),
        }
    }
    #[derive(Clone, Debug, PartialEq)]
    pub(crate) enum Value {
        Text(String),
        Unsigned(u64),
        Boolean(bool),
    }
    #[derive(Clone, Debug)]
    pub(crate) struct Record {
        pub(crate) fields: BTreeMap<String, Value>,
        pub(crate) level: tracing::Level,
    }
    impl Visit for Record {
        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            self.record_str(field, &format!("{value:?}"));
        }
        fn record_str(&mut self, field: &Field, value: &str) {
            self.fields
                .insert(field.name().to_owned(), Value::Text(value.to_owned()));
        }
        fn record_u64(&mut self, field: &Field, value: u64) {
            self.fields
                .insert(field.name().to_owned(), Value::Unsigned(value));
        }
        fn record_bool(&mut self, field: &Field, value: bool) {
            self.fields
                .insert(field.name().to_owned(), Value::Boolean(value));
        }
    }
    #[derive(Clone, Default)]
    pub(crate) struct Capture(Arc<Mutex<Vec<Record>>>);
    impl<S: Subscriber> Layer<S> for Capture {
        fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
            let mut record = Record {
                fields: BTreeMap::new(),
                level: *event.metadata().level(),
            };
            event.record(&mut record);
            if matches!(record.fields.get("message"), Some(Value::Text(message)) if matches!(message.as_str(), "session switch completed" | "home reset completed" | "home recovery"))
            {
                self.0
                    .lock()
                    .unwrap_or_else(|e| panic!("capture: {e}"))
                    .push(record);
            }
        }
    }
    impl Capture {
        pub(crate) fn subscriber(&self) -> impl Subscriber + Send + Sync + use<> {
            tracing_subscriber::registry().with(self.clone())
        }
        pub(crate) fn events(&self, message: &str) -> Vec<Record> {
            self.0
                .lock()
                .unwrap_or_else(|e| panic!("capture: {e}"))
                .iter()
                .filter(|record| {
                    record.fields.get("message") == Some(&Value::Text(message.to_owned()))
                })
                .cloned()
                .collect()
        }
    }
    pub(crate) fn assert_text(record: &Record, field: &str, value: &str) {
        assert_eq!(record.level, tracing::Level::INFO);
        assert_eq!(
            record.fields.get(field),
            Some(&Value::Text(value.to_owned()))
        );
    }

    impl PrivilegedService {
        fn fixture(filesystem_root: &Path) -> Self {
            Self {
                filesystem_root: filesystem_root.to_owned(),
                home_reset_task: None,
            }
        }
    }

    fn tempdir() -> TempDir {
        match TempDir::new() {
            Ok(directory) => directory,
            Err(error) => panic!("fixture directory must be created: {error}"),
        }
    }

    fn write_fixture(root: &Path, relative: &str, bytes: &[u8]) {
        let path = root.join(relative);
        let Some(parent) = path.parent() else {
            panic!("fixture path must have a parent");
        };
        if let Err(error) = fs::create_dir_all(parent) {
            panic!("fixture parent must be created: {error}");
        }
        if let Err(error) = fs::write(path, bytes) {
            panic!("fixture file must be written: {error}");
        }
    }

    fn service_fixture() -> (TempDir, PrivilegedService) {
        let fixture = tempdir();
        write_fixture(
            fixture.path(),
            "sys/class/dmi/id/product_uuid",
            b"550E8400-E29B-41D4-A716-446655440000\n",
        );
        write_fixture(
            fixture.path(),
            "sys/class/dmi/id/board_serial",
            b"BOARD-42\n",
        );
        let dev_block = fixture.path().join("sys/dev/block");
        if let Err(error) = fs::create_dir_all(&dev_block) {
            panic!("sysfs block fixture must be created: {error}");
        }
        let service = PrivilegedService::fixture(fixture.path());
        (fixture, service)
    }

    type IdentitySession = (String, u32, String, String, zbus::OwnedObjectPath);

    #[derive(Clone)]
    struct IdentityFacts {
        sessions: Vec<IdentitySession>,
        kind: &'static str,
        class: &'static str,
        state: &'static str,
        preparation: &'static str,
        remote: bool,
        locked: bool,
        listing_error: bool,
        desktop_queries: usize,
    }

    #[derive(Clone)]
    struct IdentitySystem(Arc<Mutex<IdentityFacts>>);

    impl IdentitySystem {
        fn facts(&self) -> MutexGuard<'_, IdentityFacts> {
            self.0
                .lock()
                .unwrap_or_else(|error| panic!("identity fixture lock: {error}"))
        }
    }

    struct IdentityLogind(IdentitySystem);
    struct IdentityProperties(IdentitySystem);
    struct IdentitySystemd;
    struct IdentityPreparation(IdentitySystem);

    #[zbus::interface(name = "org.freedesktop.login1.Manager")]
    impl IdentityLogind {
        fn list_sessions(&self) -> zbus::fdo::Result<Vec<IdentitySession>> {
            let facts = self.0.facts();
            if facts.listing_error {
                return Err(zbus::fdo::Error::Failed(
                    "fixture logind failure".to_owned(),
                ));
            }
            Ok(facts.sessions.clone())
        }

        fn get_user(&self, uid: u32) -> zbus::fdo::Result<zbus::OwnedObjectPath> {
            assert_eq!(uid, 1002);
            self.0.facts().desktop_queries += 1;
            Err(zbus::fdo::Error::Failed(
                "unexpected desktop query".to_owned(),
            ))
        }

        fn get_seat(&self, seat: &str) -> zbus::fdo::Result<zbus::OwnedObjectPath> {
            assert_eq!(seat, "seat0");
            self.0.facts().desktop_queries += 1;
            Err(zbus::fdo::Error::Failed(
                "unexpected foreground query".to_owned(),
            ))
        }
    }

    #[zbus::interface(name = "org.freedesktop.login1.Session")]
    impl IdentityProperties {
        #[zbus(property)]
        fn remote(&self) -> bool {
            self.0.facts().remote
        }
        #[zbus(property)]
        fn class(&self) -> &str {
            self.0.facts().class
        }
        #[zbus(property, name = "Type")]
        fn kind(&self) -> &str {
            self.0.facts().kind
        }
        #[zbus(property)]
        fn state(&self) -> &str {
            self.0.facts().state
        }
        #[zbus(property)]
        fn locked_hint(&self) -> bool {
            self.0.facts().locked
        }
    }

    #[zbus::interface(name = "org.freedesktop.systemd1.Manager")]
    impl IdentitySystemd {
        #[expect(
            clippy::unused_self,
            reason = "D-Bus methods require an interface receiver"
        )]
        fn load_unit(&self, name: &str) -> zbus::OwnedObjectPath {
            assert_eq!(
                name,
                natsume_workstation_ipc::preparation_unit(SessionRole::Waiting)
            );
            zbus::OwnedObjectPath::try_from("/identity/unit")
                .unwrap_or_else(|error| panic!("identity unit path: {error}"))
        }
    }

    #[zbus::interface(name = "org.freedesktop.systemd1.Unit")]
    impl IdentityPreparation {
        #[zbus(property)]
        fn active_state(&self) -> &str {
            self.0.facts().preparation
        }
    }

    async fn waiting_identity_fixture(
        fixture: &TempDir,
        client: &zbus::Connection,
    ) -> (IdentitySystem, GraphicalSession) {
        let boot = "550e8400-e29b-41d4-a716-446655440000";
        write_fixture(
            fixture.path(),
            "proc/sys/kernel/random/boot_id",
            boot.as_bytes(),
        );
        write_fixture(fixture.path(), "proc/uptime", b"100.00 0.00\n");
        write_fixture(
            fixture.path(),
            "etc/passwd",
            b"waiting:x:1002:1002::/home/waiting:/bin/bash\n",
        );
        let baseline = IdentityFacts {
            sessions: vec![(
                "w1".to_owned(),
                1002,
                "waiting".to_owned(),
                "seat0".to_owned(),
                zbus::OwnedObjectPath::try_from("/identity/session")
                    .unwrap_or_else(|error| panic!("identity session path: {error}")),
            )],
            kind: "wayland",
            class: "user",
            state: "online",
            preparation: "inactive",
            remote: false,
            locked: false,
            listing_error: false,
            desktop_queries: 0,
        };
        let system = IdentitySystem(Arc::new(Mutex::new(baseline.clone())));
        let objects = client.object_server();
        objects
            .at("/org/freedesktop/login1", IdentityLogind(system.clone()))
            .await
            .unwrap_or_else(|error| panic!("logind fixture: {error}"));
        objects
            .at("/identity/session", IdentityProperties(system.clone()))
            .await
            .unwrap_or_else(|error| panic!("session fixture: {error}"));
        objects
            .at("/org/freedesktop/systemd1", IdentitySystemd)
            .await
            .unwrap_or_else(|error| panic!("systemd fixture: {error}"));
        objects
            .at("/identity/unit", IdentityPreparation(system.clone()))
            .await
            .unwrap_or_else(|error| panic!("unit fixture: {error}"));
        let expected = GraphicalSession {
            logind_session_id: "w1".to_owned(),
            boot_id: boot.to_owned(),
        };
        (system, expected)
    }

    async fn check_waiting_identity(
        fixture: &TempDir,
        client: &zbus::Connection,
        proxy: &Privileged1Proxy<'_>,
    ) {
        let (system, expected) = waiting_identity_fixture(fixture, client).await;
        let baseline = system.facts().clone();
        let boot = &expected.boot_id;
        for case in [
            "valid",
            "locked",
            "active",
            "wrong-session",
            "wrong-boot",
            "missing",
            "duplicate",
            "uid",
            "user",
            "seat",
            "x11",
            "remote",
            "class",
            "opening",
            "closing",
            "unknown",
            "activating",
            "preparing",
            "deactivating",
            "logind-error",
        ] {
            *system.facts() = baseline.clone();
            let mut claimed = expected.clone();
            {
                let mut facts = system.facts();
                match case {
                    "locked" => facts.locked = true,
                    "active" => facts.state = "active",
                    "wrong-session" => claimed.logind_session_id = "w2".to_owned(),
                    "wrong-boot" => {
                        claimed.boot_id = "550e8400-e29b-41d4-a716-446655440001".to_owned();
                    }
                    "missing" => facts.sessions.clear(),
                    "duplicate" => {
                        let session = facts.sessions[0].clone();
                        facts.sessions.push(session);
                    }
                    "uid" => facts.sessions[0].1 = 1003,
                    "user" => facts.sessions[0].2 = "other".to_owned(),
                    "seat" => facts.sessions[0].3 = "seat1".to_owned(),
                    "x11" => facts.kind = "x11",
                    "remote" => facts.remote = true,
                    "class" => facts.class = "greeter",
                    "opening" => facts.state = "opening",
                    "closing" => facts.state = "closing",
                    "unknown" => facts.state = "unknown",
                    "activating" => facts.preparation = "activating",
                    "preparing" => facts.preparation = "active",
                    "deactivating" => facts.preparation = "deactivating",
                    "logind-error" => facts.listing_error = true,
                    "valid" => {}
                    _ => panic!("unknown identity case: {case}"),
                }
            }
            let result = proxy.validate_waiting_session(&claimed).await;
            if case == "logind-error" {
                assert!(matches!(result, Err(ResourceControlError::Unavailable(_))));
            } else {
                assert_eq!(
                    result.unwrap_or_else(|error| panic!("{case}: {error}")),
                    matches!(case, "valid" | "locked" | "active"),
                    "{case}"
                );
            }
            assert_eq!(system.facts().desktop_queries, 0, "{case}");
        }
        *system.facts() = baseline;
        for (record, fails) in [
            (format!("1\n{boot}\nw1\ndraining\n50\n"), false),
            ("corrupt\n".to_owned(), true),
        ] {
            write_fixture(
                fixture.path(),
                "run/natsume-privileged/waiting-recovery",
                record.as_bytes(),
            );
            let result = proxy.validate_waiting_session(&expected).await;
            if fails {
                assert!(matches!(result, Err(ResourceControlError::Rejected(_))));
            } else {
                assert!(!result.unwrap_or_else(|error| panic!("draining identity: {error}")));
            }
        }
        assert_eq!(system.facts().desktop_queries, 0);
    }

    #[tokio::test]
    #[expect(
        clippy::too_many_lines,
        reason = "One real peer-bus round trip checks the fixed capabilities and retained waiting budget together"
    )]
    async fn generated_proxy_round_trips_the_real_service() {
        let (fixture, service) = service_fixture();
        let streams = UnixStream::pair();
        let Ok((server_stream, client_stream)) = streams else {
            panic!("unix socketpair must be created");
        };
        let guid = zbus::Guid::generate();
        let server_builder = zbus::connection::Builder::unix_stream(
            server_stream
                .into_std()
                .unwrap_or_else(|error| panic!("server socket: {error}")),
        )
        .server(guid)
        .p2p()
        .serve_at(PRIVILEGED1_PATH, service);
        let client_builder = zbus::connection::Builder::unix_stream(
            client_stream
                .into_std()
                .unwrap_or_else(|error| panic!("client socket: {error}")),
        )
        .p2p();
        let (server, client) = tokio::join!(server_builder.build(), client_builder.build());
        let Ok(_server) = server else {
            panic!("peer server connection must be built: {server:?}");
        };
        let Ok(client) = client else {
            panic!("peer client connection must be built: {client:?}");
        };
        let proxy = match Privileged1Proxy::new(&client).await {
            Ok(proxy) => proxy,
            Err(error) => panic!("generated proxy must be built: {error}"),
        };

        write_fixture(fixture.path(), "etc/hosts", b"127.0.0.1 localhost\n");
        proxy
            .configure_local_gateway("gateway.test")
            .await
            .unwrap_or_else(|e| panic!("Gateway sync must round trip: {e}"));
        assert!(
            fs::read_to_string(fixture.path().join("etc/hosts"))
                .unwrap_or_else(|e| panic!("hosts: {e}"))
                .contains("127.0.0.1 gateway.test")
        );
        assert!(matches!(
            proxy.configure_local_gateway("bad hostname").await,
            Err(ResourceControlError::Rejected(_))
        ));

        proxy
            .configure_local_gateway("other.test")
            .await
            .unwrap_or_else(|e| panic!("Daemon-supplied Gateway change must round trip: {e}"));
        let hosts = fs::read_to_string(fixture.path().join("etc/hosts"))
            .unwrap_or_else(|e| panic!("hosts: {e}"));
        assert!(hosts.contains("127.0.0.1 other.test"));
        assert!(!hosts.contains("gateway.test"));
        assert_eq!(
            fs::read_to_string(fixture.path().join("etc/natsume/submit.env"))
                .unwrap_or_else(|e| panic!("submit settings: {e}")),
            "SUBMITBASEURL='https://other.test/'\n"
        );

        let report = match proxy.derive_machine_identity().await {
            Ok(report) => report,
            Err(error) => panic!("machine identity report must round trip: {error}"),
        };
        assert_eq!(report.outcome, MachineIdentityOutcome::Derived);
        assert_eq!(
            report.sources.dmi_system_uuid,
            HardwareSourceStatus::Present
        );
        assert_eq!(
            report.sources.dmi_board_serial,
            HardwareSourceStatus::Present
        );
        let identity = report
            .identity
            .unwrap_or_else(|| panic!("derived report must contain identity"));

        assert_eq!(
            identity.machine_hardware_id,
            "939d8a7f-642f-583e-ad40-ba40c44b6b0e"
        );
        assert_eq!(
            identity.quality,
            natsume_workstation_ipc::MachineIdentityQuality::Strong
        );
        write_fixture(
            fixture.path(),
            "sys/class/dmi/id/board_serial",
            b"unknown\n",
        );
        let insufficient = proxy
            .derive_machine_identity()
            .await
            .unwrap_or_else(|e| panic!("insufficient report must round trip: {e}"));
        assert_eq!(
            insufficient.outcome,
            MachineIdentityOutcome::InsufficientSources
        );
        assert_eq!(insufficient.identity, None);
        assert_eq!(
            insufficient.sources.dmi_system_uuid,
            HardwareSourceStatus::Present
        );
        assert_eq!(
            insufficient.sources.dmi_board_serial,
            HardwareSourceStatus::RejectedPlaceholder
        );
        fs::remove_dir_all(fixture.path().join("sys"))
            .unwrap_or_else(|e| panic!("platform fixture must be removed: {e}"));
        let unsupported = proxy
            .derive_machine_identity()
            .await
            .unwrap_or_else(|e| panic!("unsupported report must round trip: {e}"));
        assert_eq!(unsupported.outcome, MachineIdentityOutcome::Unsupported);
        assert_eq!(unsupported.identity, None);
        assert_eq!(
            unsupported.sources.dmi_system_uuid,
            HardwareSourceStatus::Unsupported
        );
        assert_eq!(
            unsupported.sources.dmi_board_serial,
            HardwareSourceStatus::Unsupported
        );
        assert_eq!(
            unsupported.sources.first_disk_serial,
            HardwareSourceStatus::Unsupported
        );

        assert!(
            !proxy
                .has_home_reset_state()
                .await
                .unwrap_or_else(|error| panic!("empty Home state query failed: {error}"))
        );
        write_fixture(
            fixture.path(),
            "var/lib/natsume-privileged/home-reset/progress",
            b"7\nverified\n",
        );
        assert!(
            proxy
                .has_home_reset_state()
                .await
                .unwrap_or_else(|error| panic!("present Home state query failed: {error}"))
        );
        assert!(
            !proxy
                .resume_waiting_recovery()
                .await
                .unwrap_or_else(|e| panic!("resume: {e}"))
        );
        let boot = "550e8400-e29b-41d4-a716-446655440000";
        let spent = format!("1\n{boot}\nw1\nfinished\n60\n");
        write_fixture(
            fixture.path(),
            "proc/sys/kernel/random/boot_id",
            boot.as_bytes(),
        );
        write_fixture(fixture.path(), "proc/uptime", b"100.00 0.00\n");
        write_fixture(
            fixture.path(),
            "run/natsume-privileged/waiting-recovery",
            spent.as_bytes(),
        );
        fs::set_permissions(
            fixture.path().join("run/natsume-privileged"),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap_or_else(|e| panic!("runtime permissions: {e}"));
        let replacement = Some(GraphicalSession {
            boot_id: boot.to_owned(),
            logind_session_id: "w2".to_owned(),
        });
        // No mock logind exists on this peer bus. A spent replay must return
        // without trying to capture or terminate the newly supplied session.
        assert!(
            !proxy
                .recover_waiting_session(&replacement)
                .await
                .unwrap_or_else(|e| panic!("spent replay: {e}"))
        );
        assert!(
            !proxy
                .resume_waiting_recovery()
                .await
                .unwrap_or_else(|e| panic!("spent resume: {e}"))
        );
        assert_eq!(
            fs::read_to_string(
                fixture
                    .path()
                    .join("run/natsume-privileged/waiting-recovery")
            )
            .unwrap_or_else(|e| panic!("record: {e}")),
            spent
        );
        check_waiting_identity(&fixture, &client, &proxy).await;
    }
    #[tokio::test]
    async fn bounded_attempt_retains_the_eight_second_deadline_and_result_classification() {
        let (result, hit) =
            bounded_session_attempt(async { Ok::<_, ResourceControlError>(7) }).await;
        assert_eq!(result.unwrap_or_else(|e| panic!("result: {e}")), 7);
        assert!(!hit);
        let (result, hit) =
            bounded_session_attempt(async { Err::<(), _>(session::rejected("fixture")) }).await;
        assert!(!hit);
        assert_eq!(local_result(&result, hit), "error");
        let started = tokio::time::Instant::now();
        let (result, hit) =
            bounded_session_attempt(std::future::pending::<Result<(), ResourceControlError>>())
                .await;
        assert!(hit);
        assert!(started.elapsed() >= std::time::Duration::from_secs(8));
        assert!(
            matches!(&result, Err(ResourceControlError::Unavailable(message)) if message == "session operation timed out; resample before retry")
        );
        let _guard = subscriber_guard();
        let capture = Capture::default();
        let _subscriber = tracing::subscriber::set_default(capture.subscriber());
        trace_home_request(7, "apply_request", started.elapsed(), &result, hit, true);
        let events = capture.events("home reset completed");
        assert_eq!(events.len(), 1);
        assert_text(&events[0], "result", "timeout");
        assert!(!events[0].fields.contains_key("deadline_hit"));
    }
}
