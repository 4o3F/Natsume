//! One bounded login owner; GDM keeps ownership of workers and user desktops.

use super::{
    GDM, GreeterStatus, HELPER, LoginContext, PREPARE_TIMEOUT, mutation_when_available,
    role_observation,
};
use crate::{
    admission, gdm_registration, processes, runtime,
    session::{self, fresh_proxy, rejected, role_name, unavailable},
};
use natsume_local_control_api::{
    GraphicalSession, GraphicalSessionState, ResourceControlError, SessionRole,
};
use serde::{Deserialize, Serialize};
use std::{path::Path, process::Stdio};
use tokio::{
    io::AsyncWriteExt as _,
    process::Command,
    time::{Duration, Instant, sleep, timeout},
};
use zbus::Connection;

const RECORD: &str = "session-preparation.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Greeter,
    Registration,
    Authentication,
    Desktop,
    Ready,
    Failed,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    boot: String,
    role: SessionRole,
    pid: i32,
    start: u64,
    phase: Phase,
    session: Option<GraphicalSession>,
    greeter: Option<GreeterStatus>,
}

impl State {
    fn record(&mut self, phase: Phase) -> Result<(), ResourceControlError> {
        if self.phase != phase {
            tracing::info!(
                role = role_name(self.role),
                ?phase,
                "Graphical preparation phase changed"
            );
            self.phase = phase;
        }
        runtime::write(Path::new("/"), RECORD, self)
    }
}

pub(super) fn failed_session(
    root: &Path,
    role: SessionRole,
    current: Option<&GraphicalSession>,
) -> Result<bool, ResourceControlError> {
    let Some(current) = current else {
        return Ok(false);
    };
    let Some(state) = runtime::read::<State>(root, RECORD)? else {
        return Ok(false);
    };
    // A still-running desktop that failed preparation is not repeatedly pulled
    // to the front. Recovery of the UI or an explicit preparation can retry it.
    Ok(state.boot == current.boot_id
        && state.role == role
        && state.phase == Phase::Failed
        && state.session.as_ref() == Some(current))
}

async fn probe() -> Result<Option<GreeterStatus>, ResourceControlError> {
    let account = session::account(Path::new("/"), "gdm")?;
    let operation = Command::new("/usr/bin/setpriv")
        .args([
            "--reuid",
            &account.uid.to_string(),
            "--regid",
            &account.gid.to_string(),
            "--clear-groups",
            "--no-new-privs",
            HELPER,
            "greeter-status",
        ])
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .output();
    let Ok(output) = timeout(Duration::from_secs(2), operation).await else {
        return Ok(None);
    };
    let output = output.map_err(|_| unavailable("fixed greeter probe cannot start"))?;
    if !output.status.success() {
        return Ok(None);
    }
    let status: GreeterStatus = serde_json::from_slice(&output.stdout)
        .map_err(|_| rejected("fixed greeter probe returned invalid data"))?;
    if status.identity.uid != account.uid || status.identity.pid <= 1 || status.identity.start == 0
    {
        return Err(rejected("fixed greeter probe returned invalid identity"));
    }
    Ok(Some(status))
}

async fn registered_greeter(
    connection: &Connection,
    state: &mut State,
) -> Result<LoginContext, ResourceControlError> {
    gdm_registration::require_observer(Path::new("/"), connection).await?;
    if session::greeter(connection, Path::new("/"))
        .await?
        .is_none()
    {
        let factory = fresh_proxy(
            connection,
            GDM,
            "/org/gnome/DisplayManager/LocalDisplayFactory",
            "org.gnome.DisplayManager.LocalDisplayFactory",
        )
        .await?;
        // GDM owns this display. Its public interface cannot identify a native
        // Wayland display by session, so a missed acknowledgement never grants
        // permission to terminate or recycle an unproven greeter.
        let _display: zbus::zvariant::OwnedObjectPath =
            factory.call("CreateTransientDisplay", &()).await?;
    }
    loop {
        let Some((id, path)) = session::greeter(connection, Path::new("/")).await? else {
            sleep(Duration::from_millis(100)).await;
            continue;
        };
        if state
            .greeter
            .as_ref()
            .is_some_and(|g| g.identity.session != id)
        {
            return Err(unavailable(
                "captured greeter was replaced; resample before retry",
            ));
        }
        session::activate_greeter(connection, &id, &path).await?;
        state.record(Phase::Registration)?;
        if let Some(current) = probe().await? {
            if current.identity.session != id
                || state.greeter.as_ref().is_some_and(|old| *old != current)
            {
                return Err(unavailable("captured greeter identity changed"));
            }
            if state.greeter.is_none() {
                state.greeter = Some(current.clone());
                state.record(Phase::Registration)?;
            }
            if let Some(proof) = gdm_registration::proof(Path::new("/"), connection, &id).await? {
                if proof.greeter != current.identity {
                    return Err(rejected("GDM registration belongs to another greeter"));
                }
                state.record(Phase::Authentication)?;
                return Ok(LoginContext {
                    proof,
                    greeter: current,
                });
            }
        }
        sleep(Duration::from_millis(100)).await;
    }
}

