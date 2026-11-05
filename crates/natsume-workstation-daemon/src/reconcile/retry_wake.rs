use futures_util::StreamExt as _;
use natsume_workstation_ipc::{SessionRole, preparation_unit};
use snafu::{ResultExt as _, Snafu};
use tokio::{
    sync::watch,
    task::JoinHandle,
    time::{Duration, timeout},
};
use zbus::{
    Connection, Message, OwnedObjectPath, Proxy,
    proxy::{OwnerChangedStream, SignalStream},
};

const INITIALIZATION_TIMEOUT: Duration = Duration::from_secs(2);
const RESUBSCRIBE_DELAY: Duration = Duration::from_secs(1);
const MAX_RESUBSCRIBE_DELAY: Duration = Duration::from_secs(60);

#[derive(Debug, Snafu)]
enum WakeError {
    #[snafu(display("retry wake bus operation failed: {source}"))]
    Bus { source: zbus::Error },
    #[snafu(display("retry wake subscription timed out: {source}"))]
    Timeout { source: tokio::time::error::Elapsed },
    #[snafu(display("retry wake signal stream ended"))]
    StreamEnded,
}

#[derive(Clone, Copy)]
enum Source {
    Logind,
    Preparation,
}

impl Source {
    fn name(self) -> &'static str {
        match self {
            Self::Logind => "logind",
            Self::Preparation => "preparation",
        }
    }

    async fn streams(
        self,
        connection: Connection,
    ) -> Result<(SignalStream<'static>, OwnerChangedStream<'static>), WakeError> {
        let (service, path, interface, member) = match self {
            Self::Logind => (
                "org.freedesktop.login1",
                "/org/freedesktop/login1",
                "org.freedesktop.login1.Manager",
                "SessionRemoved",
            ),
            Self::Preparation => (
                "org.freedesktop.systemd1",
                "/org/freedesktop/systemd1",
                "org.freedesktop.systemd1.Manager",
                "JobRemoved",
            ),
        };
        let proxy = Proxy::new_owned(connection, service, path, interface)
            .await
            .context(BusSnafu)?;
        let owners = proxy.receive_owner_changed().await.context(BusSnafu)?;
        let signals = proxy.receive_signal(member).await.context(BusSnafu)?;
        if matches!(self, Self::Preparation) {
            proxy
                .call::<_, _, ()>("Subscribe", &())
                .await
                .context(BusSnafu)?;
        }
        Ok((signals, owners))
    }

    fn relevant(self, message: &Message) -> Result<bool, WakeError> {
        match self {
            Self::Logind => {
                let _: (String, OwnedObjectPath) =
                    message.body().deserialize().context(BusSnafu)?;
                Ok(true)
            }
            Self::Preparation => {
                let (_, _, unit, _): (u32, OwnedObjectPath, String, String) =
                    message.body().deserialize().context(BusSnafu)?;
                Ok([SessionRole::Waiting, SessionRole::Contest]
                    .into_iter()
                    .any(|role| unit == preparation_unit(role)))
            }
        }
    }

    async fn observe(self, connection: Connection, changes: watch::Sender<()>) {
        let mut retry_delay = RESUBSCRIBE_DELAY;
        loop {
            let result = async {
                let (mut signals, mut owners) =
                    timeout(INITIALIZATION_TIMEOUT, self.streams(connection.clone()))
                        .await
                        .context(TimeoutSnafu)??;
                retry_delay = RESUBSCRIBE_DELAY;
                loop {
                    tokio::select! {
                        message = signals.next() => {
                            let message = message.ok_or(WakeError::StreamEnded)?;
                            if self.relevant(&message)? {
                                changes.send_replace(());
                            }
                        }
                        _ = owners.next() => return Ok::<_, WakeError>(()),
                    }
                }
            }
            .await;
            let delay = retry_delay;
            if let Err(error) = result {
                tracing::warn!(
                    source = self.name(),
                    error = %error,
                    "Retry wake subscription unavailable"
                );
                retry_delay = (retry_delay * 2).min(MAX_RESUBSCRIBE_DELAY);
            }
            tokio::time::sleep(delay).await;
        }
    }
}

pub(super) struct RetryWake {
    connection: Connection,
    changes: watch::Sender<()>,
}

impl RetryWake {
    pub(super) fn new(connection: Connection) -> Self {
        Self {
            connection,
            changes: watch::channel(()).0,
        }
    }

    #[cfg(test)]
    pub(super) fn notify(&self) {
        self.changes.send_replace(());
    }

    pub(super) fn subscribe(&self) -> watch::Receiver<()> {
        self.changes.subscribe()
    }

