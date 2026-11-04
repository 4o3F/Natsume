use super::*;
use crate::reconcile::observability::tests::{Capture, Value, assert_text, subscriber_guard};

fn canonical_snapshot() -> ServerStateSnapshot {
    let target = ConcreteTargetState::decode(
        include_bytes!("../../../../../natsume-workstation-protocol/testdata/target-digest.pb")
            .as_slice(),
    )
    .unwrap_or_else(|e| panic!("fixture: {e}"));
    ServerStateSnapshot {
        intent: Some(ServerIntentState {
            gateway_credential: Some(GatewayCredentialIntent {
                credential_id: target
                    .gateway
                    .as_ref()
                    .unwrap_or_else(|| panic!("gateway"))
                    .credential_id
                    .clone(),
            }),
            binding: None,
        }),
        target: Some(target),
    }
}

#[tokio::test]
async fn active_fixture_records_shared_digest_and_successful_send_sources() {
    let _guard = subscriber_guard();
    let capture = Capture::default();
    async {
        let mut active = active_fixture(HelperState::default())
            .await
            .unwrap_or_else(|e| panic!("active: {e}"));
        let initial = capture.records("actual sent");
        assert_eq!(initial.len(), 1);
        assert_text(&initial[0], "source", "observation");
        assert!(!initial[0].fields.contains_key("plan"));
        assert!(!initial[0].fields.contains_key("target_digest"));
        active
            .target(canonical_snapshot())
            .await
            .unwrap_or_else(|e| panic!("target: {e}"));
        active
            .next_snapshot()
            .await
            .unwrap_or_else(|e| panic!("snapshot: {e}"));
        let targets = capture.records("target received");
        assert_eq!(targets.len(), 1);
        let expected =
            include_str!("../../../../../natsume-workstation-protocol/testdata/target-digest.txt")
                .trim();
        assert!(validate_server_snapshot(canonical_snapshot()).is_ok());
        assert_text(&targets[0], "target_digest", expected);
        assert_text(&targets[0], "foreground_target", "contest");
        assert_text(&targets[0], "action", "started");
        assert_eq!(
            targets[0].fields.get("terminate_epoch"),
            Some(&Value::Unsigned(7))
        );
        assert_eq!(
            targets[0].fields.get("reset_epoch"),
            Some(&Value::Unsigned(9))
        );
        assert!(!targets[0].fields.contains_key("since_previous_ms"));
        let actuals = capture.records("actual sent");
        assert_eq!(actuals.len(), 2);
        assert_text(&actuals[1], "source", "observation");
        assert_text(&actuals[1], "target_digest", expected);
        assert!(!actuals[1].fields.contains_key("plan"));
        assert!(matches!(
            actuals[1].fields.get("waiting_ready"),
            Some(Value::Boolean(_))
        ));
        for message in ["target received", "actual sent", "reconcile completed"] {
            let encoded = format!("{:?}", capture.records(message));
            for canary in [
                "fictitious-observability-password-canary",
                "fixture-team",
                "A-01",
                "judge.example.test",
                "/tmp/",
            ] {
                assert!(!encoded.contains(canary));
            }
        }
    }
    .with_subscriber(capture.subscriber())
    .await;
}

#[tokio::test]
async fn received_actions_and_metadata_preserve_business_equality_and_retry() {
    let _guard = subscriber_guard();
    let capture = Capture::default();
    async {
        let resources = fixture(HelperState::default())
            .await
            .unwrap_or_else(|e| panic!("fixture: {e}"));
        let (mut current, mut pending) = (None, None);
        let mut retry = RetrySchedule::new();
        let mut previous = None;
        let mut counter = 0;
        let wire = canonical_snapshot();
        let mut receive = |target| {
            queue_latest_plan(
                &resources.snapshots,
                &mut current,
                &mut pending,
                &mut retry,
                false,
                true,
                target,
                &mut previous,
                &mut counter,
            )
        };
        receive(wire.clone()).unwrap_or_else(|e| panic!("receive: {e}"));
        receive(wire.clone()).unwrap_or_else(|e| panic!("repeat: {e}"));
        let mut presentation = wire.clone();
        presentation
            .target
            .as_mut()
            .and_then(|target| target.binding_access.as_mut())
            .and_then(|binding| binding.bound.as_mut())
            .and_then(|bound| bound.presentation.as_mut())
            .unwrap_or_else(|| panic!("presentation"))
            .team_name_en = "different-presentation-canary".to_owned();
        receive(presentation).unwrap_or_else(|e| panic!("presentation: {e}"));
        let mut password = wire;
        password
            .target
            .as_mut()
            .and_then(|target| target.binding_access.as_mut())
            .and_then(|binding| binding.bound.as_mut())
            .unwrap_or_else(|| panic!("bound"))
            .password = Some(natsume_workstation_protocol::generated::SecretBytes {
            value: b"different-password-canary".to_vec(),
        });
        receive(password).unwrap_or_else(|e| panic!("password: {e}"));
        assert!(receive(ServerStateSnapshot::default()).is_err());
        let records = capture.records("target received");
        assert_eq!(records.len(), 4);
        for (record, action) in records
            .iter()
            .zip(["queued", "coalesced", "coalesced", "queued"])
        {
            assert_text(record, "action", action);
        }
        assert!(records[1..].iter().all(|record| matches!(
            record.fields.get("since_previous_ms"),
            Some(Value::Unsigned(_))
        )));
        assert_eq!(
            records[0].fields.get("target_digest"),
            records[3].fields.get("target_digest")
        );
        assert_eq!(counter, 0);
        fence_plans(&resources.snapshots, current, pending).await;
    }
    .with_subscriber(capture.subscriber())
    .await;
}