async fn desktop(connection: &Connection, state: &mut State) -> Result<(), ResourceControlError> {
    state.record(Phase::Desktop)?;
    loop {
        let current = role_observation(
            session::observe(connection, Path::new("/")).await?,
            state.role,
        );
        if let Some(identity) = current.session {
            if state
                .session
                .as_ref()
                .is_some_and(|expected| *expected != identity)
            {
                return Err(rejected(
                    "graphical preparation cannot retarget a replacement session",
                ));
            }
            if state.session.is_none() {
                state.session = Some(identity.clone());
                state.record(Phase::Desktop)?;
            }
            if current.state == GraphicalSessionState::Running
                && !current.locked_hint
                && current.desktop_ready
            {
                // Native Wayland may defer application drawing while hidden.
                // Establish the foreground capability here; presentation is
                // validated again when the current target is applied.
                match session::activate(connection, Path::new("/"), state.role, &identity).await {
                    Ok(()) => {
                        state.record(Phase::Ready)?;
                        return Ok(());
                    }
                    // Display resume may temporarily withdraw a capability.
                    // Keep the captured identity and the existing outer deadline.
                    Err(ResourceControlError::Unavailable(_)) => {}
                    Err(error) => return Err(error),
                }
            }
        }
        if matches!(
            current.state,
            GraphicalSessionState::Ambiguous | GraphicalSessionState::Error
        ) || current.locked_hint
        {
            return Err(rejected(
                "graphical preparation observed an invalid or locked role",
            ));
        }
        sleep(Duration::from_millis(100)).await;
    }
}

async fn prepare(connection: &Connection, state: &mut State) -> Result<(), ResourceControlError> {
    let current = role_observation(
        session::observe(connection, Path::new("/")).await?,
        state.role,
    );
    if current.session.is_some() {
        if current.state != GraphicalSessionState::Running || current.locked_hint {
            return Err(rejected("existing graphical role cannot be prepared"));
        }
        return desktop(connection, state).await;
    }
    if current.state != GraphicalSessionState::None {
        return Err(rejected("graphical role is not absent"));
    }
    if state.role == SessionRole::Contest {
        admission::require_no_workers(Path::new("/"))?;
    }
    let context = registered_greeter(connection, state).await?;
    let account = session::account(Path::new("/"), "gdm")?;
    let input = serde_json::to_vec(&context)
        .map_err(|_| unavailable("captured GDM context cannot be encoded"))?;
    let mut child = Command::new("/usr/bin/setpriv")
        .args([
            "--reuid",
            &account.uid.to_string(),
            "--regid",
            &account.gid.to_string(),
            "--clear-groups",
            "--no-new-privs",
            HELPER,
            "gdm-login",
            role_name(state.role),
        ])
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| unavailable("fixed GDM client could not start"))?;
    let mut pipe = child
        .stdin
        .take()
        .ok_or_else(|| unavailable("fixed GDM client input is unavailable"))?;
    pipe.write_all(&input)
        .await
        .map_err(|_| unavailable("captured GDM context could not be sent"))?;
    drop(pipe);
    tokio::select! {
        result = desktop(connection, state) => {
            let _killed = child.kill().await;
            let _reaped = child.wait().await;
            result
        }
        result = child.wait() => {
            let status = result.map_err(|_| unavailable("fixed GDM client status is unavailable"))?;
            if status.success() {
                desktop(connection, state).await
            } else {
                Err(unavailable("GDM authentication failed before desktop initialization"))
            }
        }
    }
}

pub(super) async fn run(role: SessionRole) -> Result<(), ResourceControlError> {
    if !rustix::process::geteuid().is_root() {
        return Err(rejected("session preparation requires root"));
    }
    let begun = Instant::now();
    let _mutation = mutation_when_available(Path::new("/")).await?;
    if role == SessionRole::Waiting {
        crate::waiting::require_login_allowed(Path::new("/"))?;
    }
    if role == SessionRole::Contest {
        admission::require_open(Path::new("/"))?;
        crate::home::require_template(Path::new("/"))?;
    }
    let connection = Connection::system()
        .await
        .map_err(|_| unavailable("system bus is unavailable"))?;
    let boot = session::read_boot_id(Path::new("/"))?;
    let pid =
        i32::try_from(std::process::id()).map_err(|_| unavailable("preparation PID is invalid"))?;
    let start = processes::start_time(Path::new("/"), pid)?
        .ok_or_else(|| unavailable("preparation process is unavailable"))?;
    let mut state = State {
        boot,
        role,
        pid,
        start,
        phase: Phase::Greeter,
        session: None,
        greeter: None,
    };
    state.record(Phase::Greeter)?;
    // Keep the owner alive during bounded cleanup; dropping the work future
    // alone does not prove that GDM cancelled an already-opened session.
    let work = PREPARE_TIMEOUT
        .saturating_sub(Duration::from_secs(5))
        .saturating_sub(begun.elapsed());
    let result = match timeout(work, prepare(&connection, &mut state)).await {
        Ok(result) => result,
        Err(_) => Err(unavailable(
            "preparation_timeout: registration or desktop initialization did not complete",
        )),
    };
    if let Err(error) = &result {
        tracing::error!(%error, role = role_name(role), phase = ?state.phase,
            elapsed_ms = begun.elapsed().as_millis(), "Graphical preparation incomplete; resample before retry");
        state.record(Phase::Failed)?;
        if role == SessionRole::Contest {
            let cleanup = async {
                let observed = session::observe(&connection, Path::new("/")).await?;
                if observed.waiting.desktop_ready
                    && !observed.waiting.locked_hint
                    && let Some(waiting) = observed.waiting.session
                {
                    session::activate(&connection, Path::new("/"), SessionRole::Waiting, &waiting)
                        .await?;
                }
                Ok::<_, ResourceControlError>(())
            };
            if !matches!(
                timeout(PREPARE_TIMEOUT.saturating_sub(begun.elapsed()), cleanup).await,
                Ok(Ok(()))
            ) {
                tracing::warn!(
                    code = "waiting_restore_unconfirmed",
                    "Could not confirm waiting after preparation failure"
                );
            }
        }
    }
    result
}
