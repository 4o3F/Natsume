use natsume_local_control_api::GraphicalSession;
use zbus::{Connection, Proxy, message::Header, proxy::CacheProperties, zvariant::OwnedObjectPath};

const LOGIN1: &str = "org.freedesktop.login1";
const LOGIN_PATH: &str = "/org/freedesktop/login1";

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Caller {
    pub(super) sender: String,
    uid: u32,
    pid: u32,
}

async fn proxy<'a>(
    connection: &'a Connection,
    service: &'a str,
    path: &'a str,
    interface: &'a str,
) -> zbus::Result<Proxy<'a>> {
    zbus::proxy::Builder::new(connection)
        .destination(service)?
        .path(path)?
        .interface(interface)?
        .cache_properties(CacheProperties::No)
        .build()
        .await
}

fn waiting_uid() -> Option<u32> {
    let passwd = std::fs::read_to_string("/etc/passwd").ok()?;
    let rows: Vec<Vec<_>> = passwd
        .lines()
        .map(|line| line.split(':').collect())
        .collect();
    let mut waiting = rows.iter().filter(|row| row.first() == Some(&"waiting"));
    let row = waiting.next()?;
    if row.len() != 7 || waiting.next().is_some() {
        return None;
    }
    let uid = row[2].parse::<u32>().ok()?;
    (uid != 0
        && rows
            .iter()
            .filter(|row| row.get(2).and_then(|uid| uid.parse::<u32>().ok()) == Some(uid))
            .count()
            == 1)
        .then_some(uid)
}

fn no_session(error: &zbus::Error) -> bool {
    matches!(error, zbus::Error::MethodError(name, _, _) if name.as_str() == "org.freedesktop.login1.NoSessionForPID")
}

/// Only the explicit logind `NoSessionForPID` error permits GNOME's user-manager
/// mapping. Any other error, extra login or missing fact rejects the caller.
pub(super) async fn authenticate(
    connection: &Connection,
    header: &Header<'_>,
    claimed: &GraphicalSession,
) -> Option<Caller> {
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        authenticate_inner(connection, header, claimed),
    )
    .await
    .ok()?
    .ok()
}

async fn authenticate_inner(
    connection: &Connection,
    header: &Header<'_>,
    claimed: &GraphicalSession,
) -> zbus::Result<Caller> {
    let sender = header.sender().ok_or_else(invalid)?;
    let bus = zbus::fdo::DBusProxy::new(connection).await?;
    let uid = bus.get_connection_unix_user(sender.clone().into()).await?;
    let pid = bus
        .get_connection_unix_process_id(sender.clone().into())
        .await?;
    if Some(uid) != waiting_uid() || pid == 0 {
        return Err(invalid());
    }
    let manager = proxy(
        connection,
        LOGIN1,
        LOGIN_PATH,
        "org.freedesktop.login1.Manager",
    )
    .await?;
    let path = match manager
        .call::<_, _, OwnedObjectPath>("GetSessionByPID", &(pid,))
        .await
    {
        Ok(path) => path,
        Err(error) if no_session(&error) => {
            gnome_app_session(connection, &manager, uid, pid, claimed).await?
        }
        Err(error) => return Err(error),
    };
    let session = proxy(
        connection,
        LOGIN1,
        path.as_str(),
        "org.freedesktop.login1.Session",
    )
    .await?;
    let id: String = session.get_property("Id").await?;
    let (session_uid, _): (u32, OwnedObjectPath) = session.get_property("User").await?;
    let (seat, _): (String, OwnedObjectPath) = session.get_property("Seat").await?;
    let class: String = session.get_property("Class").await?;
    let kind: String = session.get_property("Type").await?;
    let remote: bool = session.get_property("Remote").await?;
    if id != claimed.logind_session_id
        || session_uid != uid
        || seat != "seat0"
        || class != "user"
        || kind != "x11"
        || remote
    {
        return Err(invalid());
    }
    Ok(Caller {
        sender: sender.as_str().to_owned(),
        uid,
        pid,
    })
}

fn invalid() -> zbus::Error {
    zbus::Error::Failure("waiting caller identity is unavailable or invalid".to_owned())
}

async fn gnome_app_session(
    connection: &Connection,
    manager: &Proxy<'_>,
    uid: u32,
    pid: u32,
    claimed: &GraphicalSession,
) -> zbus::Result<OwnedObjectPath> {
    let user_path: OwnedObjectPath = manager.call("GetUserByPID", &(pid,)).await?;
    let user = proxy(
        connection,
        LOGIN1,
        user_path.as_str(),
        "org.freedesktop.login1.User",
    )
    .await?;
    let actual_uid: u32 = user.get_property("UID").await?;
    let name: String = user.get_property("Name").await?;
    let (display, path): (String, OwnedObjectPath) = user.get_property("Display").await?;
    if actual_uid != uid || name != "waiting" || display != claimed.logind_session_id {
        return Err(invalid());
    }
    let systemd = proxy(
        connection,
        "org.freedesktop.systemd1",
        "/org/freedesktop/systemd1",
        "org.freedesktop.systemd1.Manager",
    )
    .await?;
    let unit_path: OwnedObjectPath = systemd.call("GetUnitByPID", &(pid,)).await?;
    let unit = proxy(
        connection,
        "org.freedesktop.systemd1",
        unit_path.as_str(),
        "org.freedesktop.systemd1.Unit",
    )
    .await?;
    let unit_id: String = unit.get_property("Id").await?;
    if unit_id != format!("user@{uid}.service") {
        return Err(invalid());
    }
    let sessions: Vec<(String, u32, String, String, OwnedObjectPath)> =
        manager.call("ListSessions", &()).await?;
    let mut matching = sessions
        .iter()
        .filter(|(_, actual_uid, user, _, _)| *actual_uid == uid || user == "waiting");
    let Some((id, actual_uid, name, seat, actual_path)) = matching.next() else {
        return Err(invalid());
    };
    if matching.next().is_some()
        || id != &display
        || *actual_uid != uid
        || name != "waiting"
        || seat != "seat0"
        || actual_path != &path
    {
        return Err(invalid());
    }
    Ok(path)
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    pub(in crate::reconcile::binding) fn caller(sender: &str) -> Caller {
        Caller {
            sender: sender.to_owned(),
            uid: 1002,
            pid: 42,
        }
    }

    #[test]
    fn only_explicit_no_session_can_enter_the_gnome_mapping()
    -> Result<(), Box<dyn std::error::Error>> {
        for (name, accepted) in [
            ("org.freedesktop.login1.NoSessionForPID", true),
            ("org.freedesktop.login1.NoSuchSession", false),
            ("org.freedesktop.DBus.Error.AccessDenied", false),
            ("org.freedesktop.DBus.Error.Timeout", false),
        ] {
            let error = zbus::Error::MethodError(
                name.try_into()?,
                None,
                zbus::Message::method_call("/", "Probe")?.build(&())?,
            );
            assert_eq!(no_session(&error), accepted);
        }
        Ok(())
    }
}