#[tokio::test]
async fn unchanged_and_failed_sends_emit_no_event() {
    let _guard = subscriber_guard();
    let capture = Capture::default();
    async {
        let (mut socket, mut peer) = socket_pair().await;
        let session_id = *Uuid::now_v7().as_bytes();
        let context = crate::reconcile::observability::tests::context();
        let state = ClientStateSnapshot::default();
        let mut last = None;
        assert!(
            send_changed_snapshot(
                &mut socket,
                session_id,
                &mut last,
                state.clone(),
                SnapshotSource::Plan(&context)
            )
            .await
            .unwrap_or_else(|()| panic!("send"))
        );
        peer_snapshot(&mut peer).await;
        assert!(
            !send_changed_snapshot(
                &mut socket,
                session_id,
                &mut last,
                state.clone(),
                SnapshotSource::Observation(None)
            )
            .await
            .unwrap_or_else(|()| panic!("dedup"))
        );
        let actuals = capture.records("actual sent");
        assert_eq!(actuals.len(), 1);
        assert_text(&actuals[0], "source", "plan");
        assert_eq!(
            actuals[0].fields.get("plan"),
            Some(&Value::Unsigned(context.plan))
        );
        assert_text(&actuals[0], "target_digest", &context.target.target_digest);
        socket
            .close(None)
            .await
            .unwrap_or_else(|e| panic!("close: {e}"));
        drop(peer);
        last = None;
        assert!(
            send_changed_snapshot(
                &mut socket,
                session_id,
                &mut last,
                state,
                SnapshotSource::Observation(None)
            )
            .await
            .is_err()
        );
        assert_eq!(capture.records("actual sent").len(), 1);
        assert!(last.is_none());
    }
    .with_subscriber(capture.subscriber())
    .await;
}

async fn socket_pair() -> (Socket, WebSocketStream<TcpStream>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap_or_else(|e| panic!("listen: {e}"));
    let tcp = TcpStream::connect(
        listener
            .local_addr()
            .unwrap_or_else(|e| panic!("address: {e}")),
    )
    .await
    .unwrap_or_else(|e| panic!("connect: {e}"));
    let (peer, _) = listener
        .accept()
        .await
        .unwrap_or_else(|e| panic!("accept: {e}"));
    (
        WebSocketStream::from_raw_socket(MaybeTlsStream::Plain(tcp), Role::Client, None).await,
        WebSocketStream::from_raw_socket(peer, Role::Server, None).await,
    )
}

async fn peer_snapshot(socket: &mut WebSocketStream<TcpStream>) {
    loop {
        let message = timeout(Duration::from_secs(5), socket.next())
            .await
            .unwrap_or_else(|e| panic!("receive timeout: {e}"))
            .unwrap_or_else(|| panic!("closed"))
            .unwrap_or_else(|e| panic!("receive: {e}"));
        if matches!(message, WsMessage::Binary(_)) {
            return;
        }
        if let WsMessage::Ping(payload) = message {
            socket
                .send(WsMessage::Pong(payload))
                .await
                .unwrap_or_else(|e| panic!("pong: {e}"));
        }
    }
}

