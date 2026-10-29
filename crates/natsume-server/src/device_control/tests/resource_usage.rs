//! Opt-in loopback TLS/WSS load probe with synthetic authorities and a temporary DB.
use crate::journal::JournalContext;
use std::{
    net::{Ipv4Addr, SocketAddr},
    sync::Mutex,
    time::Instant,
};

use axum::serve::Listener as _;
use sha2::{Digest as _, Sha256};
use tokio::{sync::watch, task::JoinSet};
use tokio_tungstenite::{Connector, client_async_tls_with_config};

use super::*;
use crate::{
    component::target_submission::{TargetAction, TargetScope, TargetSubmissionRequest},
    tls::{PeerAddress, TlsListener, tests::TestIdentity},
};

type ProbeResult<T = ()> = Result<T, crate::test_error::TestError>;

async fn activate_device(
    fixture: &Fixture,
    index: usize,
    key: &SigningKey,
) -> ProbeResult<(DeviceId, String)> {
    let hardware = Uuid::new_v5(&Uuid::NAMESPACE_OID, &index.to_le_bytes()).to_string();
    let evidence = ValidatedEnrollmentEvidence::new(
        MachineHardwareId::parse(&hardware)
            .ok_or(crate::test_error::TestError::missing("hardware ID"))?,
        ControlPublicKey::parse(&key.verifying_key().to_bytes())
            .ok_or(crate::test_error::TestError::missing("control key"))?,
        EvidenceQuality::Strong,
        "2.10.3".to_owned(),
        "2.10.3".to_owned(),
    );
    let EnrollmentStartOutcome::Pending(review, activation) = fixture
        .state
        .device()
        .start_enrollment(
            evidence,
            std::net::IpAddr::from([127, 0, 0, 1]),
            JournalContext::default(),
        )
        .await
        .map_err(crate::test_error::TestError::from)?
    else {
        return Err(crate::test_error::TestError::missing(
            "unexpected authority replay",
        ));
    };
    let approval = fixture
        .state
        .device()
        .approve_enrollment(
            review.review_id(),
            crate::component::device::EnrollmentApprovalMode::Manual,
            JournalContext::default(),
        )
        .await
        .map_err(crate::test_error::TestError::from)?;
    let authority = approval.authority();
    approval.complete();
    assert!(activation.await?.is_ok());
    Ok((authority.device_id(), hardware))
}

async fn resume_socket(
    address: SocketAddr,
    config: Arc<rustls::ClientConfig>,
    key: &SigningKey,
    hardware: String,
) -> ProbeResult<(TestSocket, Vec<u8>)> {
    let mut request = format!("wss://{address}{CONTROL_ROUTE}").into_client_request()?;
    request
        .headers_mut()
        .insert("sec-websocket-protocol", CONTROL_SUBPROTOCOL.parse()?);
    let (mut socket, response) = client_async_tls_with_config(
        request,
        TcpStream::connect(address).await?,
        None,
        Some(Connector::Rustls(config)),
    )
    .await?;
    assert_eq!(response.status(), 101);
    let Some(server_handshake_envelope::Body::ServerChallenge(challenge)) =
        receive_handshake(&mut socket).await.body
    else {
        return Err(crate::test_error::TestError::missing("missing challenge"));
    };
    let proof = sign_client_proof(
        key,
        &challenge,
        ClientProof {
            client_ip: None,
            daemon_version: "2.10.3".to_owned(),
            agent_version: "2.10.3".to_owned(),
            machine_hardware_id: hardware,
            signature: Vec::new(),
            purpose: Some(client_proof::Purpose::Resume(
                natsume_workstation_protocol::generated::ResumeSession::default(),
            )),
        },
    );
    send_handshake(
        &mut socket,
        ClientHandshakeEnvelope {
            body: Some(client_handshake_envelope::Body::ClientProof(proof)),
        },
    )
    .await;
    let Some(server_handshake_envelope::Body::SessionReady(ready)) =
        receive_handshake(&mut socket).await.body
    else {
        return Err(crate::test_error::TestError::missing(
            "resume was not activated",
        ));
    };
    socket
        .send(ClientMessage::Binary(
            ClientActiveEnvelope {
                session_id: ready.session_id.clone(),
                body: Some(client_active_envelope::Body::ClientState(valid_snapshot())),
            }
            .encode_to_vec()
            .into(),
        ))
        .await?;
    assert_complete_target(ServerActiveEnvelope::decode(
        receive_binary(&mut socket).await.as_slice(),
    )?);
    Ok((socket, ready.session_id))
}

