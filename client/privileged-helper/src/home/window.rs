use std::{fs, path::Path};

use futures_util::StreamExt as _;
use natsume_local_control_api::{HomeResetPhase, HomeResetProgress, ResourceControlError};
use tokio::time::{Duration, timeout};
use zbus::{Connection, Proxy, proxy::CacheProperties, zvariant::OwnedObjectPath};

use super::{rejected, unavailable};

const DISPLAY_MANAGER: &str = "display-manager.service";
const HELPER: &str = "natsume-privileged-helper.service";
const SYSTEMD: &str = "org.freedesktop.systemd1";
const SYSTEMD_PATH: &str = "/org/freedesktop/systemd1";
const READY: &str = "/run/natsume-privileged/home-ready";
const WINDOW: &str = "login-window";
const JOB_TIMEOUT: Duration = Duration::from_secs(5);

/// Durable authority to finish this reset and restore only the login service it stopped.
struct Window {
    epoch: u64,
    restart_display: bool,
}

impl Window {
    fn read(root: &Path) -> Result<Option<Self>, ResourceControlError> {
        let encoded = match fs::read_to_string(super::state_directory(root).join(WINDOW)) {
            Ok(encoded) => encoded,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(unavailable("Home maintenance window is unreadable")),
        };
        let mut fields = encoded.lines();
        let version = fields.next();
        let epoch = fields.next().and_then(|value| value.parse::<u64>().ok());
        let restart_display = match fields.next() {
            Some("restart") => true,
            Some("leave-stopped") => false,
            _ => return Err(rejected("Home maintenance window is invalid")),
        };
        let epoch = epoch.ok_or_else(|| rejected("Home maintenance window is invalid"))?;
        super::require_epoch(epoch)?;
        if version != Some("1") || fields.next().is_some() {
            return Err(rejected("Home maintenance window is invalid"));
        }
        Ok(Some(Self {
            epoch,
            restart_display,
        }))
    }

    fn persist(&self, root: &Path) -> Result<(), ResourceControlError> {
        let restore = if self.restart_display {
            "restart"
        } else {
            "leave-stopped"
        };
        super::write_state(root, WINDOW, &format!("1\n{}\n{restore}\n", self.epoch))
    }
}

fn set_ready(root: &Path, ready: bool) -> Result<(), ResourceControlError> {
    let path = root.join(READY.trim_start_matches('/'));
    if ready {
        // The containing directory is package-created and root-only. /run is
        // cleared at boot, so this permit never stands in for a new mount check.
        fs::write(path, b"").map_err(|_| unavailable("Home login permit cannot be published"))
    } else {
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(unavailable("Home login permit cannot be withdrawn")),
        }
    }
}

async fn manager(connection: &Connection) -> zbus::Result<Proxy<'_>> {
    Proxy::new(
        connection,
        SYSTEMD,
        SYSTEMD_PATH,
        "org.freedesktop.systemd1.Manager",
    )
    .await
}

async fn display_state(connection: &Connection) -> Result<String, ResourceControlError> {
    let operation = async {
        let manager = manager(connection).await?;
        let path: OwnedObjectPath = manager.call("LoadUnit", &(DISPLAY_MANAGER,)).await?;
        let unit = zbus::proxy::Builder::<Proxy<'_>>::new(connection)
            .destination(SYSTEMD)?
            .path(path)?
            .interface("org.freedesktop.systemd1.Unit")?
            .cache_properties(CacheProperties::No)
            .build()
            .await?;
        let conditions: Vec<(String, bool, bool, String, i32)> =
            unit.get_property("Conditions").await?;
        let after: Vec<String> = unit.get_property("After").await?;
        let wants: Vec<String> = unit.get_property("Wants").await?;
        if !conditions.iter().any(|(kind, trigger, negate, path, _)| {
            kind == "ConditionPathExists" && !trigger && !negate && path == READY
        }) || !after.iter().any(|name| name == HELPER)
            || !wants.iter().any(|name| name == HELPER)
        {
            return Ok(Err(rejected(
                "display manager lacks the Home login interlock",
            )));
        }
        Ok::<_, zbus::Error>(Ok(unit.get_property::<String>("ActiveState").await?))
    };
    timeout(JOB_TIMEOUT, operation)
        .await
        .map_err(|_| unavailable("display manager inspection timed out"))?
        .map_err(|_| unavailable("display manager is unavailable"))?
}

