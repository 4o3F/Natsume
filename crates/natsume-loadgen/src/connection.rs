use std::{future::pending, sync::Arc, time::Duration};

use futures_util::{SinkExt as _, StreamExt as _};
use natsume_workstation_protocol::{
    CONTROL_ROUTE, CONTROL_SUBPROTOCOL,
    generated::{
        ClientActiveEnvelope, ClientHandshakeEnvelope, EnrollmentReviewState, ServerActiveEnvelope,
        ServerHandshakeEnvelope, client_active_envelope, client_handshake_envelope,
        server_active_envelope, server_handshake_envelope,
    },
};
use prost::Message as _;
use rustls::{ClientConfig, RootCertStore};
use rustls_pki_types::{CertificateDer, ServerName, pem::PemObject as _};
use snafu::{OptionExt as _, ResultExt as _};
use tokio::{
    net::TcpStream,
    sync::watch,
    time::{Instant, MissedTickBehavior, timeout},
};
use tokio_rustls::{TlsConnector, client::TlsStream};
use tokio_tungstenite::{
    WebSocketStream, client_async_with_config,
    tungstenite::{Message, client::IntoClientRequest as _, protocol::WebSocketConfig},
};
use tokio_util::sync::CancellationToken;
use uuid::{Uuid, Variant, Version};

use crate::{
    cli::Run,
    emulate::{Emulator, millis},
    error::{self, Error, Result},
    identity::Samples,
    logo,
    metrics::Metrics,
};

type Socket = WebSocketStream<TlsStream<TcpStream>>;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const SEND_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_MESSAGE: usize = 65_536;

pub(crate) struct Settings {
    tls: Arc<ClientConfig>,
    host: String,
    port: u16,
    url: String,
}

impl Settings {
    pub(crate) fn new(run: &Run) -> Result<Self> {
        Self::from_origin(&run.server, &run.trust_root)
    }

    pub(crate) fn from_origin(origin: &str, trust_root: &std::path::Path) -> Result<Self> {
        let encoded = std::fs::read(trust_root).context(error::IoSnafu)?;
        let mut roots = RootCertStore::empty();
        let mut count = 0;
        for certificate in CertificateDer::pem_slice_iter(&encoded) {
            let certificate = certificate.context(error::PemSnafu)?;
            roots.add(certificate).context(error::TlsSnafu)?;
            count += 1;
        }
        if count != 1 {
            return Err(Error::Invalid { code: "TRUST_ROOT" });
        }
        let mut tls =
            ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_protocol_versions(&[&rustls::version::TLS13])
                .context(error::TlsSnafu)?
                .with_root_certificates(roots)
                .with_no_client_auth();
        tls.alpn_protocols = vec![b"http/1.1".to_vec()];
        let url = url::Url::parse(origin).context(error::UrlSnafu)?;
        let host = url
            .host_str()
            .context(error::InvalidSnafu {
                code: "SERVER_HOST",
            })?
            .trim_matches(['[', ']'])
            .to_owned();
        Ok(Self {
            tls: Arc::new(tls),
            host,
            port: url.port().unwrap_or(443),
            url: format!(
                "wss://{}{CONTROL_ROUTE}",
                origin.trim_start_matches("https://")
            ),
        })
    }

    fn for_device(&self) -> Self {
        let mut tls = (*self.tls).clone();
        tls.resumption = rustls::client::Resumption::in_memory_sessions(256);
        Self {
            tls: Arc::new(tls),
            host: self.host.clone(),
            port: self.port,
            url: self.url.clone(),
        }
    }