async fn keep_alive(
    mut socket: TestSocket,
    session: Vec<u8>,
    mut stop: watch::Receiver<bool>,
    samples: Arc<Mutex<Vec<Duration>>>,
) -> ProbeResult {
    let mut interval = tokio::time::interval(Duration::from_secs(2));
    let mut outstanding: Option<Instant> = None;
    loop {
        tokio::select! {
            _ = stop.changed() => { socket.close(None).await?; return Ok(()); }
            _ = interval.tick() => {
                if let Some(started) = outstanding {
                    if started.elapsed() > Duration::from_secs(5) { return Err(crate::test_error::TestError::missing("heartbeat exceeded existing five-second test bound")); }
                } else {
                    let started = Instant::now();
                    socket.send(ClientMessage::Ping(session.clone().into())).await?;
                    outstanding = Some(started);
                }
            }
            message = socket.next() => match message {
                Some(Ok(ClientMessage::Pong(payload))) if payload.as_ref() == session => {
                    if let Some(started) = outstanding.take() {
                        samples.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(started.elapsed());
                    }
                }
                Some(Ok(ClientMessage::Binary(_))) => {},
                Some(Ok(ClientMessage::Ping(payload))) => socket.send(ClientMessage::Pong(payload)).await?,
                _ => return Err(crate::test_error::TestError::missing("live control connection")),
            }
        }
    }
}

fn report(label: &str, samples: &mut [Duration]) {
    samples.sort_unstable();
    if !samples.is_empty() {
        tracing::info!(
            "{label}: samples={}, p50={:?}, p95={:?}, max={:?}",
            samples.len(),
            samples[(samples.len() - 1) / 2],
            samples[(samples.len() * 95).div_ceil(100) - 1],
            samples[samples.len() - 1]
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "600 loopback TLS/WSS clients; NATSUME_RESOURCE_WORKBOOK selects synthetic XLSX; not production acceptance"]
async fn measure_import_and_target_burst_with_six_hundred_tls_control_connections() -> ProbeResult {
    timeout(Duration::from_mins(2), run_mixed_probe()).await?
}

async fn run_mixed_probe() -> ProbeResult {
    let fixture = Fixture::new().await;
    let identity =
        TestIdentity::new(Ipv4Addr::LOCALHOST).map_err(crate::test_error::TestError::from)?;
    let listener = TlsListener::bind(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        identity.certificate_path(),
        identity.private_key_path(),
    )
    .await?;
    let address = listener.local_addr()?;
    let application = crate::http::router(
        Arc::clone(&fixture.state),
        std::path::Path::new("/nonexistent-resource-probe-web"),
    );
    let (shutdown, receiver) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            application.into_make_service_with_connect_info::<PeerAddress>(),
        )
        .with_graceful_shutdown(async {
            let _ = receiver.await;
        })
        .await
    });
    let mut roots = rustls::RootCertStore::empty();
    roots.add(identity.ca_certificate().clone())?;
    let mut config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_protocol_versions(&[&rustls::version::TLS13])?
    .with_root_certificates(roots)
    .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    let config = Arc::new(config);
    let (stop, stopped) = watch::channel(false);
    let heartbeat_samples = Arc::new(Mutex::new(Vec::new()));
    let mut clients = JoinSet::new();
    let mut ids = Vec::new();
    for index in 0_usize..600 {
        let key = SigningKey::from_bytes(&Sha256::digest(index.to_le_bytes()).into());
        let (id, hardware) = activate_device(&fixture, index, &key).await?;
        let (socket, session) = resume_socket(address, Arc::clone(&config), &key, hardware).await?;
        ids.push(id);
        clients.spawn(keep_alive(
            socket,
            session,
            stopped.clone(),
            Arc::clone(&heartbeat_samples),
        ));
    }
    assert_eq!(
        fixture
            .state
            .device_control()
            .active_device_ids()
            .await
            .len(),
        600
    );
    tracing::info!("600 TLS 1.3 / authenticated WSS connections active");
    exercise_mixed_load(&fixture, &ids, &heartbeat_samples).await?;
    assert_eq!(
        fixture
            .state
            .device_control()
            .active_device_ids()
            .await
            .len(),
        600
    );
    assert!(
        clients.try_join_next().is_none(),
        "control client failed during load"
    );
    stop.send(true)?;
    while let Some(result) = clients.join_next().await {
        result??;
    }
    report(
        "WSS ping/pong round trip",
        &mut heartbeat_samples
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    );
    for line in fs::read_to_string("/proc/self/status")?
        .lines()
        .filter(|line| line.starts_with("VmRSS:") || line.starts_with("VmHWM:"))
    {
        tracing::info!("{line}");
    }
    let _ = shutdown.send(());
    timeout(Duration::from_secs(10), server).await???;
    Ok(())
}