async fn display_job(connection: &Connection, start: bool) -> Result<(), ResourceControlError> {
    let operation = async {
        let manager = manager(connection).await?;
        let mut completed = manager.receive_signal("JobRemoved").await?;
        match manager.call::<_, _, ()>("Subscribe", &()).await {
            Ok(()) => {}
            Err(zbus::Error::MethodError(name, _, _))
                if name.as_str() == "org.freedesktop.systemd1.AlreadySubscribed" => {}
            Err(error) => return Err(error),
        }
        let job: OwnedObjectPath = manager
            .call(
                if start { "StartUnit" } else { "StopUnit" },
                &(DISPLAY_MANAGER, "replace"),
            )
            .await?;
        while let Some(message) = completed.next().await {
            let (_, path, _, result): (u32, OwnedObjectPath, String, String) =
                message.body().deserialize()?;
            if path == job {
                return Ok(result == "done");
            }
        }
        Ok(false)
    };
    let done = timeout(JOB_TIMEOUT, operation)
        .await
        .map_err(|_| unavailable("display manager transition timed out"))?
        .map_err(|_| unavailable("display manager transition failed"))?;
    if !done || display_state(connection).await? != if start { "active" } else { "inactive" } {
        return Err(unavailable("display manager transition did not complete"));
    }
    Ok(())
}

async fn enter(
    root: &Path,
    connection: &Connection,
    epoch: u64,
) -> Result<(), ResourceControlError> {
    super::require_epoch(epoch)?;
    if let Some(window) = Window::read(root)? {
        if window.epoch != epoch {
            return Err(rejected("another Home maintenance epoch is in progress"));
        }
    } else {
        crate::require_no_contest_session(connection, root).await?;
        let state = display_state(connection).await?;
        let restart_display = match state.as_str() {
            "active" | "activating" => true,
            "inactive" | "failed" | "deactivating" => false,
            _ => return Err(unavailable("display manager state is indeterminate")),
        };
        Window {
            epoch,
            restart_display,
        }
        .persist(root)?;
    }
    set_ready(root, false)?;
    // An inactive unit cannot pass its required permit check. At boot its start
    // job is also ordered after Helper readiness; do not cancel that queued job.
    if display_state(connection).await? != "inactive" {
        display_job(connection, false).await?;
    }
    crate::require_no_contest_session(connection, root).await
}

pub(crate) async fn prepare(
    root: &Path,
    connection: &Connection,
    epoch: u64,
) -> Result<(), ResourceControlError> {
    super::prepare_metadata(root, epoch)?;
    if let Some(progress) = super::query(root)?
        && progress.reset_epoch != epoch
        && progress.phase != HomeResetPhase::Verified
    {
        return Err(rejected("another Home reset epoch is in progress"));
    }
    enter(root, connection, epoch).await?;
    super::prepare(root, epoch)
}

pub(crate) async fn apply(
    root: &Path,
    connection: &Connection,
    epoch: u64,
) -> Result<(), ResourceControlError> {
    super::require_target_progress(root, epoch)?;
    enter(root, connection, epoch).await?;
    super::apply(root, epoch)
}