    pub(crate) fn http(&self) -> Result<reqwest::Client> {
        reqwest::Client::builder()
            .no_proxy()
            .https_only(true)
            .http1_only()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(20))
            .use_preconfigured_tls((*self.tls).clone())
            .build()
            .context(error::HttpSnafu)
    }

    async fn connect(&self, device: usize, metrics: &Metrics) -> Result<Socket> {
        let started = Instant::now();
        let tls_result = timeout(CONNECT_TIMEOUT, async {
            let stream = TcpStream::connect((self.host.as_str(), self.port))
                .await
                .context(error::TransportSnafu)?;
            stream.set_nodelay(true).context(error::TransportSnafu)?;
            let name = ServerName::try_from(self.host.clone()).context(error::ServerNameSnafu)?;
            TlsConnector::from(Arc::clone(&self.tls))
                .connect(name, stream)
                .await
                .context(error::TransportSnafu)
        })
        .await
        .context(error::TimeoutSnafu)
        .and_then(std::convert::identity);
        metrics.record(
            device,
            "tcp_tls",
            started.elapsed(),
            tls_result
                .as_ref()
                .map_or_else(super::error::Error::code, |_| "OK"),
        )?;
        let stream = tls_result?;
        let mut request = self
            .url
            .as_str()
            .into_client_request()
            .context(error::WebSocketSnafu)?;
        request.headers_mut().insert(
            "sec-websocket-protocol",
            CONTROL_SUBPROTOCOL.parse().context(error::HeaderSnafu)?,
        );
        let started = Instant::now();
        let upgrade = timeout(
            CONNECT_TIMEOUT,
            client_async_with_config(
                request,
                stream,
                Some(
                    WebSocketConfig::default()
                        .max_message_size(Some(MAX_MESSAGE))
                        .max_frame_size(Some(MAX_MESSAGE)),
                ),
            ),
        )
        .await
        .context(error::TimeoutSnafu)
        .and_then(|r| r.context(error::WebSocketSnafu));
        let code = match &upgrade {
            Ok((_, response)) => response.status().as_u16().to_string(),
            Err(Error::WebSocket {
                source: tokio_tungstenite::tungstenite::Error::Http(response),
            }) => response.status().as_u16().to_string(),
            Err(error) => error.code().to_owned(),
        };
        metrics.record(device, "wss_upgrade", started.elapsed(), code)?;
        let (socket, response) = upgrade?;
        if response
            .headers()
            .get("sec-websocket-protocol")
            .and_then(|s| s.to_str().ok())
            != Some(CONTROL_SUBPROTOCOL)
        {
            return Err(Error::Peer {
                code: "SUBPROTOCOL_MISMATCH",
            });
        }
        Ok(socket)
    }
}

pub(crate) async fn run(
    mut emulator: Emulator,
    index: usize,
    run: Arc<Run>,
    settings: Arc<Settings>,
    metrics: Metrics,
    stop: CancellationToken,
) -> Result<()> {
    let settings = settings.for_device();
    let (school, receiver) = watch::channel(None);
    // Each workstation owns its HTTP pool, matching independent daemon processes.
    let mut logo_task = if run.logo_interval.is_zero() {
        None
    } else {
        Some(tokio::spawn(logo::run(
            settings.http()?,
            Arc::clone(&run),
            index,
            metrics.clone(),
            receiver,
            stop.clone(),
        )))
    };
    let mut samples = Samples::new(run.seed, index, "reconnect");
    let mut backoff = Backoff::new(&run);
    let mut first = true;
    let result = loop {
        let delay = Duration::from_millis(samples.uniform(backoff.window_ms));
        metrics.record(
            index,
            "reconnect_delay",
            delay,
            if first { "INITIAL" } else { "RETRY" },
        )?;
        tokio::select! { () = stop.cancelled() => break Ok(()), () = tokio::time::sleep(delay) => {} }
        if !first {
            backoff.advance(&run);
        }
        first = false;
        let mut stable = false;
        let attempt = async {
            let mut socket = settings.connect(index, &metrics).await?;
            let started = Instant::now();
            let purpose = if emulator.identity.state.device_id.is_some() {
                "RESUME"
            } else {
                "ENROLLMENT"
            };
            let ready = handshake(&mut socket, &mut emulator, &run).await;
            metrics.record(
                index,
                "handshake",
                started.elapsed(),
                ready
                    .as_ref()
                    .map_or_else(super::error::Error::code, |_| purpose),
            )?;
            let session = ready?;
            let _connected = metrics.connected();
            active(
                socket,
                &mut emulator,
                &run,
                Lease {
                    session: &session,
                    index,
                    metrics: &metrics,
                },
                &school,
                &mut stable,
            )
            .await
        };
        let result = tokio::select! {
            biased;
            () = stop.cancelled() => break Ok(()),
            result = async { match logo_task.as_mut() { Some(task) => task.await, None => pending().await } } => {
                logo_task = None;
                break result.context(error::JoinSnafu)?.and(Err(Error::Invalid { code: "LOGO_WORKER_ENDED" }));
            }
            result = attempt => result,
        };
        school.send_replace(None);
        emulator.deactivate()?;
        if let Err(error) = result {
            metrics.record(index, "disconnect", Duration::ZERO, error.code())?;
            if error.fatal() {
                break Err(error);
            }
        }
        if stable {
            backoff = Backoff::new(&run);
        }
    };
    drop(school);
    if let Some(task) = logo_task {
        task.await.context(error::JoinSnafu)??;
    }
    result
}