async fn observe_status(
    fixture: &Fixture,
    device: DeviceId,
    count: usize,
    spacing: Duration,
) -> ProbeResult<Vec<Duration>> {
    let mut samples = Vec::new();
    for _ in 0..count {
        let started = Instant::now();
        assert!(
            fixture
                .state
                .device_control()
                .read_device_status(device)
                .await
                .map_err(crate::test_error::TestError::from)?
                .is_some()
        );
        samples.push(started.elapsed());
        if !spacing.is_zero() {
            tokio::time::sleep(spacing).await;
        }
    }
    Ok(samples)
}

async fn exercise_mixed_load(
    fixture: &Fixture,
    ids: &[DeviceId],
    heartbeat_samples: &Mutex<Vec<Duration>>,
) -> ProbeResult {
    let mut baseline = observe_status(fixture, ids[0], 10, Duration::ZERO).await?;
    report("single-device status before mixed load", &mut baseline);
    heartbeat_samples
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();
    let bytes = fs::read(std::env::var("NATSUME_RESOURCE_WORKBOOK")?)?;
    let importing = async {
        let started = Instant::now();
        let preview = fixture
            .state
            .import()
            .try_begin()?
            .create_candidate(&bytes)
            .await?;
        let change = fixture
            .state
            .import()
            .try_begin()?
            .commit(
                preview.candidate_id(),
                preview.preview_token_bytes(),
                &bytes,
            )
            .await?;
        fixture.state.device_control().publish(change).await;
        tracing::info!("preview + commit: {:?}", started.elapsed());
        Ok::<_, crate::test_error::TestError>(())
    };
    let submitting = async {
        let mut jobs = Vec::new();
        for _ in 0..10 {
            jobs.push(async {
                let started = Instant::now();
                let outcome = fixture
                    .state
                    .target_submission()
                    .submit(
                        Uuid::now_v7(),
                        TargetSubmissionRequest {
                            operation_id: Uuid::now_v7(),
                            scope: TargetScope::AllEnabled,
                            action: TargetAction::ResetHome,
                        },
                        JournalContext::default(),
                    )
                    .await?;
                assert_eq!(outcome.result.results.len(), 600);
                assert!(
                    outcome
                        .result
                        .results
                        .iter()
                        .all(|row| row.rejection.is_none())
                );
                fixture.state.device_control().publish(outcome.change).await;
                Ok::<_, crate::test_error::TestError>(started.elapsed())
            });
        }
        let mut samples = futures_util::future::try_join_all(jobs).await?;
        report("ten simultaneous 600-device submissions", &mut samples);
        Ok::<_, crate::test_error::TestError>(())
    };
    let (finished, completed) = watch::channel(false);
    let operations = async {
        tokio::try_join!(importing, submitting)?;
        finished.send(true)?;
        Ok::<_, crate::test_error::TestError>(())
    };
    let observing = async {
        let mut samples = Vec::new();
        // Do not cancel an in-flight query when work finishes: its complete
        // latency must be recorded, including a wait behind the final commit.
        while !*completed.borrow() {
            samples.extend(observe_status(fixture, ids[0], 1, Duration::from_millis(100)).await?);
        }
        report("single-device status during import/burst", &mut samples);
        Ok::<_, crate::test_error::TestError>(())
    };
    tokio::try_join!(operations, observing)?;
    Ok(())
}