    pub(super) fn start(&self) -> JoinHandle<()> {
        let connection = self.connection.clone();
        let changes = self.changes.clone();
        tokio::spawn(async move {
            tokio::join!(
                Source::Logind.observe(connection.clone(), changes.clone()),
                Source::Preparation.observe(connection, changes)
            );
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    use super::*;

    const SYSTEMD_PATH: &str = "/org/freedesktop/systemd1";
    const SYSTEMD_INTERFACE: &str = "org.freedesktop.systemd1.Manager";

    struct Manager {
        calls: Arc<AtomicUsize>,
        reject: AtomicBool,
    }

    #[zbus::interface(name = "org.freedesktop.systemd1.Manager")]
    impl Manager {
        async fn subscribe(
            &self,
            #[zbus(connection)] connection: &Connection,
        ) -> zbus::fdo::Result<()> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.reject.swap(false, Ordering::SeqCst) {
                return Err(zbus::fdo::Error::Failed("fixture unavailable".to_owned()));
            }
            job(connection, preparation_unit(SessionRole::Waiting), "done").await;
            Ok(())
        }
    }

    async fn bus() -> (
        tempfile::TempDir,
        tokio::process::Child,
        Connection,
        Connection,
    ) {
        let directory =
            tempfile::tempdir().unwrap_or_else(|error| panic!("private bus directory: {error}"));
        let (child, address) = super::super::tests::private_bus(&directory)
            .await
            .unwrap_or_else(|e| panic!("private bus: {e}"));
        let server = zbus::connection::Builder::address(address.as_str())
            .build()
            .await
            .unwrap_or_else(|error| panic!("server connection: {error}"));
        let client = zbus::connection::Builder::address(address.as_str())
            .build()
            .await
            .unwrap_or_else(|error| panic!("client connection: {error}"));
        (directory, child, server, client)
    }

    async fn job(connection: &Connection, unit: &str, result: &str) {
        connection
            .emit_signal(
                None::<&str>,
                SYSTEMD_PATH,
                SYSTEMD_INTERFACE,
                "JobRemoved",
                &(
                    1_u32,
                    OwnedObjectPath::try_from("/org/freedesktop/systemd1/job/1")
                        .unwrap_or_else(|error| panic!("job path: {error}")),
                    unit,
                    result,
                ),
            )
            .await
            .unwrap_or_else(|error| panic!("job signal: {error}"));
    }

    async fn session_removed(connection: &Connection) {
        connection
            .emit_signal(
                None::<&str>,
                "/org/freedesktop/login1",
                "org.freedesktop.login1.Manager",
                "SessionRemoved",
                &(
                    "c1",
                    OwnedObjectPath::try_from("/org/freedesktop/login1/session/c1")
                        .unwrap_or_else(|error| panic!("session path: {error}")),
                ),
            )
            .await
            .unwrap_or_else(|error| panic!("session signal: {error}"));
    }

    async fn next(stream: &mut SignalStream<'_>) -> Message {
        timeout(Duration::from_secs(2), stream.next())
            .await
            .unwrap_or_else(|error| panic!("signal deadline: {error}"))
            .unwrap_or_else(|| panic!("signal stream"))
    }

    #[tokio::test]
    async fn preparation_subscription_installs_stream_first_and_filters_units_and_senders() {
        let (_directory, _child, server, client) = bus().await;
        server
            .request_name("org.freedesktop.systemd1")
            .await
            .unwrap_or_else(|error| panic!("systemd owner: {error}"));
        let calls = Arc::new(AtomicUsize::new(0));
        server
            .object_server()
            .at(
                SYSTEMD_PATH,
                Manager {
                    calls: Arc::clone(&calls),
                    reject: AtomicBool::new(false),
                },
            )
            .await
            .unwrap_or_else(|error| panic!("manager interface: {error}"));
        let (mut signals, _owners) = Source::Preparation
            .streams(client.clone())
            .await
            .unwrap_or_else(|error| panic!("subscription: {error}"));
        assert!(
            Source::Preparation
                .relevant(&next(&mut signals).await)
                .unwrap_or_else(|error| panic!("initial signal: {error}"))
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        job(&client, preparation_unit(SessionRole::Contest), "done").await;
        job(&server, "unrelated.service", "done").await;
        let unrelated = next(&mut signals).await;
        assert_eq!(
            unrelated
                .header()
                .sender()
                .map(zbus::names::UniqueName::as_str),
            server.unique_name().map(|name| name.as_str())
        );
        assert!(
            !Source::Preparation
                .relevant(&unrelated)
                .unwrap_or_else(|error| panic!("unrelated signal: {error}"))
        );
        for result in ["done", "failed", "timeout", "canceled"] {
            job(&server, preparation_unit(SessionRole::Contest), result).await;
            assert!(
                Source::Preparation
                    .relevant(&next(&mut signals).await)
                    .unwrap_or_else(|error| panic!("settled signal: {error}"))
            );
        }
        let malformed = Message::signal(SYSTEMD_PATH, SYSTEMD_INTERFACE, "JobRemoved")
            .build(&true)
            .unwrap_or_else(|error| panic!("malformed body: {error}"));
        assert!(Source::Preparation.relevant(&malformed).is_err());
    }

    #[tokio::test]
    async fn logind_subscription_verifies_sender_and_tracks_new_owner() {
        let (_directory, _child, server, client) = bus().await;
        server
            .request_name("org.freedesktop.login1")
            .await
            .unwrap_or_else(|error| panic!("logind owner: {error}"));
        let (mut signals, _owners) = Source::Logind
            .streams(client.clone())
            .await
            .unwrap_or_else(|error| panic!("subscription: {error}"));
        session_removed(&client).await;
        session_removed(&server).await;
        let accepted = next(&mut signals).await;
        assert_eq!(
            accepted
                .header()
                .sender()
                .map(zbus::names::UniqueName::as_str),
            server.unique_name().map(|name| name.as_str())
        );
        assert!(
            Source::Logind
                .relevant(&accepted)
                .unwrap_or_else(|error| panic!("removed session: {error}"))
        );
        server
            .release_name("org.freedesktop.login1")
            .await
            .unwrap_or_else(|error| panic!("release owner: {error}"));
        client
            .request_name("org.freedesktop.login1")
            .await
            .unwrap_or_else(|error| panic!("new owner: {error}"));
        session_removed(&server).await;
        session_removed(&client).await;
        assert_eq!(
            next(&mut signals)
                .await
                .header()
                .sender()
                .map(zbus::names::UniqueName::as_str),
            client.unique_name().map(|name| name.as_str())
        );
    }

    #[tokio::test]
    async fn source_failure_preserves_other_source_and_resubscribes_after_owner_change() {
        let (_directory, _child, server, client) = bus().await;
        server
            .request_name("org.freedesktop.systemd1")
            .await
            .unwrap_or_else(|error| panic!("systemd owner: {error}"));
        server
            .request_name("org.freedesktop.login1")
            .await
            .unwrap_or_else(|error| panic!("logind owner: {error}"));
        let calls = Arc::new(AtomicUsize::new(0));
        server
            .object_server()
            .at(
                SYSTEMD_PATH,
                Manager {
                    calls: Arc::clone(&calls),
                    reject: AtomicBool::new(true),
                },
            )
            .await
            .unwrap_or_else(|error| panic!("manager interface: {error}"));
        let wake = RetryWake::new(client.clone());
        let mut changes = wake.subscribe();
        let _task = tokio_util::task::AbortOnDropHandle::new(wake.start());
        timeout(Duration::from_millis(500), async {
            loop {
                session_removed(&server).await;
                tokio::select! {
                    Ok(()) = changes.changed() => break,
                    () = tokio::time::sleep(Duration::from_millis(5)) => {},
                }
            }
        })
        .await
        .unwrap_or_else(|error| panic!("logind remains available: {error}"));
        timeout(Duration::from_secs(3), async {
            while calls.load(Ordering::SeqCst) < 2 {
                changes
                    .changed()
                    .await
                    .unwrap_or_else(|error| panic!("watch sender: {error}"));
            }
        })
        .await
        .unwrap_or_else(|error| panic!("preparation resubscribes after failure: {error}"));
        server
            .release_name("org.freedesktop.systemd1")
            .await
            .unwrap_or_else(|error| panic!("release systemd: {error}"));
        client
            .object_server()
            .at(
                SYSTEMD_PATH,
                Manager {
                    calls: Arc::clone(&calls),
                    reject: AtomicBool::new(false),
                },
            )
            .await
            .unwrap_or_else(|error| panic!("replacement manager: {error}"));
        client
            .request_name("org.freedesktop.systemd1")
            .await
            .unwrap_or_else(|error| panic!("replacement owner: {error}"));
        timeout(Duration::from_secs(3), async {
            while calls.load(Ordering::SeqCst) < 3 {
                changes
                    .changed()
                    .await
                    .unwrap_or_else(|error| panic!("watch sender: {error}"));
            }
        })
        .await
        .unwrap_or_else(|error| panic!("Subscribe called on replacement owner: {error}"));
    }
}