// Same scheduling as crates/natsume-workstation-daemon/src/control/connection.rs:
// first jitter 0..5s; retries 0..5/10/20/30s; only valid Active traffic after 60s resets it.
pub(crate) struct Backoff {
    pub(crate) window_ms: u64,
}
impl Backoff {
    pub(crate) fn new(run: &Run) -> Self {
        Self {
            window_ms: millis(run.reconnect_initial),
        }
    }
    pub(crate) fn advance(&mut self, run: &Run) {
        self.window_ms = self
            .window_ms
            .saturating_mul(2)
            .min(millis(run.reconnect_max));
    }
}

async fn send(socket: &mut Socket, message: Message) -> Result<()> {
    timeout(SEND_TIMEOUT, socket.send(message))
        .await
        .context(error::TimeoutSnafu)?
        .context(error::WebSocketSnafu)
}

async fn receive(socket: &mut Socket) -> Result<Message> {
    socket
        .next()
        .await
        .context(error::PeerSnafu { code: "EOF" })?
        .context(error::WebSocketSnafu)
}

async fn send_proof(socket: &mut Socket, emulator: &Emulator) -> Result<()> {
    let message = timeout(CONNECT_TIMEOUT, receive(socket))
        .await
        .context(error::TimeoutSnafu)??;
    let Message::Binary(bytes) = message else {
        return Err(Error::Peer {
            code: "CHALLENGE_FRAME",
        });
    };
    let envelope = ServerHandshakeEnvelope::decode(bytes).context(error::DecodeSnafu)?;
    let Some(server_handshake_envelope::Body::ServerChallenge(challenge)) = envelope.body else {
        return Err(Error::Peer {
            code: "CHALLENGE_MISSING",
        });
    };
    if challenge.challenge_nonce.len() != 32 {
        return Err(Error::Peer {
            code: "CHALLENGE_LENGTH",
        });
    }
    let ip = socket
        .get_ref()
        .get_ref()
        .0
        .local_addr()
        .context(error::IoSnafu)?
        .ip()
        .to_canonical()
        .to_string();
    let proof = emulator.identity.proof(&challenge, ip);
    send(
        socket,
        Message::Binary(
            ClientHandshakeEnvelope {
                body: Some(client_handshake_envelope::Body::ClientProof(proof)),
            }
            .encode_to_vec()
            .into(),
        ),
    )
    .await?;
    Ok(())
}

async fn handshake(socket: &mut Socket, emulator: &mut Emulator, run: &Run) -> Result<Vec<u8>> {
    send_proof(socket, emulator).await?;
    let mut enrolling = emulator.identity.state.device_id.is_none();
    let mut pending_review = false;
    let mut deadline = Instant::now() + CONNECT_TIMEOUT;
    let mut ping = tokio::time::interval_at(Instant::now() + run.ping_interval, run.ping_interval);
    ping.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        let message = tokio::select! {
            () = tokio::time::sleep_until(deadline) => return Err(Error::Peer { code: "HANDSHAKE_SILENCE" }),
            _ = ping.tick(), if pending_review => { send(socket, Message::Ping(Vec::new().into())).await?; continue; }
            message = receive(socket) => message?,
        };
        if let Message::Pong(payload) = &message
            && pending_review
            && payload.is_empty()
        {
            deadline = Instant::now() + run.silence_timeout;
            continue;
        }
        let Message::Binary(bytes) = message else {
            return Err(Error::Peer {
                code: "HANDSHAKE_FRAME",
            });
        };
        let envelope = ServerHandshakeEnvelope::decode(bytes).context(error::DecodeSnafu)?;
        match envelope.body {
            Some(server_handshake_envelope::Body::EnrollmentReviewStatus(status)) if enrolling => {
                if status.state != i32::from(EnrollmentReviewState::PendingReview)
                    || !status.error_code.is_empty()
                    || pending_review
                {
                    return Err(Error::Peer {
                        code: "ENROLLMENT_REJECTED",
                    });
                }
                pending_review = true;
                deadline = Instant::now() + run.silence_timeout;
            }
            Some(server_handshake_envelope::Body::EnrollmentActivated(authority)) if enrolling => {
                if !uuid_v7(&authority.device_id) {
                    return Err(Error::Peer {
                        code: "AUTHORITY_INVALID",
                    });
                }
                emulator.identity.state.device_id = Some(authority.device_id.clone());
                emulator.identity.save()?;
                send(
                    socket,
                    Message::Binary(
                        ClientHandshakeEnvelope {
                            body: Some(client_handshake_envelope::Body::EnrollmentReady(authority)),
                        }
                        .encode_to_vec()
                        .into(),
                    ),
                )
                .await?;
                enrolling = false;
                pending_review = false;
                deadline = Instant::now() + CONNECT_TIMEOUT;
            }
            Some(server_handshake_envelope::Body::SessionReady(ready)) if !enrolling => {
                if !Uuid::from_slice(&ready.session_id).is_ok_and(|u| {
                    u.get_version() == Some(Version::SortRand)
                        && u.get_variant() == Variant::RFC4122
                }) {
                    return Err(Error::Peer {
                        code: "SESSION_INVALID",
                    });
                }
                return Ok(ready.session_id);
            }
            _ => {
                return Err(Error::Peer {
                    code: "HANDSHAKE_ORDER",
                });
            }
        }
    }
}