#[tokio::test]
async fn execution_counter_survives_two_active_leases() {
    let _guard = subscriber_guard();
    let capture = Capture::default();
    async {
        let resources = fixture(HelperState::default())
            .await
            .unwrap_or_else(|e| panic!("fixture: {e}"));
        let mut counter = 0;
        for lease in 1..=2 {
            let (client, mut peer) = socket_pair().await;
            let session_id = *Uuid::now_v7().as_bytes();
            let remote = async {
                peer_snapshot(&mut peer).await;
                peer.send(WsMessage::Binary(
                    ServerActiveEnvelope {
                        session_id: session_id.to_vec(),
                        body: Some(server_active_envelope::Body::ServerState(snapshot())),
                    }
                    .encode_to_vec()
                    .into(),
                ))
                .await
                .unwrap_or_else(|e| panic!("target: {e}"));
                peer_snapshot(&mut peer).await;
                peer.close(None)
                    .await
                    .unwrap_or_else(|e| panic!("close: {e}"));
            };
            timeout(Duration::from_secs(10), async {
                tokio::join!(
                    run_active(
                        client,
                        session_id,
                        Arc::clone(&resources.snapshots),
                        None,
                        &mut counter
                    ),
                    remote
                );
            })
            .await
            .unwrap_or_else(|e| panic!("lease: {e}"));
            assert_eq!(counter, lease);
        }
        let received = capture.records("target received");
        assert_eq!(received.len(), 2);
        assert!(
            received
                .iter()
                .all(|record| !record.fields.contains_key("since_previous_ms"))
        );
        let plans = capture
            .records("reconcile completed")
            .into_iter()
            .collect::<Vec<_>>();
        assert_eq!(
            plans.len(),
            2,
            "completed: {:?}",
            capture.records("reconcile completed")
        );
        for (record, plan) in plans.iter().zip(1..=2) {
            assert_eq!(record.fields.get("plan"), Some(&Value::Unsigned(plan)));
        }
    }
    .with_subscriber(capture.subscriber())
    .await;
}

#[tokio::test]
async fn superseded_attempt_still_records_cancelled_with_original_context() {
    let _guard = subscriber_guard();
    let capture = Capture::default();
    async {
        let resources = fixture(HelperState::default())
            .await
            .unwrap_or_else(|e| panic!("fixture: {e}"));
        let (mut current, mut pending) = (None, None);
        let mut retry = RetrySchedule::new();
        let mut last = None;
        let mut counter = 0;
        for wire in [snapshot(), canonical_snapshot()] {
            queue_latest_plan(
                &resources.snapshots,
                &mut current,
                &mut pending,
                &mut retry,
                false,
                false,
                wire,
                &mut last,
                &mut counter,
            )
            .unwrap_or_else(|e| panic!("queue: {e}"));
        }
        assert_eq!(counter, 1);
        let old = current.take().unwrap_or_else(|| panic!("current"));
        assert!(old.fence.is_cancelled());
        assert!(matches!(old.task.await, Ok(Err(SnapshotError::Cancelled))));
        let records = capture.records("reconcile completed");
        assert_eq!(records.len(), 1);
        assert_text(&records[0], "outcome", "cancelled");
        assert_text(
            &records[0],
            "target_digest",
            &old.context.target.target_digest,
        );
        assert_eq!(records[0].fields.get("plan"), Some(&Value::Unsigned(1)));
        assert_eq!(
            pending
                .as_ref()
                .map(|plan| plan.target.metadata.target_digest.as_str()),
            Some(
                natsume_workstation_protocol::target_digest(
                    canonical_snapshot()
                        .target
                        .as_ref()
                        .unwrap_or_else(|| panic!("target"))
                )
                .as_str()
            )
        );
        fence_plans(&resources.snapshots, None, pending).await;
    }
    .with_subscriber(capture.subscriber())
    .await;
}

#[tokio::test]
async fn retry_attempts_increment_and_observations_have_no_plan() {
    let _guard = subscriber_guard();
    let capture = Capture::default();
    async {
        let mut active = active_fixture(HelperState {
            home_failures: 1,
            ..HelperState::default()
        })
        .await
        .unwrap_or_else(|e| panic!("fixture: {e}"));
        active
            .target(destructive_target())
            .await
            .unwrap_or_else(|e| panic!("target: {e}"));
        loop {
            let state = active
                .next_snapshot()
                .await
                .unwrap_or_else(|e| panic!("snapshot: {e}"));
            if state
                .actual
                .as_ref()
                .and_then(|actual| actual.home.as_ref())
                .and_then(|home| home.completed_reset_epoch)
                == Some(8)
            {
                break;
            }
        }
        let plans = capture.records("reconcile completed");
        assert!(plans.len() >= 2);
        assert_text(&plans[0], "outcome", "retry");
        for (i, record) in plans.iter().enumerate() {
            assert_eq!(
                record.fields.get("plan"),
                Some(&Value::Unsigned(i as u64 + 1))
            );
            crate::reconcile::observability::tests::assert_partition(record);
        }
        active
            .resources
            .helper
            .lock()
            .unwrap_or_else(|e| panic!("helper: {e}"))
            .session_state = Some(natsume_workstation_ipc::GraphicalSessionState::Ambiguous);
        tokio::time::pause();
        tokio::time::advance(HEARTBEAT_INTERVAL).await;
        tokio::time::resume();
        active
            .next_snapshot()
            .await
            .unwrap_or_else(|e| panic!("observation: {e}"));
        let actuals = capture.records("actual sent");
        let last = actuals.last().unwrap_or_else(|| panic!("actual"));
        assert_text(last, "source", "observation");
        assert!(!last.fields.contains_key("plan"));
        assert_eq!(
            last.fields.get("target_digest"),
            plans[0].fields.get("target_digest")
        );
    }
    .with_subscriber(capture.subscriber())
    .await;
}

