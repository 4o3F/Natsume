//! GDM's acknowledgement is later than `SessionManager` Running and `sd_notify`.
//! Keep this observer independent of the graphical mutation lock and RPC server.

use std::path::{Path, PathBuf};

use futures_util::StreamExt as _;
use natsume_local_control_api::ResourceControlError;
use serde::{Deserialize, Serialize};
use tokio::time::{Duration, Instant, sleep, timeout};
use zbus::{
    Connection, MatchRule, Message, MessageStream, message::Type, zvariant::OwnedObjectPath,
};

use crate::{
    processes, runtime,
    session::{self, fresh_proxy, unavailable},
};

const RECORD: &str = "gdm-registration.json";
const GDM: &str = "org.gnome.DisplayManager";
const MANAGER: &str = "org.gnome.DisplayManager.Manager";
const MANAGER_PATH: &str = "/org/gnome/DisplayManager/Manager";
const CAPACITY: usize = 16;
const CALL_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Context {
    pub(crate) boot: String,
    bus: String,
    owner: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Greeter {
    pub(crate) session: String,
    pub(crate) uid: u32,
    pub(crate) pid: i32,
    pub(crate) start: u64,
}

/// Passed through the root-owned pipe to the fixed gdm UID child.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Proof {
    pub(crate) context: Context,
    pub(crate) greeter: Greeter,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cache {
    version: u32,
    context: Context,
    observer_pid: i32,
    observer_start: u64,
    connected: bool,
    subscription: String,
    receipts: Vec<Greeter>,
}

struct Pending {
    sender: String,
    serial: u32,
    proof: Proof,
    requested: Instant,
}

async fn context(root: &Path, connection: &Connection) -> Result<Context, ResourceControlError> {
    let dbus = zbus::fdo::DBusProxy::new(connection).await?;
    let owner = match dbus
        .get_name_owner(GDM.try_into().map_err(zbus::Error::from)?)
        .await
    {
        Ok(owner) => Some(owner.to_string()),
        Err(zbus::fdo::Error::NameHasNoOwner(_)) => None,
        Err(error) => return Err(zbus::Error::from(error).into()),
    };
    Ok(Context {
        boot: session::read_boot_id(root)?,
        bus: dbus.get_id().await.map_err(zbus::Error::from)?.to_string(),
        owner,
    })
}

pub(crate) async fn current_context(
    root: &Path,
    connection: &Connection,
) -> Result<Context, ResourceControlError> {
    timeout(CALL_TIMEOUT, context(root, connection))
        .await
        .map_err(|_| unavailable("GDM registration context timed out"))?
}

impl Cache {
    fn matches(&self, context: &Context) -> bool {
        self.version == 1 && self.receipts.len() <= CAPACITY && self.context == *context
    }

    fn refresh(&mut self, context: Context) {
        if self.context != context {
            self.receipts.clear();
            self.context = context;
        }
    }

    fn retain_live(&mut self, root: &Path) {
        self.receipts.retain(|g| {
            g.pid > 1
                && g.start > 0
                && processes::start_time(root, g.pid).ok().flatten() == Some(g.start)
        });
    }
}

pub(crate) async fn require_observer(
    root: &Path,
    connection: &Connection,
) -> Result<String, ResourceControlError> {
    let now = current_context(root, connection).await?;
    let cache = runtime::read::<Cache>(root, RECORD)?
        .ok_or_else(|| unavailable("GDM registration observer has not started"))?;
    if !cache.matches(&now)
        || !cache.connected
        || processes::start_time(root, cache.observer_pid)? != Some(cache.observer_start)
    {
        return Err(unavailable("GDM registration observer is unavailable"));
    }
    Ok(cache.subscription)
}

pub(crate) async fn proof(
    root: &Path,
    connection: &Connection,
    session: &str,
) -> Result<Option<Proof>, ResourceControlError> {
    let now = current_context(root, connection).await?;
    let Some(cache) = runtime::read::<Cache>(root, RECORD)? else {
        return Ok(None);
    };
    if !cache.matches(&now) || now.owner.is_none() {
        return Ok(None);
    }
    let uid = session::account(root, "gdm")?.uid;
    let matches: Vec<_> = cache
        .receipts
        .into_iter()
        .filter(|g| {
            g.session == session
                && g.uid == uid
                && g.pid > 1
                && g.start > 0
                && processes::start_time(root, g.pid).ok().flatten() == Some(g.start)
        })
        .collect();
    match matches.as_slice() {
        [greeter] => Ok(Some(Proof {
            context: now,
            greeter: greeter.clone(),
        })),
        _ => Ok(None),
    }
}

pub(crate) async fn context_matches(
    root: &Path,
    connection: &Connection,
    proof: &Proof,
) -> Result<bool, ResourceControlError> {
    Ok(proof.context.owner.is_some() && current_context(root, connection).await? == proof.context)
}

async fn identify(
    root: &Path,
    connection: &Connection,
    sender: &str,
) -> Result<Option<Greeter>, ResourceControlError> {
    let expected = session::account(root, "gdm")?;
    let dbus = zbus::fdo::DBusProxy::new(connection).await?;
    let name: zbus::names::BusName<'_> = sender.try_into().map_err(zbus::Error::from)?;
    if dbus
        .get_connection_unix_user(name.clone())
        .await
        .map_err(zbus::Error::from)?
        != expected.uid
    {
        return Ok(None);
    }
    let pid = i32::try_from(
        dbus.get_connection_unix_process_id(name)
            .await
            .map_err(zbus::Error::from)?,
    )
    .map_err(|_| unavailable("GDM registration process is invalid"))?;
    let Some(start) = processes::start_time(root, pid)? else {
        return Ok(None);
    };
    let logind = fresh_proxy(
        connection,
        "org.freedesktop.login1",
        "/org/freedesktop/login1",
        "org.freedesktop.login1.Manager",
    )
    .await?;
    let pid_argument =
        u32::try_from(pid).map_err(|_| unavailable("GDM registration process is invalid"))?;
    let path: OwnedObjectPath = logind.call("GetSessionByPID", &(pid_argument,)).await?;
    let proxy = fresh_proxy(
        connection,
        "org.freedesktop.login1",
        path.as_str(),
        "org.freedesktop.login1.Session",
    )
    .await?;
    let id: String = proxy.get_property("Id").await?;
    let (seat, _): (String, OwnedObjectPath) = proxy.get_property("Seat").await?;
    if seat != "seat0"
        || proxy.get_property::<String>("Class").await? != "greeter"
        || proxy.get_property::<String>("Type").await? != "wayland"
        || proxy.get_property::<bool>("Remote").await?
        || proxy.get_property::<String>("State").await? == "closing"
        || processes::start_time(root, pid)? != Some(start)
    {
        return Ok(None);
    }
    // Executable and private bus identity are checked under gdm's own UID by
    // the consuming fixed client. This root task needs no CAP_SYS_PTRACE.
    Ok(Some(Greeter {
        session: id,
        uid: expected.uid,
        pid,
        start,
    }))
}

fn accept_reply(pending: &Pending, message: &Message, context: &Context) -> bool {
    let header = message.header();
    message.message_type() == Type::MethodReturn
        && header.primary().body_len() == 0
        && pending.proof.context == *context
        && header.sender().map(zbus::names::UniqueName::as_str) == context.owner.as_deref()
        && header.destination().map(zbus::names::BusName::as_str) == Some(pending.sender.as_str())
        && header.reply_serial().map(std::num::NonZeroU32::get) == Some(pending.serial)
}

async fn subscribe(
    root: &Path,
) -> Result<(Connection, MessageStream, Cache), ResourceControlError> {
    let lookup = zbus::connection::Builder::system()?
        .method_timeout(CALL_TIMEOUT)
        .build()
        .await?;
    let monitor = zbus::connection::Builder::system()?
        .max_queued(256)
        .build()
        .await?;
    let rules: Vec<MatchRule<'_>> = [
        "type='method_call',interface='org.gnome.DisplayManager.Manager',member='RegisterSession',path='/org/gnome/DisplayManager/Manager'",
        "type='method_return',sender='org.gnome.DisplayManager'",
        "type='error',sender='org.gnome.DisplayManager'",
        "type='signal',sender='org.freedesktop.DBus',interface='org.freedesktop.DBus',member='NameOwnerChanged',arg0='org.gnome.DisplayManager'",
    ].into_iter().map(MatchRule::try_from).collect::<zbus::Result<_>>()?;
    let stream = MessageStream::from(&monitor);
    zbus::fdo::MonitoringProxy::new(&monitor)
        .await?
        .become_monitor(&rules, 0)
        .await
        .map_err(zbus::Error::from)?;
    let now = current_context(root, &lookup).await?;
    let pid = std::process::id()
        .try_into()
        .map_err(|_| unavailable("registration observer PID is invalid"))?;
    let start = processes::start_time(root, pid)?
        .ok_or_else(|| unavailable("registration observer process is unavailable"))?;
    let receipts = match runtime::read::<Cache>(root, RECORD) {
        Ok(Some(cache)) if cache.matches(&now) => cache.receipts,
        Ok(_) => Vec::new(),
        Err(error) => {
            tracing::warn!(%error, code = "gdm_registration_cache_invalid", "Discarding unverified startup observations");
            Vec::new()
        }
    };
    let mut cache = Cache {
        version: 1,
        context: now,
        observer_pid: pid,
        observer_start: start,
        connected: true,
        subscription: uuid::Uuid::now_v7().to_string(),
        receipts,
    };
    cache.retain_live(root);
    runtime::write(root, RECORD, &cache)?;
    tracing::info!(
        code = "gdm_registration_observer_ready",
        "GDM registration observation available"
    );
    Ok((lookup, stream, cache))
}