pub(crate) async fn recover(
    root: &Path,
    connection: &Connection,
    epoch: u64,
) -> Result<(), ResourceControlError> {
    super::require_epoch(epoch)?;
    match super::query(root)? {
        Some(progress) if progress.reset_epoch != epoch => {
            return Err(rejected("another Home reset epoch is in progress"));
        }
        None if !super::generation_layout_exists(root, epoch) => {
            return Err(rejected("Home reset has not been prepared"));
        }
        Some(_) | None => {}
    }
    enter(root, connection, epoch).await?;
    super::recover(root, epoch)
}

pub(crate) async fn verify(
    root: &Path,
    connection: &Connection,
    epoch: u64,
) -> Result<HomeResetProgress, ResourceControlError> {
    let progress = super::verify(root, epoch)?;
    if progress.phase == HomeResetPhase::Verified {
        finish(root, connection, &progress).await?;
    } else {
        set_ready(root, false)?;
    }
    Ok(progress)
}

async fn finish(
    root: &Path,
    connection: &Connection,
    progress: &HomeResetProgress,
) -> Result<(), ResourceControlError> {
    let Some(window) = Window::read(root)? else {
        return Ok(());
    };
    if progress.reset_epoch != window.epoch || progress.phase != HomeResetPhase::Verified {
        return Err(rejected("Home maintenance epoch has not been verified"));
    }
    set_ready(root, true)?;
    if window.restart_display {
        display_job(connection, true).await?;
    }
    fs::remove_file(super::state_directory(root).join(WINDOW))
        .map_err(|_| unavailable("Home maintenance window cannot be completed"))?;
    super::sync_directory(&super::state_directory(root))
}

/// Restores mount evidence before Type=dbus readiness releases boot-time login jobs.
pub(crate) async fn restore_home(
    root: &Path,
    connection: &Connection,
) -> Result<(), ResourceControlError> {
    let result = async {
        let window = Window::read(root)?;
        let progress = super::query(root)?;
        let epoch = window
            .as_ref()
            .map(|window| window.epoch)
            .or_else(|| progress.as_ref().map(|progress| progress.reset_epoch));
        if let Some(epoch) = epoch {
            if !super::mounted_generation(root, epoch)?
                || progress.as_ref().is_none_or(|progress| {
                    progress.reset_epoch != epoch || progress.phase != HomeResetPhase::Verified
                })
            {
                enter(root, connection, epoch).await?;
                if progress
                    .as_ref()
                    .is_none_or(|progress| progress.reset_epoch != epoch)
                    || progress
                        .as_ref()
                        .is_some_and(|progress| progress.phase == HomeResetPhase::Prepared)
                {
                    super::prepare(root, epoch)?;
                }
                super::recover(root, epoch)?;
            }
            let verified = super::verify(root, epoch)?;
            if verified.phase != HomeResetPhase::Verified {
                return Err(unavailable("Home mount recovery did not verify"));
            }
        }
        set_ready(root, true)
    }
    .await;
    if result.is_err() {
        set_ready(root, false)?;
    }
    result
}