#[tokio::test]
async fn observation_keeps_its_original_metadata_when_a_new_target_is_queued() {
    let _guard = subscriber_guard();
    let capture = Capture::default();
    async {
        let resources = fixture(HelperState::default())
            .await
            .unwrap_or_else(|e| panic!("fixture: {e}"));
        let wire = snapshot();
        let metadata =
            TargetMetadata::new(wire.target.as_ref().unwrap_or_else(|| panic!("target")));
        let observation = start_observation(
            Arc::clone(&resources.snapshots),
            Some(ObservedTarget {
                state: Arc::new(
                    validate_server_snapshot(wire).unwrap_or_else(|e| panic!("valid: {e}")),
                ),
                metadata: metadata.clone(),
            }),
        );
        let (mut current, mut pending) = (None, None);
        queue_latest_plan(
            &resources.snapshots,
            &mut current,
            &mut pending,
            &mut RetrySchedule::new(),
            false,
            true,
            canonical_snapshot(),
            &mut None,
            &mut 0,
        )
        .unwrap_or_else(|e| panic!("queue: {e}"));
        let snapshot = observation
            .task
            .await
            .unwrap_or_else(|e| panic!("join: {e}"))
            .unwrap_or_else(|e| panic!("observe: {e}"));
        assert_eq!(
            observation
                .metadata
                .as_ref()
                .map(|target| target.target_digest.as_str()),
            Some(metadata.target_digest.as_str())
        );
        assert_ne!(
            observation
                .metadata
                .as_ref()
                .map(|target| target.target_digest.as_str()),
            pending
                .as_ref()
                .map(|plan| plan.target.metadata.target_digest.as_str())
        );
        let (mut socket, mut peer) = socket_pair().await;
        assert!(
            send_changed_snapshot(
                &mut socket,
                *Uuid::now_v7().as_bytes(),
                &mut None,
                snapshot,
                SnapshotSource::Observation(observation.metadata.as_ref())
            )
            .await
            .unwrap_or_else(|()| panic!("send"))
        );
        peer_snapshot(&mut peer).await;
        let records = capture.records("actual sent");
        assert_eq!(records.len(), 1);
        assert_text(&records[0], "source", "observation");
        assert_text(&records[0], "target_digest", &metadata.target_digest);
        assert!(!records[0].fields.contains_key("plan"));
        fence_plans(&resources.snapshots, current, pending).await;
    }
    .with_subscriber(capture.subscriber())
    .await;
}

#[tokio::test]
async fn pump_keeps_cancelled_completion_when_replacing_an_attempt() {
    let _guard = subscriber_guard();
    let capture = Capture::default();
    async {
        let mut active = active_fixture(HelperState::default())
            .await
            .unwrap_or_else(|e| panic!("fixture: {e}"));
        let targets = [snapshot(), canonical_snapshot()];
        let old_digest = natsume_workstation_protocol::target_digest(
            targets[0]
                .target
                .as_ref()
                .unwrap_or_else(|| panic!("target")),
        );
        for target in targets {
            active
                .socket
                .feed(WsMessage::Binary(
                    ServerActiveEnvelope {
                        session_id: active.session_id.to_vec(),
                        body: Some(server_active_envelope::Body::ServerState(target)),
                    }
                    .encode_to_vec()
                    .into(),
                ))
                .await
                .unwrap_or_else(|e| panic!("feed: {e}"));
        }
        active
            .socket
            .flush()
            .await
            .unwrap_or_else(|e| panic!("flush: {e}"));
        active
            .next_snapshot()
            .await
            .unwrap_or_else(|e| panic!("snapshot: {e}"));
        let completed = capture.records("reconcile completed");
        assert_eq!(completed.len(), 2);
        assert_text(&completed[0], "outcome", "cancelled");
        assert_text(&completed[0], "target_digest", &old_digest);
        assert_eq!(completed[0].fields.get("plan"), Some(&Value::Unsigned(1)));
        assert_eq!(completed[1].fields.get("plan"), Some(&Value::Unsigned(2)));
        let received = capture.records("target received");
        assert_text(&received[0], "action", "started");
        assert_text(&received[1], "action", "queued");
        assert!(
            capture
                .records("actual sent")
                .iter()
                .all(|record| record.fields.get("target_digest")
                    != Some(&Value::Text(old_digest.clone())))
        );
    }
    .with_subscriber(capture.subscriber())
    .await;
}