async fn observe_once(root: &Path, outage: &mut bool) -> Result<(), ResourceControlError> {
    let (lookup, mut stream, mut cache) = subscribe(root).await?;
    *outage = false;
    let mut pending: Vec<Pending> = Vec::new();
    while let Some(message) = stream.next().await {
        let message = message?;
        let header = message.header();
        if message.message_type() == Type::Signal {
            cache.refresh(current_context(root, &lookup).await?);
            pending.retain(|p| p.proof.context == cache.context);
            cache.retain_live(root);
            runtime::write(root, RECORD, &cache)?;
            continue;
        }
        pending.retain(|p| p.requested.elapsed() < Duration::from_secs(40));
        if message.message_type() == Type::MethodCall {
            if header.interface().map(zbus::names::InterfaceName::as_str) != Some(MANAGER)
                || header.member().map(zbus::names::MemberName::as_str) != Some("RegisterSession")
                || header.path().map(zbus::zvariant::ObjectPath::as_str) != Some(MANAGER_PATH)
            {
                continue;
            }
            cache.refresh(current_context(root, &lookup).await?);
            if cache.context.owner.is_none()
                || !header.destination().is_some_and(|d| {
                    d.as_str() == GDM || Some(d.as_str()) == cache.context.owner.as_deref()
                })
            {
                continue;
            }
            let Some(sender) = header.sender() else {
                continue;
            };
            let Ok(Ok(Some(greeter))) =
                timeout(CALL_TIMEOUT, identify(root, &lookup, sender.as_str())).await
            else {
                continue;
            };
            if pending.len() == CAPACITY {
                pending.remove(0);
            }
            pending.push(Pending {
                sender: sender.to_string(),
                serial: header.primary().serial_num().get(),
                proof: Proof {
                    context: cache.context.clone(),
                    greeter,
                },
                requested: Instant::now(),
            });
        } else if let Some(index) = pending.iter().position(|p| {
            header.destination().is_some_and(|d| d.as_str() == p.sender)
                && header.reply_serial().map(std::num::NonZeroU32::get) == Some(p.serial)
        }) {
            let request = pending.remove(index);
            cache.refresh(current_context(root, &lookup).await?);
            if accept_reply(&request, &message, &cache.context) {
                cache.retain_live(root);
                let g = request.proof.greeter;
                cache.receipts.retain(|r| r.session != g.session);
                if cache.receipts.len() == CAPACITY {
                    cache.receipts.remove(0);
                }
                tracing::info!(session = %g.session, pid = g.pid, code = "greeter_registered", "GDM acknowledged greeter startup");
                cache.receipts.push(g);
                runtime::write(root, RECORD, &cache)?;
            }
        }
    }
    Err(unavailable("GDM registration monitor disconnected"))
}