/// Start jobs may wait for our bus name, so restore login only after acquiring it.
pub(crate) async fn restore_login(
    root: &Path,
    connection: &Connection,
) -> Result<(), ResourceControlError> {
    if let Some(window) = Window::read(root)? {
        let progress = super::verify(root, window.epoch)?;
        finish(root, connection, &progress).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        error::Error,
        path::PathBuf,
        sync::{
            Arc, Mutex, MutexGuard,
            atomic::{AtomicBool, Ordering},
        },
    };

    use tokio::{net::UnixStream, sync::Notify};
    use zbus::object_server::SignalEmitter;

    use super::*;
    use crate::home;

    type TestResult = Result<(), Box<dyn Error>>;
    type SessionRow = (String, u32, String, String, OwnedObjectPath);
    const UNIT: &str = "/org/freedesktop/systemd1/unit/display";
    const SESSION: &str = "/org/freedesktop/login1/session/contest";

    struct State {
        active: &'static str,
        session: bool,
        session_after_stop: bool,
        interlock: bool,
        job_result: Option<&'static str>,
        calls: Vec<&'static str>,
        stop_release: Option<Arc<Notify>>,
    }

    #[derive(Clone)]
    struct Host {
        root: PathBuf,
        state: Arc<Mutex<State>>,
        subscribed: Arc<AtomicBool>,
        stop_started: Arc<Notify>,
    }

    impl Host {
        fn state(&self) -> MutexGuard<'_, State> {
            self.state
                .lock()
                .unwrap_or_else(|error| panic!("fixture lock poisoned: {error}"))
        }

        async fn job(
            &self,
            start: bool,
            emitter: &SignalEmitter<'_>,
        ) -> zbus::fdo::Result<OwnedObjectPath> {
            let (result, release) = {
                let mut state = self.state();
                state.calls.push(if start { "start" } else { "stop" });
                (state.job_result, state.stop_release.clone())
            };
            assert!(Window::read(&self.root).is_ok_and(|window| window.is_some()));
            assert_eq!(
                self.root.join(READY.trim_start_matches('/')).exists(),
                start
            );
            if !start {
                self.stop_started.notify_one();
                if let Some(release) = release {
                    release.notified().await;
                }
            }
            let path = OwnedObjectPath::try_from("/org/freedesktop/systemd1/job/1")
                .map_err(zbus::Error::from)?;
            if let Some(result) = result {
                if result == "done" {
                    let mut state = self.state();
                    state.active = if start { "active" } else { "inactive" };
                    if !start {
                        state.session = state.session_after_stop;
                    }
                }
                // Deliberately emit before returning the job path, as a fast systemd job can.
                Self::job_removed(emitter, 1, &path, DISPLAY_MANAGER, result).await?;
            }
            Ok(path)
        }
    }

    #[derive(Debug, zbus::DBusError)]
    #[zbus(prefix = "org.freedesktop.systemd1")]
    enum SubscriptionError {
        AlreadySubscribed(String),
    }

    #[zbus::interface(name = "org.freedesktop.systemd1.Manager")]
    impl Host {
        #[allow(clippy::unused_self)] // Fixed D-Bus method in the host fixture.
        fn load_unit(&self, name: &str) -> zbus::fdo::Result<OwnedObjectPath> {
            assert_eq!(name, DISPLAY_MANAGER);
            Ok(OwnedObjectPath::try_from(UNIT).map_err(zbus::Error::from)?)
        }

        fn subscribe(&self) -> Result<(), SubscriptionError> {
            if self.subscribed.swap(true, Ordering::SeqCst) {
                return Err(SubscriptionError::AlreadySubscribed(
                    "already subscribed".to_owned(),
                ));
            }
            Ok(())
        }

        async fn stop_unit(
            &self,
            name: &str,
            mode: &str,
            #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
        ) -> zbus::fdo::Result<OwnedObjectPath> {
            assert_eq!((name, mode), (DISPLAY_MANAGER, "replace"));
            self.job(false, &emitter).await
        }

        async fn start_unit(
            &self,
            name: &str,
            mode: &str,
            #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
        ) -> zbus::fdo::Result<OwnedObjectPath> {
            assert_eq!((name, mode), (DISPLAY_MANAGER, "replace"));
            self.job(true, &emitter).await
        }

        #[zbus(signal)]
        async fn job_removed(
            emitter: &SignalEmitter<'_>,
            id: u32,
            job: &OwnedObjectPath,
            unit: &str,
            result: &str,
        ) -> zbus::Result<()>;
    }

    struct Unit(Host);

    #[zbus::interface(name = "org.freedesktop.systemd1.Unit")]
    #[allow(clippy::unused_self)] // Fixed D-Bus properties in the host fixture.
    impl Unit {
        #[zbus(property)]
        fn active_state(&self) -> &str {
            self.0.state().active
        }

        #[zbus(property)]
        fn conditions(&self) -> Vec<(String, bool, bool, String, i32)> {
            if self.0.state().interlock {
                vec![(
                    "ConditionPathExists".to_owned(),
                    false,
                    false,
                    READY.to_owned(),
                    0,
                )]
            } else {
                vec![]
            }
        }

        #[zbus(property)]
        fn after(&self) -> Vec<&str> {
            vec![HELPER]
        }

        #[zbus(property)]
        fn wants(&self) -> Vec<&str> {
            vec![HELPER]
        }
    }

    struct Login(Host);

    #[zbus::interface(name = "org.freedesktop.login1.Manager")]
    impl Login {
        fn list_sessions(&self) -> zbus::fdo::Result<Vec<SessionRow>> {
            Ok(if self.0.state().session {
                vec![(
                    "c1".to_owned(),
                    1000,
                    "contest".to_owned(),
                    "seat0".to_owned(),
                    OwnedObjectPath::try_from(SESSION).map_err(zbus::Error::from)?,
                )]
            } else {
                vec![]
            })
        }
    }

    struct Session;

    #[zbus::interface(name = "org.freedesktop.login1.Session")]
    #[allow(clippy::unused_self)] // Fixed D-Bus properties in the session fixture.
    impl Session {
        #[zbus(property)]
        fn active(&self) -> bool {
            true
        }
        #[zbus(property)]
        fn remote(&self) -> bool {
            false
        }
        #[zbus(property)]
        fn class(&self) -> &'static str {
            "user"
        }
        #[zbus(property, name = "Type")]
        fn kind(&self) -> &'static str {
            "x11"
        }
        #[zbus(property)]
        fn locked_hint(&self) -> bool {
            false
        }
    }

    struct Fixture {
        root: tempfile::TempDir,
        host: Host,
        client: Connection,
        _server: Connection,
    }

    impl Fixture {
        async fn new() -> Result<Self, Box<dyn Error>> {
            let root = tempfile::tempdir()?;
            for path in [
                home::TEMPLATE_RELATIVE_PATH,
                home::CONTEST_HOME_RELATIVE_PATH,
                home::STATE_RELATIVE_PATH,
                "run/natsume-privileged",
                "proc/sys/kernel/random",
            ] {
                fs::create_dir_all(root.path().join(path))?;
            }
            fs::write(
                root.path().join("proc/sys/kernel/random/boot_id"),
                "01900000-0000-7000-8000-000000000001",
            )?;
            set_ready(root.path(), true)?;
            let host = Host {
                root: root.path().to_owned(),
                state: Arc::new(Mutex::new(State {
                    active: "active",
                    session: false,
                    session_after_stop: false,
                    interlock: true,
                    job_result: Some("done"),
                    calls: vec![],
                    stop_release: None,
                })),
                subscribed: Arc::new(AtomicBool::new(false)),
                stop_started: Arc::new(Notify::new()),
            };
            let (server, client) = UnixStream::pair()?;
            let server = zbus::connection::Builder::unix_stream(server)
                .server(zbus::Guid::generate())?
                .p2p()
                .serve_at(SYSTEMD_PATH, host.clone())?
                .serve_at(UNIT, Unit(host.clone()))?
                .serve_at("/org/freedesktop/login1", Login(host.clone()))?
                .serve_at(SESSION, Session)?;
            let client = zbus::connection::Builder::unix_stream(client).p2p();
            let (server, client) = tokio::try_join!(server.build(), client.build())?;
            Ok(Self {
                root,
                host,
                client,
                _server: server,
            })
        }

        fn ready(&self) -> bool {
            self.root
                .path()
                .join(READY.trim_start_matches('/'))
                .exists()
        }

        fn simulated_verification(&self) -> Result<HomeResetProgress, ResourceControlError> {
            // Exercise completion only at the mount-evidence boundary; fixture verify
            // still inspects real mountinfo and must never report a fabricated mount.
            home::apply(self.root.path(), 7)?;
            home::record_verification(self.root.path(), 7, true)
        }
    }

    #[tokio::test]
    async fn prepare_waits_for_stop_and_holds_permit_until_verified_completion() -> TestResult {
        let fixture = Fixture::new().await?;
        let root = fixture.root.path();
        let release = Arc::new(Notify::new());
        fixture.host.state().stop_release = Some(release.clone());
        let prepare = prepare(root, &fixture.client, 7);
        let observation = async {
            fixture.host.stop_started.notified().await;
            assert!(!fixture.ready());
            assert!(home::query(root)?.is_none());
            assert!(!home::generation_directory(root, 7).exists());
            release.notify_one();
            Ok::<_, ResourceControlError>(())
        };
        tokio::try_join!(prepare, observation)?;
        apply(root, &fixture.client, 7).await?;
        assert!(!fixture.ready());
        let verified = fixture.simulated_verification()?;
        finish(root, &fixture.client, &verified).await?;
        assert!(fixture.ready());
        assert!(Window::read(root)?.is_none());
        assert_eq!(fixture.host.state().calls, ["stop", "start"]);
        Ok(())
    }

    #[tokio::test]
    async fn existing_session_or_missing_interlock_rejects_without_side_effects() -> TestResult {
        for session in [true, false] {
            let fixture = Fixture::new().await?;
            fixture.host.state().session = session;
            fixture.host.state().interlock = session;
            assert!(matches!(
                prepare(fixture.root.path(), &fixture.client, 7).await,
                Err(ResourceControlError::Rejected(_))
            ));
            assert!(fixture.ready());
            assert!(!home::state_exists(fixture.root.path())?);
            assert!(fixture.host.state().calls.is_empty());
        }
        Ok(())
    }

    #[tokio::test]
    async fn session_surviving_stop_rejects_before_home_effects() -> TestResult {
        let fixture = Fixture::new().await?;
        fixture.host.state().session_after_stop = true;
        assert!(matches!(
            prepare(fixture.root.path(), &fixture.client, 7).await,
            Err(ResourceControlError::Rejected(_))
        ));
        assert!(home::query(fixture.root.path())?.is_none());
        assert!(!fixture.ready());
        assert!(Window::read(fixture.root.path())?.is_some());
        Ok(())
    }

    #[tokio::test]
    async fn failed_or_timed_out_stop_preserves_window_without_home_effects() -> TestResult {
        for result in [Some("failed"), None] {
            let fixture = Fixture::new().await?;
            fixture.host.state().job_result = result;
            assert!(matches!(
                prepare(fixture.root.path(), &fixture.client, 7).await,
                Err(ResourceControlError::Unavailable(_))
            ));
            assert!(home::query(fixture.root.path())?.is_none());
            assert!(!fixture.ready());
            assert!(
                Window::read(fixture.root.path())?
                    .is_some_and(|window| window.epoch == 7 && window.restart_display)
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn retry_keeps_epoch_and_restart_obligation_after_interrupted_stop() -> TestResult {
        let fixture = Fixture::new().await?;
        let root = fixture.root.path();
        fixture.host.state().job_result = Some("canceled");
        assert!(prepare(root, &fixture.client, 7).await.is_err());
        assert!(prepare(root, &fixture.client, 8).await.is_err());
        fixture.host.state().job_result = Some("done");
        prepare(root, &fixture.client, 7).await?;
        let verified = fixture.simulated_verification()?;
        fixture.host.state().job_result = Some("failed");
        assert!(finish(root, &fixture.client, &verified).await.is_err());
        assert!(Window::read(root)?.is_some_and(|window| window.restart_display));
        fixture.host.state().job_result = Some("done");
        finish(root, &fixture.client, &verified).await?;
        assert!(Window::read(root)?.is_none());
        assert_eq!(
            fixture.host.state().calls,
            ["stop", "stop", "start", "start"]
        );
        Ok(())
    }

    #[tokio::test]
    async fn stopped_display_is_not_started_and_wrong_completion_cannot_release() -> TestResult {
        let fixture = Fixture::new().await?;
        let root = fixture.root.path();
        fixture.host.state().active = "inactive";
        prepare(root, &fixture.client, 7).await?;
        for (epoch, phase) in [(8, HomeResetPhase::Verified), (7, HomeResetPhase::Applied)] {
            assert!(
                finish(
                    root,
                    &fixture.client,
                    &HomeResetProgress {
                        reset_epoch: epoch,
                        phase
                    }
                )
                .await
                .is_err()
            );
            assert!(!fixture.ready());
        }
        finish(root, &fixture.client, &fixture.simulated_verification()?).await?;
        assert!(fixture.ready());
        assert!(fixture.host.state().calls.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn apply_and_recover_require_the_window_even_with_existing_progress() -> TestResult {
        for recovering in [false, true] {
            let fixture = Fixture::new().await?;
            let root = fixture.root.path();
            home::prepare(root, 7)?;
            fixture.host.state().job_result = Some("failed");
            let result = if recovering {
                recover(root, &fixture.client, 7).await
            } else {
                apply(root, &fixture.client, 7).await
            };
            assert!(result.is_err());
            assert!(
                home::query(root)?
                    .is_some_and(|progress| progress.phase == HomeResetPhase::Prepared)
            );
            assert!(!fixture.ready());
        }
        Ok(())
    }

    #[tokio::test]
    async fn unmounted_verified_marker_never_releases_login() -> TestResult {
        let fixture = Fixture::new().await?;
        let root = fixture.root.path();
        prepare(root, &fixture.client, 7).await?;
        fixture.simulated_verification()?;
        assert_eq!(
            verify(root, &fixture.client, 7).await?.phase,
            HomeResetPhase::RecoveryRequired
        );
        assert!(!fixture.ready());
        assert!(Window::read(root)?.is_some());
        assert_eq!(fixture.host.state().calls, ["stop"]);
        Ok(())
    }

    #[tokio::test]
    async fn startup_recovers_each_durable_crash_point_but_requires_current_mount_evidence()
    -> TestResult {
        for phase in [
            None,
            Some(HomeResetPhase::Prepared),
            Some(HomeResetPhase::Applied),
            Some(HomeResetPhase::Verified),
        ] {
            let fixture = Fixture::new().await?;
            let root = fixture.root.path();
            Window {
                epoch: 7,
                restart_display: true,
            }
            .persist(root)?;
            if let Some(phase) = phase {
                home::prepare(root, 7)?;
                home::write_progress(root, 7, phase)?;
            }
            // A reboot clears /run and starts with the display manager inactive.
            set_ready(root, false)?;
            fixture.host.state().active = "inactive";
            assert!(restore_home(root, &fixture.client).await.is_err());
            assert!(
                home::query(root)?.is_some_and(|progress| progress.reset_epoch == 7
                    && progress.phase == HomeResetPhase::RecoveryRequired)
            );
            assert!(home::generation_layout_exists(root, 7));
            assert!(!fixture.ready());
            assert!(fixture.host.state().calls.is_empty());
            assert!(
                Window::read(root)?
                    .is_some_and(|window| window.epoch == 7 && window.restart_display)
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn fresh_start_allows_login_but_invalid_journal_closes_it() -> TestResult {
        let fixture = Fixture::new().await?;
        let root = fixture.root.path();
        set_ready(root, false)?;
        restore_home(root, &fixture.client).await?;
        assert!(fixture.ready());
        for encoded in ["2\n7\nrestart\n", "1\n0\nrestart\n", "1\n7\ninvalid\n"] {
            fs::write(home::state_directory(root).join(WINDOW), encoded)?;
            assert!(restore_home(root, &fixture.client).await.is_err());
            assert!(!fixture.ready());
        }
        assert!(fixture.host.state().calls.is_empty());
        Ok(())
    }
}