pub(crate) fn uuid_v7(value: &str) -> bool {
    Uuid::parse_str(value).is_ok_and(|u| {
        u.get_version() == Some(Version::SortRand)
            && u.get_variant() == Variant::RFC4122
            && u.to_string() == value
    })
}

struct Lease<'a> {
    session: &'a [u8],
    index: usize,
    metrics: &'a Metrics,
}
impl Lease<'_> {
    async fn snapshot(
        &self,
        socket: &mut Socket,
        snapshot: &natsume_workstation_protocol::generated::ClientStateSnapshot,
    ) -> Result<()> {
        let envelope = ClientActiveEnvelope {
            session_id: self.session.to_vec(),
            body: Some(client_active_envelope::Body::ClientState(snapshot.clone())),
        };
        let bytes = envelope.encode_to_vec();
        if bytes.len() > MAX_MESSAGE {
            return Err(Error::Invalid {
                code: "SNAPSHOT_TOO_LARGE",
            });
        }
        send(socket, Message::Binary(bytes.into())).await
    }
}

async fn active(
    mut socket: Socket,
    emulator: &mut Emulator,
    run: &Run,
    lease: Lease<'_>,
    school: &watch::Sender<Option<String>>,
    stable: &mut bool,
) -> Result<()> {
    let session = lease.session;
    let index = lease.index;
    let metrics = lease.metrics;
    let started = Instant::now();
    lease.snapshot(&mut socket, &emulator.snapshot).await?;
    metrics.record(index, "first_snapshot", started.elapsed(), "SENT")?;
    let mut last_sent = emulator.snapshot.clone();
    let mut first_target = true;
    let mut deadline = Instant::now() + run.silence_timeout;
    let mut ping = tokio::time::interval_at(Instant::now() + run.ping_interval, run.ping_interval);
    ping.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut ping_sent = None::<Instant>;
    loop {
        tokio::select! {
            biased;
            () = tokio::time::sleep_until(deadline) => return Err(Error::Peer { code: "SERVER_SILENCE" }),
            message = receive(&mut socket) => {
                match message? {
                    Message::Pong(payload) if payload.as_ref() == session => {
                        if let Some(sent) = ping_sent.take() { metrics.record(index, "ping_rtt", sent.elapsed(), "OK")?; }
                    }
                    Message::Binary(bytes) => {
                        let envelope = ServerActiveEnvelope::decode(bytes).context(error::DecodeSnafu)?;
                        if envelope.session_id.as_slice() != session { return Err(Error::Peer { code: "SESSION_FENCED" }); }
                        let Some(server_active_envelope::Body::ServerState(target)) = envelope.body else { return Err(Error::Peer { code: "SERVER_STATE_MISSING" }); };
                        emulator.accept(target.clone(), run)?;
                        if let Some(device) = emulator.identity.state.device_id.as_deref() { metrics.convergence.target(device, index, &target, metrics)?; }
                        if first_target { metrics.record(index, "first_target", started.elapsed(), "RECEIVED")?; first_target = false; }
                        let next = emulator.organization().map(str::to_owned);
                        school.send_if_modified(|current| { if *current == next { false } else { *current = next; true } });
                    }
                    Message::Close(_) => return Err(Error::Peer { code: "CLOSE" }),
                    _ => return Err(Error::Peer { code: "ACTIVE_FRAME" }),
                }
                deadline = Instant::now() + run.silence_timeout;
                *stable |= started.elapsed() >= run.stable_after;
            }
            _ = ping.tick() => {
                send(&mut socket, Message::Ping(session.to_vec().into())).await?;
                ping_sent = Some(Instant::now());
            }
            () = async { match emulator.deadline() { Some(due) => tokio::time::sleep_until(due).await, None => pending().await } } => {
                emulator.apply().await?;
                let completed = emulator.changes(&last_sent);
                if emulator.snapshot != last_sent {
                    lease.snapshot(&mut socket, &emulator.snapshot).await?;
                    if let Some(device) = emulator.identity.state.device_id.as_deref() { metrics.convergence.actual(device, &emulator.snapshot, metrics)?; }
                    let sent = Instant::now();
                    last_sent.clone_from(&emulator.snapshot);
                    for (resource, received) in completed {
                        lease.metrics.record(lease.index, "target_actual", sent.duration_since(received), resource.name())?;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn corrupt_logo_cache_stops_device_with_the_encoding_source() -> Result<()> {
        let directory = tempfile::tempdir().context(error::IoSnafu)?;
        let mut run = crate::cli::tests::run();
        run.state_dir = directory.path().to_owned();
        run.trust_root = directory.path().join("root.crt");
        run.reconnect_initial = Duration::from_millis(1);
        let key = zeroize::Zeroizing::new(
            rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)
                .context(error::CertificateSnafu)?,
        );
        let certificate = rcgen::CertificateParams::default()
            .self_signed(&*key)
            .context(error::CertificateSnafu)?;
        std::fs::write(&run.trust_root, certificate.der()).context(error::IoSnafu)?;
        std::fs::write(directory.path().join("logos-00000.json"), b"invalid JSON")
            .context(error::IoSnafu)?;
        let identity = crate::identity::Identity::load(directory.path(), 1, 0, "001".to_owned())?;
        let emulator = Emulator::new(identity, 1, 0)?;
        let mut roots = RootCertStore::empty();
        roots
            .add(certificate.der().clone())
            .context(error::TlsSnafu)?;
        let tls =
            ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_protocol_versions(&[&rustls::version::TLS13])
                .context(error::TlsSnafu)?
                .with_root_certificates(roots)
                .with_no_client_auth();
        let settings = Arc::new(Settings {
            tls: Arc::new(tls),
            host: "127.0.0.1".to_owned(),
            port: 0,
            url: "wss://127.0.0.1:0".to_owned(),
        });
        let (metrics, writer) = Metrics::start(&directory.path().join("events"))?;
        let result = timeout(
            Duration::from_secs(5),
            super::run(
                emulator,
                0,
                Arc::new(run),
                settings,
                metrics.clone(),
                CancellationToken::new(),
            ),
        )
        .await
        .context(error::TimeoutSnafu)?;
        assert!(matches!(result, Err(Error::Json { .. })));
        drop(metrics);
        writer.await.context(error::JoinSnafu)??;
        Ok(())
    }

    #[test]
    fn retry_windows_and_seeded_full_jitter_match_daemon_scheduling() {
        let run = crate::cli::tests::run();
        let mut backoff = Backoff::new(&run);
        let mut first = Samples::new(42, 7, "reconnect");
        let mut replay = Samples::new(42, 7, "reconnect");
        // Initial wait does not advance; advance happens after each retry wait.
        for (retry, expected) in [5_000, 5_000, 10_000, 20_000, 30_000, 30_000]
            .into_iter()
            .enumerate()
        {
            assert_eq!(backoff.window_ms, expected);
            for _ in 0..100 {
                let sample = first.uniform(backoff.window_ms);
                assert!(sample <= expected);
                assert_eq!(sample, replay.uniform(backoff.window_ms));
            }
            if retry > 0 {
                backoff.advance(&run);
            }
        }
    }
}