pub(crate) async fn observe(root: PathBuf) {
    let mut outage = false;
    loop {
        if let Err(error) = observe_once(&root, &mut outage).await
            && !outage
        {
            outage = true;
            tracing::warn!(%error, code = "gdm_registration_observer_unavailable", "GDM registration observation will reconnect");
        }
        if let Ok(Some(mut cache)) = runtime::read::<Cache>(&root, RECORD) {
            cache.connected = false;
            if let Err(error) = runtime::write(&root, RECORD, &cache) {
                tracing::warn!(%error, "Could not withdraw GDM observation availability");
            }
        }
        sleep(Duration::from_secs(2)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> Context {
        Context {
            boot: "boot".into(),
            bus: "bus".into(),
            owner: Some(":1.20".into()),
        }
    }

    #[test]
    fn only_matching_successful_acknowledgement_completes_startup()
    -> Result<(), Box<dyn std::error::Error>> {
        let call = Message::method_call(MANAGER_PATH, "RegisterSession")?
            .sender(":1.30")?
            .destination(GDM)?
            .build(&())?;
        let request = Pending {
            sender: ":1.30".into(),
            serial: call.primary_header().serial_num().get(),
            proof: Proof {
                context: context(),
                greeter: Greeter {
                    session: "c1".into(),
                    uid: 107,
                    pid: 100,
                    start: 22,
                },
            },
            requested: Instant::now(),
        };
        let reply = Message::method_return(&call.header())?
            .sender(":1.20")?
            .build(&())?;
        assert!(accept_reply(&request, &reply, &context()));
        let wrong = Message::method_return(&call.header())?
            .sender(":1.21")?
            .build(&())?;
        assert!(!accept_reply(&request, &wrong, &context()));
        let nonempty = Message::method_return(&call.header())?
            .sender(":1.20")?
            .build(&true)?;
        assert!(!accept_reply(&request, &nonempty, &context()));
        let error = Message::error(&call.header(), "org.freedesktop.DBus.Error.Failed")?
            .sender(":1.20")?
            .build(&"failed")?;
        assert!(!accept_reply(&request, &error, &context()));
        let mut replaced = context();
        replaced.bus = "new bus".into();
        assert!(!accept_reply(&request, &reply, &replaced));
        Ok(())
    }

    #[test]
    fn gdm_replacement_invalidates_cached_completions() {
        let mut cache = Cache {
            version: 1,
            context: context(),
            observer_pid: 100,
            observer_start: 1,
            connected: true,
            subscription: "stream".into(),
            receipts: vec![Greeter {
                session: "c1".into(),
                uid: 107,
                pid: 200,
                start: 2,
            }],
        };
        cache.refresh(context());
        assert_eq!(cache.receipts.len(), 1);
        let mut replacement = context();
        replacement.owner = Some(":1.40".into());
        cache.refresh(replacement);
        assert!(cache.receipts.is_empty());
    }
}
