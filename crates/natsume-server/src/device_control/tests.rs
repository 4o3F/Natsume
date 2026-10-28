mod resource_usage;

use std::{collections::HashSet, fs, path::PathBuf, sync::Arc, time::Duration};

use diesel::{
    Connection as _, ExpressionMethods, QueryDsl, RunQueryDsl, connection::SimpleConnection,
};
use ed25519_dalek::SigningKey;
use futures_util::{SinkExt as _, StreamExt as _};
use natsume_workstation_protocol::{
    CONTROL_ROUTE, CONTROL_SUBPROTOCOL,
    generated::{
        ActualState, BindingAccessActualState, BindingArtifactState, ClientActiveEnvelope,
        ClientHandshakeEnvelope, ClientInputState, ClientProof, ClientStateSnapshot,
        EnrollmentAttempt, EnrollmentEvidenceQuality, EnrollmentReviewState, GatewayActualState,
        GatewayState, HomeActualState, HomeState, RuntimeConfigActualState, RuntimeConfigState,
        ServerActiveEnvelope, ServerHandshakeEnvelope, SessionControlActualState, SessionState,
        client_active_envelope, client_handshake_envelope, client_proof, server_active_envelope,
        server_handshake_envelope,
    },
    sign_client_proof,
};
use prost::Message as _;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{Notify, mpsc},
    time::timeout,
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{Message as ClientMessage, client::IntoClientRequest as _},
};
use uuid::Uuid;

use crate::{
    component::device::{
        ControlPublicKey, DeviceError, DeviceId, EnrollmentReviewDecision, EnrollmentStartOutcome,
        EvidenceQuality, LifecycleOutcome, MachineHardwareId, ValidatedEnrollmentEvidence,
    },
    component::session::ForegroundTarget,
    db::{Database, DatabaseConfig, PersistenceError},
    diesel_schema::{
        binding_negotiations, device_home_targets, device_session_targets, gateway_credentials,
        runtime_config,
    },
    server_state::{self, ServerState},
};

use super::actor::{
    DeviceConnectionState, DeviceHandle, DeviceRegistry, TARGET_REFRESH_INTERVAL,
    TRANSIENT_FAILURE_LIMIT, TRANSIENT_RETRY_DELAY,
};

const MACHINE_HARDWARE_ID: &str = "a9aa9d04-3ece-5567-8260-910930ff5e03";
const DOMJUDGE_ORIGIN: &str = "https://domjudge.example.test";

fn admission(control: &super::DeviceControl) -> crate::lifecycle::Admission {
    control
        .lifecycle
        .admit()
        .unwrap_or_else(|| panic!("admission closed"))
}

async fn wait_for_failed_actor(handle: &DeviceHandle) {
    timeout(Duration::from_secs(5), async {
        while handle
            .enqueue_client_state(Uuid::nil(), valid_snapshot())
            .await
            .is_ok()
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("actor mailbox did not close"));
}

async fn assert_supervised_reattach(fixture: &Fixture) {
    let control = fixture.state.device_control();
    let machine =
        MachineHardwareId::parse(MACHINE_HARDWARE_ID).unwrap_or_else(|| panic!("machine ID"));
    let authority = control
        .device
        .find_current_authority(machine)
        .await
        .unwrap_or_else(|error| panic!("{}", snafu::Report::from_error(error)))
        .unwrap_or_else(|| panic!("authority"));
    let (outbound, mut outgoing) = mpsc::channel(1);
    let (session, handle) = control
        .attach_device_lease(&admission(control), machine, authority, outbound)
        .await
        .unwrap_or_else(|error| panic!("attach admission: {error}"))
        .unwrap_or_else(|| panic!("reattach failed"));
    assert!(
        handle
            .enqueue_client_state(session, valid_snapshot())
            .await
            .is_ok()
    );
    assert_complete_target(
        timeout(Duration::from_secs(5), outgoing.recv())
            .await
            .unwrap_or_else(|_| panic!("snapshot timeout"))
            .unwrap_or_else(|| panic!("lease closed")),
    );
}

#[tokio::test]
async fn supervision_reports_panics_without_payload_and_recovers_attachment() {
    use crate::{
        config::LogLevel,
        logging::tests::{CapturedLogs, SubscriberTestGuard},
    };
    use tracing::instrument::WithSubscriber as _;

    let _guard = SubscriberTestGuard::acquire();
    let captured = CapturedLogs::default();
    let device_id = async {
        let fixture = Fixture::new().await;
        let device_id = fixture.activate(&SigningKey::from_bytes(&[0x78; 32])).await;
        let (_handle, supervisor) = super::actor::spawn_test_actor(
            &fixture.state.device_control().registry,
            device_id,
            async {
                panic!("actor_panic_secret_canary");
            },
        )
        .await
        .unwrap_or_else(|| panic!("registry closed"));
        timeout(
            Duration::from_secs(5),
            tokio::spawn(supervisor.with_current_subscriber()),
        )
        .await
        .unwrap_or_else(|_| panic!("supervisor timeout"))
        .unwrap_or_else(|error| panic!("supervisor: {error}"));
        assert!(
            fixture
                .state
                .device_control()
                .registry
                .get(device_id)
                .await
                .is_none()
        );
        assert_supervised_reattach(&fixture).await;
        device_id
    }
    .with_subscriber(captured.subscriber(LogLevel::Trace))
    .await;
    let events = captured.text().unwrap_or_else(|()| panic!("logs"));
    assert_eq!(
        events.matches("Device actor stopped unexpectedly").count(),
        1
    );
    assert!(events.contains("ERROR"));
    assert!(events.contains("panicked=true"));
    assert!(events.contains(&device_id.as_text()));
    assert!(!events.contains("actor_panic_secret_canary"));
}

#[tokio::test]
async fn supervision_closed_entry_is_replaced_once_and_late_cleanup_preserves_replacement() {
    use crate::{
        config::LogLevel,
        logging::tests::{CapturedLogs, SubscriberTestGuard},
    };
    use tracing::instrument::WithSubscriber as _;

    let _guard = SubscriberTestGuard::acquire();
    let captured = CapturedLogs::default();
    async {
        let fixture = Fixture::new().await;
        let device_id = fixture.activate(&SigningKey::from_bytes(&[0x79; 32])).await;
        let registry = &fixture.state.device_control().registry;
        let (stale, supervisor) = super::actor::spawn_test_actor(registry, device_id, async {
            panic!("late_actor_secret_canary");
        })
        .await
        .unwrap_or_else(|| panic!("registry closed"));
        wait_for_failed_actor(&stale).await;
        *stale.authority_fence.lock().await = true;
        let (first, second) = tokio::join!(
            registry.get_or_spawn(device_id),
            registry.get_or_spawn(device_id)
        );
        let first = first.unwrap_or_else(|| panic!("first replacement"));
        let second = second.unwrap_or_else(|| panic!("second replacement"));
        assert!(Arc::ptr_eq(&first.authority_fence, &second.authority_fence));
        assert!(!Arc::ptr_eq(&first.authority_fence, &stale.authority_fence));
        assert!(!*first.authority_fence.lock().await);
        supervisor.await;
        let current = registry
            .get(device_id)
            .await
            .unwrap_or_else(|| panic!("late supervisor removed replacement"));
        assert!(Arc::ptr_eq(
            &current.authority_fence,
            &first.authority_fence
        ));
        assert_supervised_reattach(&fixture).await;
    }
    .with_subscriber(captured.subscriber(LogLevel::Trace))
    .await;
    let events = captured.text().unwrap_or_else(|()| panic!("logs"));
    assert_eq!(
        events
            .matches("Replacing closed Device actor mailbox")
            .count(),
        1
    );
    assert_eq!(
        events.matches("Device actor stopped unexpectedly").count(),
        1
    );
    assert!(events.contains("WARN"));
    assert!(!events.contains("late_actor_secret_canary"));
}

#[tokio::test]
async fn supervision_unexpected_clean_exit_is_reported_and_removed() {
    use crate::{
        config::LogLevel,
        logging::tests::{CapturedLogs, SubscriberTestGuard},
    };
    use tracing::instrument::WithSubscriber as _;

    let _guard = SubscriberTestGuard::acquire();
    let captured = CapturedLogs::default();
    async {
        let registry = DeviceRegistry::new();
        let device_id = DeviceId::parse("01900000-0000-7000-8000-000000000001")
            .unwrap_or_else(|| panic!("device ID"));
        let (_handle, supervisor) = super::actor::spawn_test_actor(&registry, device_id, async {})
            .await
            .unwrap_or_else(|| panic!("registry closed"));
        timeout(Duration::from_secs(5), supervisor)
            .await
            .unwrap_or_else(|_| panic!("supervisor timeout"));
        assert!(registry.get(device_id).await.is_none());
    }
    .with_subscriber(captured.subscriber(LogLevel::Trace))
    .await;
    let events = captured.text().unwrap_or_else(|()| panic!("logs"));
    assert_eq!(
        events.matches("Device actor stopped unexpectedly").count(),
        1
    );
    assert!(events.contains("panicked=false"));
}

#[tokio::test]
async fn supervision_respawn_waits_for_in_flight_disable_and_revoke_commits() {
    for revoke in [false, true] {
        assert_supervised_authority_commit(revoke).await;
    }
}

async fn assert_supervised_authority_commit(revoke: bool) {
    let fixture = Fixture::new().await;
    let device_id = fixture.activate(&SigningKey::from_bytes(&[0x7a; 32])).await;
    let control = fixture.state.device_control();
    let machine =
        MachineHardwareId::parse(MACHINE_HARDWARE_ID).unwrap_or_else(|| panic!("machine ID"));
    let authority = control
        .device
        .find_current_authority(machine)
        .await
        .unwrap_or_else(|error| panic!("{}", snafu::Report::from_error(error)))
        .unwrap_or_else(|| panic!("authority"));
    let (fail, failed) = tokio::sync::oneshot::channel();
    let (stale, supervisor) = super::actor::spawn_test_actor(&control.registry, device_id, async {
        let _ = failed.await;
        panic!("fenced actor panic canary");
    })
    .await
    .unwrap_or_else(|| panic!("registry closed"));
    let lock = crate::db::tests::test_lock_database(&fixture.path)
        .unwrap_or_else(|error| panic!("{}", snafu::Report::from_error(error)));
    let composition = Arc::clone(&fixture.state);
    let mutation = tokio::spawn(async move {
        if revoke {
            composition
                .device_control()
                .revoke_device(&admission(composition.device_control()), device_id)
                .await
        } else {
            composition
                .device_control()
                .disable_device(&admission(composition.device_control()), device_id)
                .await
        }
    });
    timeout(Duration::from_secs(2), async {
        while stale.authority_fence.try_lock().is_ok() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("authority mutation did not acquire its fence"));
    assert!(fail.send(()).is_ok());
    wait_for_failed_actor(&stale).await;
    tokio::pin!(supervisor);
    assert!(futures_util::poll!(&mut supervisor).is_pending());
    let respawn = control.registry.get_or_spawn(device_id);
    tokio::pin!(respawn);
    assert!(futures_util::poll!(&mut respawn).is_pending());
    assert!(control.registry.get(device_id).await.is_some());
    drop(lock);
    let outcome = timeout(Duration::from_secs(5), mutation)
        .await
        .unwrap_or_else(|_| panic!("mutation timeout"))
        .unwrap_or_else(|error| panic!("mutation task: {error}"));
    assert!(matches!(outcome, Ok(LifecycleOutcome::Changed)));
    supervisor.await;
    let replacement = timeout(Duration::from_secs(5), respawn)
        .await
        .unwrap_or_else(|_| panic!("respawn timeout"))
        .unwrap_or_else(|| panic!("registry closed"));
    assert!(!Arc::ptr_eq(
        &replacement.authority_fence,
        &stale.authority_fence
    ));
    assert!(!*replacement.authority_fence.lock().await);
    assert!(*stale.authority_fence.lock().await);
    let (outbound, _outgoing) = mpsc::channel(1);
    assert!(
        control
            .attach_device_lease(&admission(control), machine, authority, outbound)
            .await
            .unwrap_or_else(|error| panic!("attach admission: {error}"))
            .is_none()
    );
    let device = control
        .device
        .find_device(device_id)
        .await
        .unwrap_or_else(|error| panic!("{}", snafu::Report::from_error(error)))
        .unwrap_or_else(|| panic!("device missing"));
    assert_eq!(
        device.state(),
        if revoke {
            crate::component::device::DeviceState::Revoked
        } else {
            crate::component::device::DeviceState::Disabled
        }
    );
    assert_eq!(fixture.component_row_counts().await, [0, 0, 0, 0]);
}

#[tokio::test]
async fn supervision_authority_mutation_relocks_replacement_after_lookup_race() {
    for supervised in [false, true] {
        for revoke in [false, true] {
            assert_replacement_fence_is_current(supervised, revoke).await;
        }
    }
}

async fn assert_replacement_fence_is_current(supervised: bool, revoke: bool) {
    let fixture = Fixture::new().await;
    let device_id = fixture.activate(&SigningKey::from_bytes(&[0x7b; 32])).await;
    let control = fixture.state.device_control();
    let accepted = admission(control);
    let (exit, exited) = tokio::sync::oneshot::channel();
    let (stale, supervisor) = super::actor::spawn_test_actor(&control.registry, device_id, async {
        let _ = exited.await;
    })
    .await
    .unwrap_or_else(|| panic!("registry closed"));
    let gate = Arc::clone(&stale.authority_fence).lock_owned().await;
    assert!(exit.send(()).is_ok());
    wait_for_failed_actor(&stale).await;
    tokio::pin!(supervisor);
    let mutation = super::application::mutate_authority_for_test(control, device_id, revoke);
    tokio::pin!(mutation);
    let gap = {
        let cleanup = async {
            if supervised {
                supervisor.as_mut().await;
            } else {
                let _ = control.registry.get_or_spawn(device_id).await;
            }
        };
        tokio::pin!(cleanup);
        assert!(futures_util::poll!(&mut cleanup).is_pending());
        let hold_lookup_gap = Arc::clone(&stale.authority_fence).lock_owned();
        tokio::pin!(hold_lookup_gap);
        assert!(futures_util::poll!(&mut hold_lookup_gap).is_pending());
        assert!(futures_util::poll!(&mut mutation).is_pending());
        drop(gate);
        cleanup.await;
        hold_lookup_gap.await
    };
    let machine =
        MachineHardwareId::parse(MACHINE_HARDWARE_ID).unwrap_or_else(|| panic!("machine"));
    let authority = control
        .device
        .find_current_authority(machine)
        .await
        .unwrap_or_else(|error| panic!("{}", snafu::Report::from_error(error)))
        .unwrap_or_else(|| panic!("authority"));
    let (outbound, mut outgoing) = mpsc::channel(1);
    let (session, attached) = control
        .attach_device_lease(&accepted, machine, authority, outbound)
        .await
        .unwrap_or_else(|error| panic!("{error}"))
        .unwrap_or_else(|| panic!("replacement attach"));
    assert!(
        attached
            .enqueue_client_state(session, valid_snapshot())
            .await
            .is_ok()
    );
    assert_complete_target(
        timeout(Duration::from_secs(5), outgoing.recv())
            .await
            .unwrap_or_else(|_| panic!("snapshot timeout"))
            .unwrap_or_else(|| panic!("lease closed")),
    );
    let replacement = control
        .registry
        .get(device_id)
        .await
        .unwrap_or_else(|| panic!("replacement"));
    assert!(!Arc::ptr_eq(
        &replacement.authority_fence,
        &stale.authority_fence
    ));
    let lock = crate::db::tests::test_lock_database(&fixture.path)
        .unwrap_or_else(|error| panic!("{}", snafu::Report::from_error(error)));
    drop(gap);
    assert!(futures_util::poll!(&mut mutation).is_pending());
    timeout(Duration::from_secs(2), async {
        while replacement.authority_fence.try_lock().is_ok() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("mutation locked a retired actor"));
    drop(lock);
    let result = timeout(Duration::from_secs(5), mutation)
        .await
        .unwrap_or_else(|_| panic!("mutation timeout"));
    assert!(matches!(result, Ok(LifecycleOutcome::Changed)));
    assert!(
        timeout(Duration::from_secs(2), outgoing.recv())
            .await
            .unwrap_or_else(|_| panic!("replacement lease survived mutation"))
            .is_none()
    );
    let refused = control
        .attach_device_lease(&accepted, machine, authority, mpsc::channel(1).0)
        .await;
    assert!(matches!(refused, Ok(None)));
    if !supervised {
        supervisor.await;
    }
}

#[tokio::test]
async fn network_addresses_ignore_replaced_leases_and_survive_disconnect()
-> Result<(), crate::test_error::TestError> {
    let fixture = Fixture::new().await;
    let device_id = fixture.activate(&SigningKey::from_bytes(&[0x76; 32])).await;
    let control = fixture.state.device_control();
    let authority = control
        .device
        .find_current_authority(
            MachineHardwareId::parse(MACHINE_HARDWARE_ID)
                .ok_or(crate::test_error::TestError::missing("invalid machine"))?,
        )
        .await?
        .ok_or(crate::test_error::TestError::missing("missing authority"))?;
    let (outbound, _first_receiver) = mpsc::channel(1);
    let (old_session, handle) = replace_current_lease(&fixture.state, device_id, outbound).await;
    let (outbound, _current_receiver) = mpsc::channel(1);
    let (session, _) = replace_current_lease(&fixture.state, device_id, outbound).await;
    handle
        .record_network_addresses(
            session,
            authority,
            "192.0.2.20".parse()?,
            Some("192.0.2.20".parse()?),
        )
        .await;
    handle
        .record_network_addresses(old_session, authority, "192.0.2.99".parse()?, None)
        .await;
    handle.clear_lease_if_current(old_session).await;
    assert!(matches!(
        control.registry.read_connection_state(device_id).await,
        DeviceConnectionState::AwaitingFreshState
    ));
    let saved = control
        .device
        .find_device(device_id)
        .await?
        .ok_or(crate::test_error::TestError::missing("missing Device"))?
        .network()
        .ok_or(crate::test_error::TestError::missing("missing network"))?;
    assert_eq!(saved.server_observed_ip.to_string(), "192.0.2.20");
    assert_eq!(saved.client_ip, Some(saved.server_observed_ip));
    handle.clear_lease_if_current(session).await;
    assert!(matches!(
        control.registry.read_connection_state(device_id).await,
        DeviceConnectionState::Offline
    ));
    assert_eq!(
        control
            .device
            .find_device(device_id)
            .await?
            .ok_or(crate::test_error::TestError::missing("missing Device"))?
            .network(),
        Some(saved)
    );
    Ok(())
}

#[tokio::test]
async fn coordinator_survives_its_composition_and_leases_do_not_keep_it_alive() {
    let fixture = Fixture::new().await;
    let device_id = fixture.activate(&SigningKey::from_bytes(&[0x77; 32])).await;
    // The fixture keeps database files alive, independently of this composition.
    let composition = Arc::new(
        server_state::tests::for_test(
            fixture.database.clone(),
            fixture.path.with_extension("print-documents"),
        )
        .unwrap_or_else(|error| panic!("independent composition failed: {error}")),
    );
    let composition_lifetime = Arc::downgrade(&composition);
    let control = Arc::clone(composition.device_control());
    drop(composition);
    assert!(composition_lifetime.upgrade().is_none());

    let machine_hardware_id = MachineHardwareId::parse(MACHINE_HARDWARE_ID)
        .unwrap_or_else(|| panic!("the fixture Machine Hardware ID is valid"));
    let authority = control
        .device
        .find_current_authority(machine_hardware_id)
        .await
        .unwrap_or_else(|error| panic!("authority lookup failed: {error}"))
        .unwrap_or_else(|| panic!("the fixture authority is absent"));
    let (outbound, mut outgoing) = mpsc::channel(1);
    let (session_id, handle) = control
        .attach_device_lease(
            &admission(&control),
            machine_hardware_id,
            authority,
            outbound,
        )
        .await
        .unwrap_or_else(|error| panic!("attach admission: {error}"))
        .unwrap_or_else(|| panic!("coordinator could not attach without ServerState"));
    assert!(
        handle
            .enqueue_client_state(session_id, valid_snapshot())
            .await
            .is_ok()
    );
    let snapshot = timeout(Duration::from_secs(5), outgoing.recv())
        .await
        .unwrap_or_else(|_| panic!("coordinator did not reconcile without ServerState"))
        .unwrap_or_else(|| panic!("the current lease unexpectedly closed"));
    assert_complete_target(snapshot);
    assert!(matches!(
        control.registry.read_connection_state(device_id).await,
        DeviceConnectionState::Active { .. }
    ));

    let coordinator_lifetime = Arc::downgrade(&control);
    drop(control);
    assert!(coordinator_lifetime.upgrade().is_none());
    drop(handle);
    assert!(
        timeout(Duration::from_secs(5), outgoing.recv())
            .await
            .unwrap_or_else(|_| panic!("the actor did not release its current lease"))
            .is_none()
    );
}

#[tokio::test]
async fn actor_rejects_stale_and_invalid_frames_before_component_writes() {
    let fixture = Fixture::new().await;
    let device_id = fixture.activate(&SigningKey::from_bytes(&[0x61; 32])).await;

    let (outbound, mut first_outgoing) = mpsc::channel(1);
    let (first_session, handle) = replace_current_lease(&fixture.state, device_id, outbound).await;
    let (outbound, _second_outgoing) = mpsc::channel(1);
    let (_second_session, _) = replace_current_lease(&fixture.state, device_id, outbound).await;
    assert!(first_outgoing.recv().await.is_none());

    assert!(
        handle
            .enqueue_client_state(first_session, valid_snapshot())
            .await
            .is_ok()
    );
    let (outbound, _third_outgoing) = mpsc::channel(1);
    let (third_session, _) = replace_current_lease(&fixture.state, device_id, outbound).await;
    assert_eq!(fixture.component_row_counts().await, [0, 0, 0, 0]);

    assert!(
        handle
            .enqueue_client_state(
                third_session,
                ClientStateSnapshot {
                    input: None,
                    actual: None,
                },
            )
            .await
            .is_ok()
    );
    let (outbound, mut current_outgoing) = mpsc::channel(1);
    let (current_session, _) = replace_current_lease(&fixture.state, device_id, outbound).await;
    assert_eq!(fixture.component_row_counts().await, [0, 0, 0, 0]);

    assert!(
        handle
            .enqueue_client_state(current_session, valid_snapshot())
            .await
            .is_ok()
    );
    let envelope = timeout(Duration::from_secs(5), current_outgoing.recv())
        .await
        .unwrap_or_else(|_| panic!("the actor did not materialize the valid first snapshot"))
        .unwrap_or_else(|| panic!("the actor closed the valid lease"));
    assert_complete_target(envelope);
    assert_eq!(fixture.component_row_counts().await, [1, 1, 1, 0]);
}

#[tokio::test]
async fn full_outbound_queue_terminates_the_current_lease() {
    let fixture = Fixture::new().await;
    let device_id = fixture.activate(&SigningKey::from_bytes(&[0x62; 32])).await;
    let (outbound, mut outgoing) = mpsc::channel(1);
    let (session_id, handle) = replace_current_lease(&fixture.state, device_id, outbound).await;

    assert!(
        handle
            .enqueue_client_state(session_id, valid_snapshot())
            .await
            .is_ok()
    );
    timeout(Duration::from_secs(5), async {
        while outgoing.len() != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("the first target did not fill the outbound queue"));
    assert!(
        handle
            .enqueue_client_state(session_id, valid_snapshot())
            .await
            .is_ok()
    );

    let (replacement_outbound, _replacement_outgoing) = mpsc::channel(1);
    let _ = replace_current_lease(&fixture.state, device_id, replacement_outbound).await;
    assert!(outgoing.recv().await.is_some());
    assert!(outgoing.recv().await.is_none());
}

#[tokio::test]
async fn failure_transient_client_state_keeps_the_lease_and_retries() {
    let fixture = Fixture::new().await;
    let device_id = fixture.activate(&SigningKey::from_bytes(&[0x81; 32])).await;
    let (outbound, mut outgoing) = mpsc::channel(1);
    let (session, handle) = replace_current_lease(&fixture.state, device_id, outbound).await;
    let failures = fixture.query_failures();
    let lock = crate::db::tests::test_lock_database(&fixture.path)
        .unwrap_or_else(|error| panic!("{}", snafu::Report::from_error(error)));
    handle
        .enqueue_client_state(session, valid_snapshot())
        .await
        .unwrap_or_else(|error| panic!("{}", snafu::Report::from_error(error)));
    await_query_failure(&failures).await;
    assert!(matches!(
        fixture
            .state
            .device_control()
            .registry
            .read_connection_state(device_id)
            .await,
        DeviceConnectionState::AwaitingFreshState
    ));
    assert!(!outgoing.is_closed());
    let failed_at = time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000;
    drop(lock);

    let envelope = timeout(
        TRANSIENT_RETRY_DELAY + Duration::from_secs(2),
        outgoing.recv(),
    )
    .await
    .unwrap_or_else(|_| panic!("the pending snapshot was not retried"))
    .unwrap_or_else(|| panic!("the transient failure closed the lease"));
    assert_complete_target(envelope);
    let DeviceConnectionState::Active {
        received_at_unix_ms,
        ..
    } = fixture
        .state
        .device_control()
        .registry
        .read_connection_state(device_id)
        .await
    else {
        panic!("the retried snapshot did not become active");
    };
    assert!(i128::from(received_at_unix_ms) <= failed_at);
}

#[tokio::test]
async fn failure_invalid_csr_still_terminates_the_lease() {
    let fixture = Fixture::new().await;
    let device_id = fixture.activate(&SigningKey::from_bytes(&[0x82; 32])).await;
    let (outbound, mut outgoing) = mpsc::channel(1);
    let (session, handle) = replace_current_lease(&fixture.state, device_id, outbound).await;
    let mut snapshot = valid_snapshot();
    snapshot
        .input
        .as_mut()
        .unwrap_or_else(|| panic!("fixture input"))
        .gateway_credential = Some(
        natsume_workstation_protocol::generated::GatewayCredentialInput {
            credential_id: Uuid::now_v7().to_string(),
            gateway_csr_der: Some(vec![0xff]),
        },
    );
    handle
        .enqueue_client_state(session, snapshot)
        .await
        .unwrap_or_else(|error| panic!("{}", snafu::Report::from_error(error)));
    assert!(
        timeout(Duration::from_secs(2), outgoing.recv())
            .await
            .unwrap_or_else(|_| panic!("invalid CSR did not close the lease"))
            .is_none()
    );
    assert_eq!(fixture.component_row_counts().await, [0, 0, 0, 0]);
}

#[tokio::test]
async fn failure_transient_limit_is_shared_by_refresh_and_client_state() {
    let fixture = Fixture::new().await;
    let device_id = fixture.activate(&SigningKey::from_bytes(&[0x83; 32])).await;
    let (outbound, mut outgoing) = mpsc::channel(1);
    let (session, handle) = replace_current_lease(&fixture.state, device_id, outbound).await;
    handle
        .enqueue_client_state(session, valid_snapshot())
        .await
        .unwrap_or_else(|error| panic!("{}", snafu::Report::from_error(error)));
    assert_complete_target(
        timeout(Duration::from_secs(2), outgoing.recv())
            .await
            .unwrap_or_else(|_| panic!("initial target timed out"))
            .unwrap_or_else(|| panic!("initial lease closed")),
    );
    let failures = fixture.query_failures();
    let _lock = crate::db::tests::test_lock_database(&fixture.path)
        .unwrap_or_else(|error| panic!("{}", snafu::Report::from_error(error)));
    fixture
        .state
        .device_control()
        .publish(super::TargetChange::device(device_id))
        .await;
    await_query_failure(&failures).await;
    assert!(matches!(
        fixture
            .state
            .device_control()
            .registry
            .read_connection_state(device_id)
            .await,
        DeviceConnectionState::Active { .. }
    ));
    assert!(!outgoing.is_closed());

    for attempt in 2..=TRANSIENT_FAILURE_LIMIT {
        handle
            .enqueue_client_state(session, valid_snapshot())
            .await
            .unwrap_or_else(|error| panic!("{}", snafu::Report::from_error(error)));
        await_query_failure(&failures).await;
        let state = fixture
            .state
            .device_control()
            .registry
            .read_connection_state(device_id)
            .await;
        if attempt < TRANSIENT_FAILURE_LIMIT {
            assert!(matches!(state, DeviceConnectionState::Active { .. }));
            assert!(!outgoing.is_closed());
        } else {
            assert!(matches!(state, DeviceConnectionState::Offline));
        }
    }
    assert!(outgoing.recv().await.is_none());
}

#[tokio::test]
async fn failure_fencing_evicts_a_lease_with_pending_transient_work() {
    let fixture = Fixture::new().await;
    let device_id = fixture.activate(&SigningKey::from_bytes(&[0x84; 32])).await;
    let (outbound, mut outgoing) = mpsc::channel(1);
    let (session, handle) = replace_current_lease(&fixture.state, device_id, outbound).await;
    let failures = fixture.query_failures();
    let lock = crate::db::tests::test_lock_database(&fixture.path)
        .unwrap_or_else(|error| panic!("{}", snafu::Report::from_error(error)));
    handle
        .enqueue_client_state(session, valid_snapshot())
        .await
        .unwrap_or_else(|error| panic!("{}", snafu::Report::from_error(error)));
    await_query_failure(&failures).await;
    assert!(matches!(
        fixture
            .state
            .device_control()
            .registry
            .read_connection_state(device_id)
            .await,
        DeviceConnectionState::AwaitingFreshState
    ));
    drop(lock);

    assert!(matches!(
        fixture
            .state
            .device_control()
            .disable_device(&admission(fixture.state.device_control()), device_id)
            .await,
        Ok(LifecycleOutcome::Changed)
    ));
    assert!(
        timeout(Duration::from_secs(2), outgoing.recv())
            .await
            .unwrap_or_else(|_| panic!("disable did not evict the pending lease"))
            .is_none()
    );
    handle
        .enqueue_client_state(session, valid_snapshot())
        .await
        .unwrap_or_else(|error| panic!("{}", snafu::Report::from_error(error)));
    assert!(matches!(
        fixture
            .state
            .device_control()
            .registry
            .read_connection_state(device_id)
            .await,
        DeviceConnectionState::Offline
    ));
    assert_eq!(fixture.component_row_counts().await, [0, 0, 0, 0]);
}

#[tokio::test]
async fn failure_newer_snapshot_replaces_pending_and_success_resets_the_limit() {
    let fixture = Fixture::new().await;
    let device_id = fixture.activate(&SigningKey::from_bytes(&[0x85; 32])).await;
    let (outbound, mut outgoing) = mpsc::channel(1);
    let (session, handle) = replace_current_lease(&fixture.state, device_id, outbound).await;
    let failures = fixture.query_failures();
    let lock = crate::db::tests::test_lock_database(&fixture.path)
        .unwrap_or_else(|error| panic!("{}", snafu::Report::from_error(error)));
    for _ in 1..TRANSIENT_FAILURE_LIMIT {
        handle
            .enqueue_client_state(session, valid_snapshot())
            .await
            .unwrap_or_else(|error| panic!("{}", snafu::Report::from_error(error)));
        await_query_failure(&failures).await;
        assert!(matches!(
            fixture
                .state
                .device_control()
                .registry
                .read_connection_state(device_id)
                .await,
            DeviceConnectionState::AwaitingFreshState
        ));
    }
    drop(lock);
    let mut newer = valid_snapshot();
    newer
        .actual
        .as_mut()
        .unwrap_or_else(|| panic!("fixture Actual"))
        .runtime_config = Some(RuntimeConfigActualState {
        state: RuntimeConfigState::Applied.into(),
        applied_domjudge_origin: Some(DOMJUDGE_ORIGIN.to_owned()),
    });
    let (_, expected) = super::convergence::parse_actual(
        newer
            .actual
            .clone()
            .unwrap_or_else(|| panic!("fixture Actual")),
    )
    .unwrap_or_else(|| panic!("new fixture Actual must validate"));
    handle
        .enqueue_client_state(session, newer)
        .await
        .unwrap_or_else(|error| panic!("{}", snafu::Report::from_error(error)));
    assert_complete_target(
        timeout(Duration::from_secs(2), outgoing.recv())
            .await
            .unwrap_or_else(|_| panic!("new complete state waited for the retry timer"))
            .unwrap_or_else(|| panic!("new complete state closed the lease")),
    );
    assert!(
        matches!(fixture.state.device_control().registry.read_connection_state(device_id).await,
        DeviceConnectionState::Active { actual, .. } if *actual == expected)
    );

    let _lock = crate::db::tests::test_lock_database(&fixture.path)
        .unwrap_or_else(|error| panic!("{}", snafu::Report::from_error(error)));
    handle
        .enqueue_client_state(session, valid_snapshot())
        .await
        .unwrap_or_else(|error| panic!("{}", snafu::Report::from_error(error)));
    await_query_failure(&failures).await;
    assert!(matches!(
        fixture
            .state
            .device_control()
            .registry
            .read_connection_state(device_id)
            .await,
        DeviceConnectionState::Active { .. }
    ));
    assert!(!outgoing.is_closed());
    handle.evict_current_lease().await;
}

#[tokio::test]
async fn failure_transient_refresh_retries_without_a_pending_client_snapshot() {
    let fixture = Fixture::new().await;
    let device_id = fixture.activate(&SigningKey::from_bytes(&[0x86; 32])).await;
    let (outbound, mut outgoing) = mpsc::channel(1);
    let (session, handle) = replace_current_lease(&fixture.state, device_id, outbound).await;
    handle
        .enqueue_client_state(session, valid_snapshot())
        .await
        .unwrap_or_else(|error| panic!("{}", snafu::Report::from_error(error)));
    assert_complete_target(
        timeout(Duration::from_secs(2), outgoing.recv())
            .await
            .unwrap_or_else(|_| panic!("initial target timed out"))
            .unwrap_or_else(|| panic!("initial lease closed")),
    );
    let failures = fixture.query_failures();
    let lock = crate::db::tests::test_lock_database(&fixture.path)
        .unwrap_or_else(|error| panic!("{}", snafu::Report::from_error(error)));
    fixture
        .state
        .device_control()
        .publish(super::TargetChange::device(device_id))
        .await;
    await_query_failure(&failures).await;
    assert!(matches!(
        fixture
            .state
            .device_control()
            .registry
            .read_connection_state(device_id)
            .await,
        DeviceConnectionState::Active { .. }
    ));
    assert!(!outgoing.is_closed());
    drop(lock);
    fixture
        .state
        .session()
        .set_foreground(device_id, ForegroundTarget::Contest)
        .await
        .unwrap_or_else(|error| panic!("{}", snafu::Report::from_error(error)));
    let envelope = timeout(
        TRANSIENT_RETRY_DELAY + Duration::from_secs(2),
        outgoing.recv(),
    )
    .await
    .unwrap_or_else(|_| panic!("refresh was not retried"))
    .unwrap_or_else(|| panic!("refresh failure closed the lease"));
    let Some(server_active_envelope::Body::ServerState(ref snapshot)) = envelope.body else {
        panic!("retry must return ServerState");
    };
    assert_eq!(
        snapshot
            .target
            .as_ref()
            .and_then(|target| target.session_control.as_ref())
            .map(|target| target.foreground_target),
        Some(natsume_workstation_protocol::generated::ForegroundTarget::Contest.into())
    );
    assert_complete_target(envelope);
}

#[test]
fn reconcile_faults_distinguish_client_transient_and_server_errors() {
    use super::state::{ReconcileError, ReconcileFault};
    use crate::component::{
        binding::BindingError, gateway::GatewayError, home::HomeError, power::PowerError,
        runtime::RuntimeConfigError, session::SessionControlError,
    };
    for error in [
        ReconcileError::GatewayIngest {
            source: GatewayError::PersistenceFailed {
                source: PersistenceError::WorkBudgetExhausted,
            },
        },
        ReconcileError::GatewayMaterialize {
            source: GatewayError::PersistenceFailed {
                source: PersistenceError::WorkBudgetExhausted,
            },
        },
        ReconcileError::BindingIngest {
            source: BindingError::PersistenceFailed {
                source: PersistenceError::WorkBudgetExhausted,
            },
        },
        ReconcileError::BindingMaterialize {
            source: BindingError::PersistenceFailed {
                source: PersistenceError::WorkBudgetExhausted,
            },
        },
        ReconcileError::RuntimeMaterialize {
            source: RuntimeConfigError::PersistenceFailed {
                source: PersistenceError::WorkBudgetExhausted,
            },
        },
        ReconcileError::SessionMaterialize {
            source: SessionControlError::PersistenceFailed {
                source: PersistenceError::WorkBudgetExhausted,
            },
        },
        ReconcileError::HomeMaterialize {
            source: HomeError::PersistenceFailed {
                source: PersistenceError::WorkBudgetExhausted,
            },
        },
        ReconcileError::PowerMaterialize {
            source: PowerError::PersistenceFailed {
                source: PersistenceError::WorkBudgetExhausted,
            },
        },
    ] {
        assert_eq!(error.fault(), ReconcileFault::ServerTransient);
    }
    for error in [
        ReconcileError::InvalidSnapshot,
        ReconcileError::GatewayIngest {
            source: GatewayError::InvalidCsr,
        },
        ReconcileError::GatewayIngest {
            source: GatewayError::ConflictingCsr,
        },
        ReconcileError::BindingIngest {
            source: BindingError::ConflictingSubmission,
        },
    ] {
        assert_eq!(error.fault(), ReconcileFault::Client);
    }
    for error in [
        ReconcileError::GatewayMaterialize {
            source: GatewayError::InvalidCsr,
        },
        ReconcileError::BindingMaterialize {
            source: BindingError::DeviceNotFound,
        },
        ReconcileError::HomeMaterialize {
            source: HomeError::EpochExhausted,
        },
        ReconcileError::SessionMaterialize {
            source: SessionControlError::InvalidPersistedFacts {
                source: PersistenceError::invalid("session"),
            },
        },
        ReconcileError::RuntimeMaterialize {
            source: RuntimeConfigError::PersistenceFailed {
                source: PersistenceError::from(diesel::result::Error::NotFound),
            },
        },
    ] {
        assert_eq!(error.fault(), ReconcileFault::Server);
    }
}

#[tokio::test]
async fn cancelled_http_mutation_commits_and_publishes_the_complete_target()
-> Result<(), crate::test_error::TestError> {
    use crate::component::operator::{
        OperatorCredentials,
        tests::{PasswordVerificationTestGuard, test_insert_admin_account},
    };
    use axum::{
        body::Body,
        http::{Request, StatusCode, header},
    };
    use tower::ServiceExt as _;

    let _guard = PasswordVerificationTestGuard::acquire().await;
    let fixture = Fixture::new().await;
    let device_id = fixture.activate(&SigningKey::from_bytes(&[0x77; 32])).await;
    let (outbound, mut outgoing) = mpsc::channel(1);
    let (session_id, handle) = replace_current_lease(&fixture.state, device_id, outbound).await;
    assert!(
        handle
            .enqueue_client_state(session_id, valid_snapshot())
            .await
            .is_ok()
    );
    assert_complete_target(
        timeout(Duration::from_secs(5), outgoing.recv())
            .await?
            .ok_or(crate::test_error::TestError::missing("initial snapshot"))?,
    );

    let credentials = OperatorCredentials::new(
        "completion-admin".to_owned(),
        "completion-password1!".to_owned(),
        "completion-password1!".to_owned(),
    )?;
    test_insert_admin_account(
        &fixture.database,
        credentials.login_name(),
        &credentials.hash_password()?,
    )
    .await?;
    let application = crate::http::router(
        Arc::clone(&fixture.state),
        std::path::Path::new("/nonexistent/completion-web"),
    );
    let login = application.clone().oneshot(Request::builder().method("POST").uri("/api/v2/session").header(header::CONTENT_TYPE, "application/json").body(Body::from(serde_json::json!({"login_name":"completion-admin", "password":"completion-password1!"}).to_string()))?).await?;
    assert_eq!(login.status(), StatusCode::OK);
    let cookie = login.headers()[header::SET_COOKIE]
        .to_str()?
        .split(';')
        .next()
        .ok_or(crate::test_error::TestError::missing("cookie"))?
        .to_owned();

    let writing = Arc::new(Notify::new());
    for mut connection in fixture.database.test_exhaust_pool() {
        connection.set_instrumentation(WriteStarted(Arc::clone(&writing)));
    }
    let lock = crate::db::tests::test_lock_database(&fixture.path)?;
    let operation_id = Uuid::now_v7();
    let request = Request::builder().method("POST").uri("/api/v2/target-submissions").header(header::COOKIE, cookie).header(header::CONTENT_TYPE, "application/json").body(Body::from(serde_json::json!({"operation_id":operation_id, "scope":{"kind":"devices", "device_ids":[device_id.as_text()]}, "action":{"kind":"reset_home"}}).to_string()))?;
    let request_task = tokio::spawn(application.oneshot(request));
    timeout(Duration::from_secs(2), writing.notified()).await?;
    assert!(!request_task.is_finished());
    request_task.abort();
    assert!(request_task.await.is_err_and(|error| error.is_cancelled()));
    drop(lock);

    let envelope = timeout(Duration::from_secs(5), outgoing.recv())
        .await?
        .ok_or(crate::test_error::TestError::missing(
            "post-commit snapshot",
        ))?;
    assert_complete_target(envelope.clone());
    let Some(server_active_envelope::Body::ServerState(snapshot)) = envelope.body else {
        panic!("snapshot absent");
    };
    assert_eq!(
        snapshot
            .target
            .and_then(|target| target.home)
            .and_then(|home| home.reset_epoch),
        Some(1)
    );
    assert_eq!(
        fixture
            .state
            .home()
            .read_current(device_id)
            .await
            .unwrap_or_else(|error| panic!("{}", snafu::Report::from_error(error))),
        Some(1)
    );
    let receipts = fixture
        .database
        .read(move |tx| {
            use crate::diesel_schema::target_submission_receipts;
            target_submission_receipts::table
                .filter(target_submission_receipts::operation_id.eq(operation_id.to_string()))
                .count()
                .get_result::<i64>(tx.connection())
                .map_err(PersistenceError::from)
        })
        .await?;
    assert_eq!(receipts, 1);
    Ok(())
}

struct WriteStarted(Arc<Notify>);

impl diesel::connection::Instrumentation for WriteStarted {
    fn on_connection_event(&mut self, event: diesel::connection::InstrumentationEvent<'_>) {
        if let diesel::connection::InstrumentationEvent::StartQuery { query, .. } = event
            && query.to_string() == "BEGIN IMMEDIATE"
        {
            self.0.notify_one();
        }
    }
}

struct QueryFailures(Arc<Notify>);

impl diesel::connection::Instrumentation for QueryFailures {
    fn on_connection_event(&mut self, event: diesel::connection::InstrumentationEvent<'_>) {
        if matches!(
            event,
            diesel::connection::InstrumentationEvent::FinishQuery { error: Some(_), .. }
        ) {
            self.0.notify_one();
        }
    }
}

async fn await_query_failure(failures: &Notify) {
    timeout(Duration::from_secs(2), failures.notified())
        .await
        .unwrap_or_else(|_| panic!("the actor did not encounter the injected SQLite lock"));
}

#[tokio::test]
async fn dirty_refreshes_the_complete_target_after_commit() {
    let fixture = Fixture::new().await;
    let device_id = fixture.activate(&SigningKey::from_bytes(&[0x63; 32])).await;
    let (outbound, mut outgoing) = mpsc::channel(1);
    let (session_id, handle) = replace_current_lease(&fixture.state, device_id, outbound).await;

    assert!(
        handle
            .enqueue_client_state(session_id, valid_snapshot())
            .await
            .is_ok()
    );
    let _initial = timeout(Duration::from_secs(5), outgoing.recv())
        .await
        .unwrap_or_else(|_| panic!("the initial target was not emitted"))
        .unwrap_or_else(|| panic!("the active lease closed unexpectedly"));

    fixture
        .state
        .session()
        .set_foreground(device_id, ForegroundTarget::Contest)
        .await
        .unwrap_or_else(|error| panic!("Session Control mutation failed: {error:?}"));
    fixture
        .state
        .device_control()
        .publish(super::TargetChange::device(device_id))
        .await;

    let refreshed = timeout(Duration::from_secs(5), outgoing.recv())
        .await
        .unwrap_or_else(|_| panic!("Dirty did not refresh the target"))
        .unwrap_or_else(|| panic!("Dirty closed the active lease"));
    let Some(server_active_envelope::Body::ServerState(snapshot)) = refreshed.body else {
        panic!("Dirty did not emit ServerState");
    };
    assert_eq!(
        snapshot
            .target
            .and_then(|target| target.session_control)
            .map(|target| target.foreground_target),
        Some(natsume_workstation_protocol::generated::ForegroundTarget::Contest.into())
    );
}

#[tokio::test]
async fn periodic_refresh_recovers_the_current_target_without_dirty() {
    let fixture = Fixture::new().await;
    let device_id = fixture.activate(&SigningKey::from_bytes(&[0x73; 32])).await;
    let (outbound, mut outgoing) = mpsc::channel(1);
    let (session_id, handle) = replace_current_lease(&fixture.state, device_id, outbound).await;

    assert!(
        handle
            .enqueue_client_state(session_id, valid_snapshot())
            .await
            .is_ok()
    );
    let _initial = timeout(Duration::from_secs(5), outgoing.recv())
        .await
        .unwrap_or_else(|_| panic!("the initial target was not emitted"))
        .unwrap_or_else(|| panic!("the active lease closed unexpectedly"));

    fixture
        .state
        .session()
        .set_foreground(device_id, ForegroundTarget::Contest)
        .await
        .unwrap_or_else(|error| panic!("Session Control mutation failed: {error:?}"));
    tokio::time::pause();
    tokio::time::advance(TARGET_REFRESH_INTERVAL).await;

    let refreshed = timeout(Duration::from_secs(5), outgoing.recv())
        .await
        .unwrap_or_else(|_| panic!("periodic refresh did not emit the current target"))
        .unwrap_or_else(|| panic!("periodic refresh closed the active lease"));
    let Some(server_active_envelope::Body::ServerState(snapshot)) = refreshed.body else {
        panic!("periodic refresh did not emit ServerState");
    };
    assert_eq!(
        snapshot
            .target
            .and_then(|target| target.session_control)
            .map(|target| target.foreground_target),
        Some(natsume_workstation_protocol::generated::ForegroundTarget::Contest.into())
    );
}

#[tokio::test]
async fn connection_query_is_lease_scoped_and_evict_closes_the_outbound_path() {
    let fixture = Fixture::new().await;
    let device_id = fixture.activate(&SigningKey::from_bytes(&[0x64; 32])).await;
    let (outbound, mut outgoing) = mpsc::channel(1);
    let (session_id, handle) = replace_current_lease(&fixture.state, device_id, outbound).await;

    assert!(matches!(
        fixture
            .state
            .device_control()
            .registry
            .read_connection_state(device_id)
            .await,
        DeviceConnectionState::AwaitingFreshState
    ));
    let expected_actual = valid_snapshot()
        .actual
        .unwrap_or_else(|| panic!("the fixture Actual is absent"));
    let (_, expected_observation) = super::convergence::parse_actual(expected_actual.clone())
        .unwrap_or_else(|| panic!("the fixture Actual is valid"));
    assert!(
        handle
            .enqueue_client_state(
                session_id,
                ClientStateSnapshot {
                    input: Some(ClientInputState {
                        gateway_credential: None,
                        binding: None,
                    }),
                    actual: Some(expected_actual.clone()),
                },
            )
            .await
            .is_ok()
    );
    let _target = timeout(Duration::from_secs(5), outgoing.recv())
        .await
        .unwrap_or_else(|_| panic!("the initial target was not emitted"))
        .unwrap_or_else(|| panic!("the active lease closed unexpectedly"));
    match fixture
        .state
        .device_control()
        .registry
        .read_connection_state(device_id)
        .await
    {
        DeviceConnectionState::Active {
            actual,
            received_at_unix_ms,
        } => {
            assert_eq!(*actual, expected_observation);
            assert!(received_at_unix_ms > 0);
        }
        _ => panic!("the current fresh Actual was not reported"),
    }

    fixture
        .state
        .device_control()
        .evict_current_lease(device_id)
        .await;
    assert!(matches!(
        fixture
            .state
            .device_control()
            .registry
            .read_connection_state(device_id)
            .await,
        DeviceConnectionState::Offline
    ));
    assert!(
        timeout(Duration::from_secs(5), outgoing.recv())
            .await
            .unwrap_or_else(|_| panic!("Evict did not close the outbound path"))
            .is_none()
    );
}

#[tokio::test]
async fn authority_commit_fences_a_waiting_old_client_state_before_component_writes() {
    let fixture = Fixture::new().await;
    let device_id = fixture.activate(&SigningKey::from_bytes(&[0x6b; 32])).await;
    let (outbound, mut outgoing) = mpsc::channel(1);
    let (session_id, handle) = replace_current_lease(&fixture.state, device_id, outbound).await;

    let gate = handle.authority_fence.lock().await;
    let state = Arc::clone(&fixture.state);
    let disable = tokio::spawn(async move {
        state
            .device_control()
            .disable_device(&admission(state.device_control()), device_id)
            .await
    });
    tokio::task::yield_now().await;
    assert!(
        handle
            .enqueue_client_state(session_id, valid_snapshot())
            .await
            .is_ok()
    );
    drop(gate);

    assert!(matches!(disable
            .await
            .unwrap_or_else(|error| panic!("disable task failed: {error}")), Ok(actual) if actual == LifecycleOutcome::Changed));
    assert_eq!(fixture.component_row_counts().await, [0, 0, 0, 0]);
    assert!(outgoing.recv().await.is_none());
}

#[tokio::test]
async fn cancelled_request_does_not_cancel_authority_fence_completion() {
    let fixture = Fixture::new().await;
    let device_id = fixture.activate(&SigningKey::from_bytes(&[0x6c; 32])).await;
    let (outbound, mut outgoing) = mpsc::channel(1);
    let (_session_id, handle) = replace_current_lease(&fixture.state, device_id, outbound).await;

    let gate = handle.authority_fence.lock().await;
    let admitted = admission(fixture.state.device_control());
    let mut disable = Box::pin(
        fixture
            .state
            .device_control()
            .disable_device(&admitted, device_id),
    );
    assert!(futures_util::poll!(&mut disable).is_pending());
    drop(disable);
    drop(gate);

    timeout(Duration::from_secs(5), async {
        loop {
            let device = fixture
                .state
                .device()
                .find_device(device_id)
                .await
                .unwrap_or_else(|error| panic!("Device lookup failed: {error:?}"))
                .unwrap_or_else(|| panic!("the Device disappeared"));
            if device.state() == crate::component::device::DeviceState::Disabled {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("detached authority mutation did not complete"));
    assert!(outgoing.recv().await.is_none());
}

#[tokio::test]
async fn missing_lifecycle_mutations_do_not_create_an_actor() {
    let fixture = Fixture::new().await;
    let missing = DeviceId::parse("01900000-0000-7000-8000-000000000099")
        .unwrap_or_else(|| panic!("missing Device fixture ID is invalid"));

    assert!(
        fixture
            .state
            .device_control()
            .read_device_status(missing)
            .await
            .unwrap_or_else(|_| panic!("missing Device query failed"))
            .is_none()
    );
    fixture
        .state
        .device_control()
        .publish(super::TargetChange::device(missing))
        .await;
    fixture
        .state
        .device_control()
        .publish(super::TargetChange::all_connected())
        .await;
    assert!(matches!(
        fixture
            .state
            .device_control()
            .disable_device(&admission(fixture.state.device_control()), missing)
            .await,
        Err(DeviceError::DeviceNotFound)
    ));
    assert!(
        fixture
            .state
            .device_control()
            .registry
            .get(missing)
            .await
            .is_none()
    );
    assert!(matches!(
        fixture
            .state
            .device_control()
            .revoke_device(&admission(fixture.state.device_control()), missing)
            .await,
        Err(DeviceError::DeviceNotFound)
    ));
    assert!(
        fixture
            .state
            .device_control()
            .registry
            .get(missing)
            .await
            .is_none()
    );
}

#[tokio::test]
async fn approval_uses_current_authority_instead_of_review_creation_state() {
    let fixture = Fixture::new().await;
    let machine_hardware_id = MachineHardwareId::parse(MACHINE_HARDWARE_ID)
        .unwrap_or_else(|| panic!("the fixture Machine Hardware ID is valid"));
    let first_evidence = ValidatedEnrollmentEvidence::new(
        machine_hardware_id,
        ControlPublicKey::parse(
            &SigningKey::from_bytes(&[0x6d; 32])
                .verifying_key()
                .to_bytes(),
        )
        .unwrap_or_else(|| panic!("the first control key is valid")),
        EvidenceQuality::Strong,
        "2.0.0".to_owned(),
        "2.0.0".to_owned(),
    );
    let second_evidence = ValidatedEnrollmentEvidence::new(
        machine_hardware_id,
        ControlPublicKey::parse(
            &SigningKey::from_bytes(&[0x6e; 32])
                .verifying_key()
                .to_bytes(),
        )
        .unwrap_or_else(|| panic!("the second control key is valid")),
        EvidenceQuality::Strong,
        "2.0.0".to_owned(),
        "2.0.0".to_owned(),
    );
    let EnrollmentStartOutcome::Pending(first_review, first_activation) = fixture
        .state
        .device()
        .start_enrollment(first_evidence)
        .await
        .unwrap_or_else(|error| panic!("first Enrollment start failed: {error:?}"))
    else {
        panic!("the first authority unexpectedly replayed");
    };
    let EnrollmentStartOutcome::Pending(second_review, second_activation) = fixture
        .state
        .device()
        .start_enrollment(second_evidence)
        .await
        .unwrap_or_else(|error| panic!("second Enrollment start failed: {error:?}"))
    else {
        panic!("the second authority unexpectedly replayed");
    };
    let first_authority = fixture
        .state
        .device_control()
        .approve_enrollment(
            &admission(fixture.state.device_control()),
            first_review.review_id(),
        )
        .await
        .unwrap_or_else(|error| panic!("first approval failed: {error:?}"));
    assert!(
        matches!(first_activation.await, Ok(Ok(actual)) if actual == EnrollmentReviewDecision::Activated(first_authority))
    );

    let device_id = first_authority.device_id();
    let (outbound, mut outgoing) = mpsc::channel(1);
    let (session_id, handle) = replace_current_lease(&fixture.state, device_id, outbound).await;

    let gate = handle.authority_fence.lock().await;
    let state = Arc::clone(&fixture.state);
    let approval = tokio::spawn(async move {
        state
            .device_control()
            .approve_enrollment(
                &admission(state.device_control()),
                second_review.review_id(),
            )
            .await
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(
        handle
            .enqueue_client_state(session_id, valid_snapshot())
            .await
            .is_ok()
    );
    drop(gate);

    let authority = approval
        .await
        .unwrap_or_else(|error| panic!("approval task failed: {error}"))
        .unwrap_or_else(|error| panic!("replacement approval failed: {error:?}"));
    assert_eq!(authority.device_id(), device_id);
    assert_eq!(fixture.component_row_counts().await, [0, 0, 0, 0]);
    assert!(outgoing.recv().await.is_none());
    assert!(
        matches!(second_activation.await, Ok(Ok(actual)) if actual == EnrollmentReviewDecision::Activated(authority))
    );
}

#[tokio::test]
async fn replacement_attach_clears_actual_and_old_session_cannot_restore_it() {
    let fixture = Fixture::new().await;
    let device_id = fixture.activate(&SigningKey::from_bytes(&[0x65; 32])).await;
    let (first_outbound, mut first_outgoing) = mpsc::channel(1);
    let (first_session, first_handle) =
        replace_current_lease(&fixture.state, device_id, first_outbound).await;
    assert!(
        first_handle
            .enqueue_client_state(first_session, valid_snapshot())
            .await
            .is_ok()
    );
    let _target = first_outgoing
        .recv()
        .await
        .unwrap_or_else(|| panic!("the initial lease closed unexpectedly"));
    assert!(matches!(
        fixture
            .state
            .device_control()
            .registry
            .read_connection_state(device_id)
            .await,
        DeviceConnectionState::Active { .. }
    ));

    let (replacement_outbound, _replacement_outgoing) = mpsc::channel(1);
    let (_replacement_session, _) =
        replace_current_lease(&fixture.state, device_id, replacement_outbound).await;
    assert!(first_outgoing.recv().await.is_none());
    assert!(
        first_handle
            .enqueue_client_state(first_session, valid_snapshot())
            .await
            .is_ok()
    );
    assert!(matches!(
        fixture
            .state
            .device_control()
            .registry
            .read_connection_state(device_id)
            .await,
        DeviceConnectionState::AwaitingFreshState
    ));
}

#[tokio::test]
async fn authority_change_after_precheck_is_rejected_after_attach() {
    let fixture = Fixture::new().await;
    let machine_hardware_id = MachineHardwareId::parse(MACHINE_HARDWARE_ID)
        .unwrap_or_else(|| panic!("the fixture Machine Hardware ID is valid"));
    let device_id = fixture.activate(&SigningKey::from_bytes(&[0x66; 32])).await;
    let authority = fixture
        .state
        .device()
        .find_current_authority(machine_hardware_id)
        .await
        .unwrap_or_else(|error| panic!("authority lookup failed: {error:?}"))
        .unwrap_or_else(|| panic!("the fixture authority was absent"));

    let (old_outbound, mut old_outgoing) = mpsc::channel(1);
    let (_, handle) = replace_current_lease(&fixture.state, device_id, old_outbound).await;
    let gate = handle.authority_fence.lock().await;
    let prior_weak_count = Arc::weak_count(fixture.state.device_control());
    let state = Arc::clone(&fixture.state);
    let (outbound, mut outgoing) = mpsc::channel(1);
    let attach = tokio::spawn(async move {
        state
            .device_control()
            .attach_device_lease(
                &admission(state.device_control()),
                machine_hardware_id,
                authority,
                outbound,
            )
            .await
            .unwrap_or_else(|error| panic!("attach admission: {error}"))
    });
    // Only a successful precheck creates the new lease's Weak. The held fence
    // keeps the actor from completing replacement while authority is changed.
    timeout(Duration::from_secs(5), async {
        while Arc::weak_count(fixture.state.device_control()) == prior_weak_count {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("attach did not pass its authority precheck"));
    fixture
        .state
        .device()
        .disable(device_id)
        .await
        .unwrap_or_else(|error| panic!("Device disable failed: {error:?}"));
    drop(gate);
    assert!(
        timeout(Duration::from_secs(5), attach)
            .await
            .unwrap_or_else(|_| panic!("attach did not finish"))
            .unwrap_or_else(|error| panic!("attach task failed: {error}"))
            .is_none()
    );
    assert!(old_outgoing.recv().await.is_none());
    assert!(outgoing.recv().await.is_none());
    assert!(matches!(
        fixture
            .state
            .device_control()
            .registry
            .read_connection_state(device_id)
            .await,
        DeviceConnectionState::Offline
    ));
}

#[tokio::test]
async fn failed_authority_commits_leave_the_current_lease_unfenced() {
    let fixture = Fixture::new().await;
    let device_id = fixture.activate(&SigningKey::from_bytes(&[0x76; 32])).await;
    let (outbound, mut outgoing) = mpsc::channel(1);
    let (session_id, handle) = replace_current_lease(&fixture.state, device_id, outbound).await;
    fixture
        .database
        .write(|transaction| {
            transaction
                .connection()
                .batch_execute(
                    "CREATE TRIGGER reject_lifecycle BEFORE UPDATE OF state ON devices
             BEGIN SELECT RAISE(ABORT, 'fixture commit rejected'); END;",
                )
                .map_err(PersistenceError::from)
        })
        .await
        .unwrap_or_else(|error| panic!("failure fixture: {error:?}"));

    assert!(matches!(
        fixture
            .state
            .device_control()
            .disable_device(&admission(fixture.state.device_control()), device_id)
            .await,
        Err(DeviceError::PersistenceFailed { .. })
    ));
    assert!(matches!(
        fixture
            .state
            .device_control()
            .revoke_device(&admission(fixture.state.device_control()), device_id)
            .await,
        Err(DeviceError::PersistenceFailed { .. })
    ));
    assert!(!*handle.authority_fence.lock().await);
    assert!(!outgoing.is_closed());
    assert!(
        handle
            .enqueue_client_state(session_id, valid_snapshot())
            .await
            .is_ok()
    );
    let snapshot = timeout(Duration::from_secs(5), outgoing.recv())
        .await
        .unwrap_or_else(|_| panic!("unfenced lease did not reconcile"))
        .unwrap_or_else(|| panic!("failed commit evicted the current lease"));
    assert_complete_target(snapshot);
}

#[tokio::test]
async fn approval_coordinator_evicts_the_old_lease_and_notifies() {
    let fixture = Fixture::new().await;
    let device_id = fixture.activate(&SigningKey::from_bytes(&[0x67; 32])).await;
    let (outbound, mut outgoing) = mpsc::channel(1);
    let (session_id, handle) = replace_current_lease(&fixture.state, device_id, outbound).await;
    assert!(
        handle
            .enqueue_client_state(session_id, valid_snapshot())
            .await
            .is_ok()
    );
    let _target = outgoing
        .recv()
        .await
        .unwrap_or_else(|| panic!("the old lease closed unexpectedly"));

    let replacement = ValidatedEnrollmentEvidence::new(
        MachineHardwareId::parse(MACHINE_HARDWARE_ID)
            .unwrap_or_else(|| panic!("the fixture Machine Hardware ID is valid")),
        ControlPublicKey::parse(
            &SigningKey::from_bytes(&[0x68; 32])
                .verifying_key()
                .to_bytes(),
        )
        .unwrap_or_else(|| panic!("the replacement control key is valid")),
        EvidenceQuality::Strong,
        "2.0.0".to_owned(),
        "2.0.0".to_owned(),
    );
    let EnrollmentStartOutcome::Pending(review, mut activation) = fixture
        .state
        .device()
        .start_enrollment(replacement)
        .await
        .unwrap_or_else(|error| panic!("replacement Enrollment start failed: {error:?}"))
    else {
        panic!("the replacement authority unexpectedly replayed");
    };
    assert!(!outgoing.is_closed());
    assert!(matches!(
        activation.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));

    let authority = fixture
        .state
        .device_control()
        .approve_enrollment(
            &admission(fixture.state.device_control()),
            review.review_id(),
        )
        .await;
    let authority =
        authority.unwrap_or_else(|error| panic!("replacement approval failed: {error:?}"));
    assert_eq!(authority.device_id(), device_id);
    assert!(outgoing.recv().await.is_none());
    assert!(
        matches!(activation.await, Ok(Ok(actual)) if actual == EnrollmentReviewDecision::Activated(authority))
    );
}

#[tokio::test]
async fn registry_accepts_six_hundred_devices() {
    let registry = DeviceRegistry::new();
    let mut sessions = HashSet::new();
    let mut device_ids = Vec::with_capacity(601);
    let mut outgoing = Vec::with_capacity(600);

    timeout(Duration::from_secs(10), async {
        for _ in 0..600 {
            let device_id = DeviceId::parse(&Uuid::now_v7().hyphenated().to_string())
                .unwrap_or_else(|| panic!("a generated UUIDv7 was not a Device ID"));
            let (outbound, receiver) = mpsc::channel(1);
            let handle = registry
                .get_or_spawn(device_id)
                .await
                .unwrap_or_else(|| panic!("registry closed"));
            let session_id = handle
                .replace_current_lease(std::sync::Weak::new(), outbound)
                .await
                .unwrap_or_else(|| panic!("the actor rejected a lease replacement"));
            assert!(sessions.insert(session_id));
            device_ids.push(device_id);
            outgoing.push(receiver);
        }
    })
    .await
    .unwrap_or_else(|_| panic!("the registry did not accept 600 Devices in time"));
    assert_eq!(sessions.len(), 600);

    let unseen = DeviceId::parse(&Uuid::now_v7().hyphenated().to_string())
        .unwrap_or_else(|| panic!("the unseen fixture ID was invalid"));
    device_ids.push(unseen);
    let states = timeout(
        Duration::from_secs(10),
        registry.read_connection_states(&device_ids),
    )
    .await
    .unwrap_or_else(|_| panic!("the registry did not read 600 states concurrently in time"));
    assert_eq!(states.len(), 601);
    assert!(matches!(
        states.get(&unseen),
        Some(DeviceConnectionState::Offline)
    ));
    assert!(device_ids[..600].iter().all(|device_id| matches!(
        states.get(device_id),
        Some(DeviceConnectionState::AwaitingFreshState)
    )));
}

#[tokio::test]
async fn batch_and_single_active_device_status_have_the_same_convergence() {
    let fixture = Fixture::new().await;
    let device_id = fixture.activate(&SigningKey::from_bytes(&[0x70; 32])).await;
    fixture
        .state
        .session()
        .set_foreground(device_id, ForegroundTarget::Waiting)
        .await
        .unwrap_or_else(|error| panic!("Session target setup failed: {error}"));
    fixture
        .state
        .session()
        .terminate(device_id)
        .await
        .unwrap_or_else(|error| panic!("Session terminate setup failed: {error}"));
    fixture
        .state
        .home()
        .reset(device_id)
        .await
        .unwrap_or_else(|error| panic!("Home target setup failed: {error}"));
    let (outbound, mut outgoing) = mpsc::channel(1);
    let (session_id, handle) = replace_current_lease(&fixture.state, device_id, outbound).await;
    assert!(
        handle
            .enqueue_client_state(session_id, valid_snapshot())
            .await
            .is_ok()
    );
    timeout(Duration::from_secs(5), outgoing.recv())
        .await
        .unwrap_or_else(|_| panic!("the initial target was not emitted"))
        .unwrap_or_else(|| panic!("the active lease closed unexpectedly"));

    let single = fixture
        .state
        .device_control()
        .read_device_status(device_id)
        .await
        .unwrap_or_else(|_| panic!("single Device status failed"))
        .unwrap_or_else(|| panic!("single Device status was absent"));
    let single_convergence = single.convergence;
    let mut batch = fixture
        .state
        .device_control()
        .read_device_statuses(crate::component::device::DeviceListFilter::All)
        .await
        .unwrap_or_else(|_| panic!("batch Device status failed"));
    assert_eq!(batch.len(), 1);
    let batch_status = batch
        .pop()
        .unwrap_or_else(|| panic!("batch Device status was absent"));
    let device = batch_status.device;
    let batch_convergence = batch_status.convergence;
    assert_eq!(device.device_id(), device_id);
    assert!(single_convergence == batch_convergence);
}

#[tokio::test]
async fn closed_window_allows_manual_enrollment_and_a_complete_target() {
    let fixture = Fixture::new().await;
    let (mut socket, server) = connect(&fixture).await;
    let session_id = enroll(&fixture, &mut socket).await;

    socket
        .send(ClientMessage::Ping(session_id.clone().into()))
        .await
        .unwrap_or_else(|error| panic!("heartbeat send failed: {error}"));
    match timeout(Duration::from_secs(5), socket.next()).await {
        Ok(Some(Ok(ClientMessage::Pong(payload)))) if payload.as_ref() == session_id.as_slice() => {
        }
        result => panic!("Server did not answer the active heartbeat: {result:?}"),
    }

    socket
        .send(ClientMessage::Binary(
            ClientActiveEnvelope {
                session_id,
                body: Some(client_active_envelope::Body::ClientState(valid_snapshot())),
            }
            .encode_to_vec()
            .into(),
        ))
        .await
        .unwrap_or_else(|error| panic!("ClientState send failed: {error}"));
    let bytes = receive_binary(&mut socket).await;
    let active = ServerActiveEnvelope::decode(bytes.as_slice())
        .unwrap_or_else(|error| panic!("ServerActive decode failed: {error}"));
    assert_complete_target(active);

    let device_id = fixture
        .state
        .device()
        .find_current_authority(
            MachineHardwareId::parse(MACHINE_HARDWARE_ID)
                .unwrap_or_else(|| panic!("invalid machine")),
        )
        .await
        .unwrap_or_else(|error| panic!("authority: {error}"))
        .unwrap_or_else(|| panic!("missing authority"))
        .device_id();
    let network = fixture
        .state
        .device()
        .find_device(device_id)
        .await
        .unwrap_or_else(|error| panic!("Device: {error}"))
        .and_then(crate::component::device::DeviceProjection::network)
        .unwrap_or_else(|| panic!("missing addresses"));
    assert_eq!(network.server_observed_ip.to_string(), "127.0.0.1");
    assert_eq!(
        network.client_ip.map(|ip| ip.to_string()).as_deref(),
        Some("127.0.0.2")
    );

    drop(socket);
    server.abort();
}

#[tokio::test]
async fn open_window_automatically_enrolls_without_operator_approval() {
    let fixture = Fixture::new().await;
    fixture.state.provisioning().open_window();
    let (mut socket, server) = connect(&fixture).await;
    submit_enrollment_proof(&mut socket).await;
    let (_, session_id) = finish_enrollment(&mut socket).await;
    assert!(
        fixture
            .state
            .device()
            .pending_enrollment_reviews()
            .await
            .is_empty()
    );
    assert_eq!(
        fixture
            .state
            .device()
            .list_devices(crate::component::device::DeviceListFilter::All)
            .await
            .unwrap_or_else(|error| panic!("Device lookup failed: {error}"))
            .len(),
        1
    );
    send_active_heartbeat(&mut socket, &session_id).await;
    drop(socket);
    server.abort();
}

#[tokio::test]
async fn automatic_approval_wait_does_not_block_pending_heartbeats() {
    let fixture = Fixture::new().await;
    fixture.state.provisioning().open_window();
    let approval = fixture
        .state
        .device_control()
        .enrollment_approval
        .lock()
        .await;
    let (mut socket, server) = connect(&fixture).await;
    submit_enrollment_proof(&mut socket).await;
    socket
        .send(ClientMessage::Ping(Vec::new().into()))
        .await
        .unwrap_or_else(|error| panic!("pending heartbeat send failed: {error}"));
    match timeout(Duration::from_secs(5), socket.next()).await {
        Ok(Some(Ok(ClientMessage::Pong(payload)))) if payload.is_empty() => {}
        result => panic!("automatic approval blocked the pending heartbeat: {result:?}"),
    }
    drop(approval);
    let (_, session_id) = finish_enrollment(&mut socket).await;
    send_active_heartbeat(&mut socket, &session_id).await;
    drop(socket);
    server.abort();
}

#[tokio::test]
async fn opening_window_approves_an_existing_review_on_the_same_connection() {
    let fixture = Fixture::new().await;
    let (mut socket, server) = connect(&fixture).await;
    submit_enrollment_proof(&mut socket).await;
    assert_eq!(
        fixture
            .state
            .device()
            .pending_enrollment_reviews()
            .await
            .len(),
        1
    );
    assert!(
        fixture
            .state
            .device()
            .list_devices(crate::component::device::DeviceListFilter::All)
            .await
            .unwrap_or_else(|error| panic!("Device lookup failed: {error}"))
            .is_empty()
    );

    fixture.state.provisioning().open_window();
    let (_, session_id) = finish_enrollment(&mut socket).await;
    assert!(
        fixture
            .state
            .device()
            .pending_enrollment_reviews()
            .await
            .is_empty()
    );
    send_active_heartbeat(&mut socket, &session_id).await;
    drop(socket);
    server.abort();
}

#[tokio::test]
async fn closing_window_restores_manual_review_for_new_connections() {
    let fixture = Fixture::new().await;
    fixture.state.provisioning().open_window();
    fixture.state.provisioning().close_window();
    let (mut socket, server) = connect(&fixture).await;
    let session_id = enroll(&fixture, &mut socket).await;
    send_active_heartbeat(&mut socket, &session_id).await;
    drop(socket);
    server.abort();
}

#[tokio::test]
async fn automatic_key_replacement_evicts_the_old_lease_before_activation_delivery() {
    let fixture = Fixture::new().await;
    let device_id = fixture.activate(&SigningKey::from_bytes(&[0x72; 32])).await;
    let (outbound, mut outgoing) = mpsc::channel(1);
    let (_session_id, _handle) = replace_current_lease(&fixture.state, device_id, outbound).await;
    fixture.state.provisioning().open_window();
    let (mut socket, server) = connect(&fixture).await;
    submit_enrollment_proof(&mut socket).await;
    let (_, session_id) = finish_enrollment(&mut socket).await;
    assert!(matches!(
        outgoing.try_recv(),
        Err(mpsc::error::TryRecvError::Disconnected)
    ));
    let devices = fixture
        .state
        .device()
        .list_devices(crate::component::device::DeviceListFilter::All)
        .await
        .unwrap_or_else(|error| panic!("Device lookup failed: {error}"));
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].device_id(), device_id);
    send_active_heartbeat(&mut socket, &session_id).await;
    drop(socket);
    server.abort();
}

#[tokio::test(start_paused = true)]
async fn exact_active_heartbeat_refreshes_the_client_deadline() {
    let session_id = Uuid::now_v7();
    let deadline =
        tokio::time::sleep_until(tokio::time::Instant::now() + super::CLIENT_SILENCE_TIMEOUT);
    tokio::pin!(deadline);
    tokio::time::advance(Duration::from_secs(1)).await;

    assert!(super::refresh_active_client_deadline(
        deadline.as_mut(),
        session_id.as_bytes(),
        &session_id,
    ));
    assert_eq!(
        deadline.deadline(),
        tokio::time::Instant::now() + super::CLIENT_SILENCE_TIMEOUT
    );
    let refreshed = deadline.deadline();
    assert!(!super::refresh_active_client_deadline(
        deadline.as_mut(),
        &[0x52; 16],
        &session_id,
    ));
    assert_eq!(deadline.deadline(), refreshed);
}

#[tokio::test]
async fn silent_active_connection_becomes_offline_after_the_client_deadline() {
    let fixture = Fixture::new().await;
    let (mut socket, server) = connect(&fixture).await;
    let session_id = enroll(&fixture, &mut socket).await;
    send_active_heartbeat(&mut socket, &session_id).await;
    let machine_hardware_id = MachineHardwareId::parse(MACHINE_HARDWARE_ID)
        .unwrap_or_else(|| panic!("the fixture Machine Hardware ID is valid"));
    let device_id = fixture
        .state
        .device()
        .find_current_authority(machine_hardware_id)
        .await
        .unwrap_or_else(|error| panic!("authority lookup failed: {error:?}"))
        .unwrap_or_else(|| panic!("the enrolled authority is absent"))
        .device_id();

    tokio::time::pause();
    tokio::time::advance(super::CLIENT_SILENCE_TIMEOUT + Duration::from_secs(1)).await;
    for _ in 0..10 {
        if matches!(
            fixture
                .state
                .device_control()
                .registry
                .read_connection_state(device_id)
                .await,
            DeviceConnectionState::Offline
        ) {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(matches!(
        fixture
            .state
            .device_control()
            .registry
            .read_connection_state(device_id)
            .await,
        DeviceConnectionState::Offline
    ));

    drop(socket);
    server.abort();
}

#[tokio::test]
async fn full_actor_mailbox_cannot_starve_the_client_deadline() {
    let fixture = Fixture::new().await;
    let (mut socket, server) = connect(&fixture).await;
    let session_id = enroll(&fixture, &mut socket).await;
    let machine_hardware_id = MachineHardwareId::parse(MACHINE_HARDWARE_ID)
        .unwrap_or_else(|| panic!("the fixture Machine Hardware ID is valid"));
    let device_id = fixture
        .state
        .device()
        .find_current_authority(machine_hardware_id)
        .await
        .unwrap_or_else(|error| panic!("authority lookup failed: {error:?}"))
        .unwrap_or_else(|| panic!("the enrolled authority is absent"))
        .device_id();
    let handle = fixture
        .state
        .device_control()
        .registry
        .get(device_id)
        .await
        .unwrap_or_else(|| panic!("the enrolled Device actor is absent"));
    let gate = handle.authority_fence.lock().await;

    tokio::time::pause();
    for _ in 0..16 {
        socket
            .send(ClientMessage::Binary(
                ClientActiveEnvelope {
                    session_id: session_id.clone(),
                    body: Some(client_active_envelope::Body::ClientState(valid_snapshot())),
                }
                .encode_to_vec()
                .into(),
            ))
            .await
            .unwrap_or_else(|error| panic!("ClientState send failed: {error}"));
    }
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }

    tokio::time::advance(super::CLIENT_SILENCE_TIMEOUT + Duration::from_secs(1)).await;
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    tokio::time::resume();
    match timeout(Duration::from_secs(1), socket.next()).await {
        Ok(None | Some(Err(_) | Ok(ClientMessage::Close(_)))) => {}
        result => panic!("the saturated mailbox kept the silent connection open: {result:?}"),
    }

    drop(gate);
    server.abort();
}

#[tokio::test]
async fn enrollment_denial_is_delivered_to_the_originating_connection() {
    let fixture = Fixture::new().await;
    let (mut socket, server) = connect(&fixture).await;
    submit_enrollment_proof(&mut socket).await;

    let reviews = fixture.state.device().pending_enrollment_reviews().await;
    assert_eq!(reviews.len(), 1);
    assert!(
        fixture
            .state
            .device()
            .deny_enrollment_review(reviews[0].review_id())
            .await
    );
    let Some(server_handshake_envelope::Body::EnrollmentReviewStatus(status)) =
        receive_handshake(&mut socket).await.body
    else {
        panic!("Server did not deliver the Enrollment denial");
    };
    assert!(
        matches!(EnrollmentReviewState::try_from(status.state), Ok(actual) if actual == EnrollmentReviewState::Denied)
    );
    assert_eq!(status.error_code, "ENROLLMENT_DENIED");

    drop(socket);
    server.abort();
}

#[tokio::test]
async fn pending_enrollment_heartbeat_keeps_the_review_attached() {
    let fixture = Fixture::new().await;
    let (mut socket, server) = connect(&fixture).await;
    submit_enrollment_proof(&mut socket).await;

    let reviews = fixture.state.device().pending_enrollment_reviews().await;
    assert_eq!(reviews.len(), 1);
    socket
        .send(ClientMessage::Ping(Vec::new().into()))
        .await
        .unwrap_or_else(|error| panic!("pending heartbeat send failed: {error}"));
    match timeout(Duration::from_secs(5), socket.next()).await {
        Ok(Some(Ok(ClientMessage::Pong(payload)))) if payload.is_empty() => {}
        result => panic!("Server did not answer the pending heartbeat: {result:?}"),
    }

    let after_heartbeat = fixture.state.device().pending_enrollment_reviews().await;
    assert_eq!(after_heartbeat.len(), 1);
    assert_eq!(after_heartbeat[0].review_id(), reviews[0].review_id());

    drop(socket);
    server.abort();
}

#[tokio::test]
async fn silent_pending_enrollment_is_removed_after_the_connection_deadline() {
    let fixture = Fixture::new().await;
    let (mut socket, server) = connect(&fixture).await;
    submit_enrollment_proof(&mut socket).await;
    assert_eq!(
        fixture
            .state
            .device()
            .pending_enrollment_reviews()
            .await
            .len(),
        1
    );

    tokio::time::pause();
    tokio::time::advance(super::CLIENT_SILENCE_TIMEOUT + Duration::from_secs(1)).await;
    for _ in 0..10 {
        if fixture
            .state
            .device()
            .pending_enrollment_reviews()
            .await
            .is_empty()
        {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(
        fixture
            .state
            .device()
            .pending_enrollment_reviews()
            .await
            .is_empty()
    );

    drop(socket);
    server.abort();
}

#[tokio::test]
async fn handshake_capacity_rejects_before_upgrade_and_recovers_after_timeout() {
    let fixture = Fixture::new().await;
    let control = fixture.state.device_control();
    let held = Arc::clone(&control.handshakes)
        .acquire_many_owned(63)
        .await
        .unwrap_or_else(|error| panic!("handshake gate closed: {error}"));
    let (mut socket, server) = connect(&fixture).await;
    receive_handshake(&mut socket).await;
    assert_eq!(control.handshakes.available_permits(), 0);
    let request = repeat_connection_request(&socket);
    let failure = connect_async(request.clone())
        .await
        .err()
        .unwrap_or_else(|| panic!("overload upgraded"));
    let tokio_tungstenite::tungstenite::Error::Http(response) = failure else {
        panic!("expected HTTP rejection")
    };
    assert_eq!(response.status(), 503);
    assert_eq!(response.headers()["retry-after"], "1");
    tokio::time::pause();
    tokio::time::advance(super::HANDSHAKE_TIMEOUT + Duration::from_secs(1)).await;
    tokio::time::resume();
    assert_closed(&mut socket).await;
    assert_eq!(control.handshakes.available_permits(), 1);
    let (mut replacement, _) = connect_async(request)
        .await
        .unwrap_or_else(|error| panic!("capacity not restored: {error}"));
    receive_handshake(&mut replacement).await;
    drop(replacement);
    drop(held);
    server.abort();
}

#[tokio::test]
async fn full_manual_review_capacity_does_not_block_resume_or_existing_control() {
    let fixture = Fixture::new().await;
    let signing_key = SigningKey::from_bytes(&[0x72; 32]);
    fixture.activate(&signing_key).await;
    let control = fixture.state.device_control();
    let held = Arc::clone(&control.handshakes)
        .acquire_many_owned(63)
        .await
        .unwrap_or_else(|error| panic!("handshake gate closed: {error}"));
    let (mut pending, server) = connect(&fixture).await;
    submit_enrollment_proof(&mut pending).await;
    assert_eq!(control.handshakes.available_permits(), 1);
    let evidence = ValidatedEnrollmentEvidence::new(
        MachineHardwareId::parse(MACHINE_HARDWARE_ID).unwrap_or_else(|| panic!("fixture HWID")),
        ControlPublicKey::parse(&[0x61; 32]).unwrap_or_else(|| panic!("fixture key")),
        EvidenceQuality::Strong,
        "2.0.0".to_owned(),
        "2.0.0".to_owned(),
    );
    let mut reviews = Vec::new();
    for _ in 1..640 {
        reviews.push(
            fixture
                .state
                .device()
                .start_enrollment(evidence.clone())
                .await
                .unwrap_or_else(|error| panic!("review capacity too small: {error:?}")),
        );
    }
    assert!(matches!(
        fixture.state.device().start_enrollment(evidence).await,
        Err(crate::component::device::EnrollmentStartError::ReviewCapacityReached)
    ));
    let request = repeat_connection_request(&pending);
    let (mut resumed, _) = connect_async(request)
        .await
        .unwrap_or_else(|error| panic!("Resume upgrade failed: {error}"));
    let Some(server_handshake_envelope::Body::ServerChallenge(challenge)) =
        receive_handshake(&mut resumed).await.body
    else {
        panic!("missing challenge")
    };
    let proof = sign_client_proof(
        &signing_key,
        &challenge,
        ClientProof {
            client_ip: None,
            daemon_version: "2.0.0".to_owned(),
            agent_version: "2.0.0".to_owned(),
            machine_hardware_id: MACHINE_HARDWARE_ID.to_owned(),
            signature: Vec::new(),
            purpose: Some(client_proof::Purpose::Resume(
                natsume_workstation_protocol::generated::ResumeSession::default(),
            )),
        },
    );
    send_handshake(
        &mut resumed,
        ClientHandshakeEnvelope {
            body: Some(client_handshake_envelope::Body::ClientProof(proof)),
        },
    )
    .await;
    let Some(server_handshake_envelope::Body::SessionReady(ready)) =
        receive_handshake(&mut resumed).await.body
    else {
        panic!("Resume did not activate")
    };
    assert_eq!(control.handshakes.available_permits(), 1);
    // Saturate short handshakes as well; the established lease still answers.
    let last = control
        .try_reserve_handshake()
        .unwrap_or_else(|error| panic!("missing released slot: {error}"));
    send_active_heartbeat(&mut resumed, &ready.session_id).await;
    drop(last);
    drop(held);
    drop(pending);
    drop(resumed);
    drop(reviews);
    server.abort();
}

fn repeat_connection_request(
    socket: &TestSocket,
) -> tokio_tungstenite::tungstenite::http::Request<()> {
    let MaybeTlsStream::Plain(stream) = socket.get_ref() else {
        panic!("expected fixture TCP stream")
    };
    let address = stream
        .peer_addr()
        .unwrap_or_else(|error| panic!("peer address: {error}"));
    let mut request = format!("ws://{address}{CONTROL_ROUTE}")
        .into_client_request()
        .unwrap_or_else(|error| panic!("request build: {error}"));
    request.headers_mut().insert(
        "sec-websocket-protocol",
        CONTROL_SUBPROTOCOL
            .parse()
            .unwrap_or_else(|error| panic!("protocol header: {error}")),
    );
    request
}

async fn assert_closed(socket: &mut TestSocket) {
    assert!(matches!(
        timeout(Duration::from_secs(5), socket.next()).await,
        Ok(None | Some(Err(_) | Ok(ClientMessage::Close(_))))
    ));
}

async fn send_active_heartbeat(socket: &mut TestSocket, session_id: &[u8]) {
    socket
        .send(ClientMessage::Ping(session_id.to_vec().into()))
        .await
        .unwrap_or_else(|error| panic!("active heartbeat send failed: {error}"));
    match timeout(Duration::from_secs(5), socket.next()).await {
        Ok(Some(Ok(ClientMessage::Pong(payload)))) if payload.as_ref() == session_id => {}
        result => panic!("Server did not answer the active heartbeat: {result:?}"),
    }
}

async fn connect(fixture: &Fixture) -> (TestSocket, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap_or_else(|error| panic!("test listener failed: {error}"));
    let address = listener
        .local_addr()
        .unwrap_or_else(|error| panic!("test listener address failed: {error}"));
    let application = crate::http::router(
        Arc::clone(&fixture.state),
        std::path::Path::new("/natsume-wss-test-unused-web-root"),
    );
    let server = tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            application.into_make_service_with_connect_info::<crate::tls::PeerAddress>(),
        )
        .await;
    });

    let mut request = format!("ws://{address}{CONTROL_ROUTE}")
        .into_client_request()
        .unwrap_or_else(|error| panic!("WebSocket request build failed: {error}"));
    request.headers_mut().insert(
        "sec-websocket-protocol",
        CONTROL_SUBPROTOCOL
            .parse()
            .unwrap_or_else(|error| panic!("subprotocol header failed: {error}")),
    );
    let (socket, response) = connect_async(request)
        .await
        .unwrap_or_else(|error| panic!("WebSocket connect failed: {error}"));
    assert_eq!(response.status(), 101);
    assert_eq!(
        response
            .headers()
            .get("sec-websocket-protocol")
            .and_then(|value| value.to_str().ok()),
        Some(CONTROL_SUBPROTOCOL)
    );
    (socket, server)
}

async fn enroll(fixture: &Fixture, socket: &mut TestSocket) -> Vec<u8> {
    submit_enrollment_proof(socket).await;

    let reviews = fixture.state.device().pending_enrollment_reviews().await;
    assert_eq!(reviews.len(), 1);
    let authority = fixture
        .state
        .device_control()
        .approve_enrollment(
            &admission(fixture.state.device_control()),
            reviews[0].review_id(),
        )
        .await
        .unwrap_or_else(|error| panic!("Enrollment approval failed: {error:?}"));
    let (device_id, session_id) = finish_enrollment(socket).await;
    assert_eq!(device_id, authority.device_id());
    session_id
}

async fn finish_enrollment(socket: &mut TestSocket) -> (DeviceId, Vec<u8>) {
    let Some(server_handshake_envelope::Body::EnrollmentActivated(enrollment_authority)) =
        receive_handshake(socket).await.body
    else {
        panic!("Server did not deliver the committed Enrollment authority");
    };
    let device_id = DeviceId::parse(&enrollment_authority.device_id)
        .unwrap_or_else(|| panic!("Server delivered an invalid Device ID"));
    send_handshake(
        socket,
        ClientHandshakeEnvelope {
            body: Some(client_handshake_envelope::Body::EnrollmentReady(
                enrollment_authority,
            )),
        },
    )
    .await;
    let Some(server_handshake_envelope::Body::SessionReady(ready)) =
        receive_handshake(socket).await.body
    else {
        panic!("Server did not establish the active session");
    };
    assert_eq!(ready.session_id.len(), 16);
    (device_id, ready.session_id)
}

async fn submit_enrollment_proof(socket: &mut TestSocket) {
    let Some(server_handshake_envelope::Body::ServerChallenge(challenge)) =
        receive_handshake(socket).await.body
    else {
        panic!("Server did not begin with a challenge");
    };
    let signing_key = SigningKey::from_bytes(&[0x63; 32]);
    let proof = sign_client_proof(
        &signing_key,
        &challenge,
        ClientProof {
            client_ip: Some("127.0.0.2".to_owned()),
            daemon_version: "2.0.0".to_owned(),
            agent_version: "2.0.0".to_owned(),
            machine_hardware_id: MACHINE_HARDWARE_ID.to_owned(),
            signature: Vec::new(),
            purpose: Some(client_proof::Purpose::Enrollment(EnrollmentAttempt {
                candidate_public_key: signing_key.verifying_key().to_bytes().to_vec(),
                evidence_quality: EnrollmentEvidenceQuality::Strong.into(),
            })),
        },
    );
    send_handshake(
        socket,
        ClientHandshakeEnvelope {
            body: Some(client_handshake_envelope::Body::ClientProof(proof)),
        },
    )
    .await;
    assert!(matches!(
        receive_handshake(socket).await.body,
        Some(server_handshake_envelope::Body::EnrollmentReviewStatus(_))
    ));
}

async fn replace_current_lease(
    state: &Arc<ServerState>,
    device_id: DeviceId,
    outbound: mpsc::Sender<ServerActiveEnvelope>,
) -> (Uuid, DeviceHandle) {
    let control = state.device_control();
    let handle = control
        .registry
        .get_or_spawn(device_id)
        .await
        .unwrap_or_else(|| panic!("registry closed"));
    let session_id = handle
        .replace_current_lease(Arc::downgrade(control), outbound)
        .await
        .unwrap_or_else(|| panic!("the Device actor rejected a lease replacement"));
    (session_id, handle)
}

fn valid_snapshot() -> ClientStateSnapshot {
    ClientStateSnapshot {
        input: Some(ClientInputState {
            gateway_credential: None,
            binding: None,
        }),
        actual: Some(ActualState {
            gateway: Some(GatewayActualState {
                credential_id: None,
                state: GatewayState::Absent.into(),
                gateway_leaf_sha256: None,
            }),
            binding_access: Some(BindingAccessActualState {
                assignment_state: BindingArtifactState::Absent.into(),
                credential_state: BindingArtifactState::Absent.into(),
                context: None,
            }),
            runtime_config: Some(RuntimeConfigActualState {
                state: RuntimeConfigState::Absent.into(),
                applied_domjudge_origin: None,
            }),
            session_control: Some(SessionControlActualState {
                contest_ready: false,
                waiting_ready: false,
                foreground: natsume_workstation_protocol::generated::SessionForeground::Unknown
                    .into(),
                session_state: SessionState::None.into(),
                completed_terminate_epoch: None,
            }),
            home: Some(HomeActualState {
                state: HomeState::Steady.into(),
                completed_reset_epoch: None,
            }),
            power: Some(
                natsume_workstation_protocol::generated::PowerControlActualState {
                    state: natsume_workstation_protocol::generated::PowerState::Idle.into(),
                    attempted_shutdown_epoch: None,
                },
            ),
        }),
    }
}

fn assert_complete_target(envelope: ServerActiveEnvelope) {
    let Some(server_active_envelope::Body::ServerState(snapshot)) = envelope.body else {
        panic!("the Actor did not emit ServerState");
    };
    let intent = snapshot
        .intent
        .unwrap_or_else(|| panic!("Server intent was absent"));
    assert!(intent.gateway_credential.is_some());
    assert!(intent.binding.is_some());
    let target = snapshot
        .target
        .unwrap_or_else(|| panic!("concrete target was absent"));
    assert!(target.gateway.is_some());
    assert!(target.binding_access.is_some());
    assert!(target.runtime_config.is_some());
    assert!(target.session_control.is_some());
    assert!(target.home.is_some());
    assert!(target.power.is_some());
}

type TestSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

async fn send_handshake(socket: &mut TestSocket, envelope: ClientHandshakeEnvelope) {
    socket
        .send(ClientMessage::Binary(envelope.encode_to_vec().into()))
        .await
        .unwrap_or_else(|error| panic!("Client handshake send failed: {error}"));
}

async fn receive_handshake(socket: &mut TestSocket) -> ServerHandshakeEnvelope {
    let bytes = receive_binary(socket).await;
    ServerHandshakeEnvelope::decode(bytes.as_slice())
        .unwrap_or_else(|error| panic!("Server handshake decode failed: {error}"))
}

async fn receive_binary(socket: &mut TestSocket) -> Vec<u8> {
    match timeout(Duration::from_secs(5), socket.next()).await {
        Ok(Some(Ok(ClientMessage::Binary(bytes)))) => bytes.to_vec(),
        result => panic!("Server did not send a binary WebSocket message: {result:?}"),
    }
}

#[tokio::test]
async fn shutdown_closes_all_registry_leases_including_awaiting_actual_and_rejects_admission() {
    let fixture = Fixture::new().await;
    let first = fixture.activate(&SigningKey::from_bytes(&[0x31; 32])).await;
    let (outbound, mut active) = mpsc::channel(1);
    let (session, handle) = replace_current_lease(&fixture.state, first, outbound).await;
    handle
        .enqueue_client_state(session, valid_snapshot())
        .await
        .unwrap_or_else(|error| panic!("snapshot: {error}"));
    active
        .recv()
        .await
        .unwrap_or_else(|| panic!("initial target"));
    let second = DeviceId::parse("019a1000-0000-7000-8000-000000000002")
        .unwrap_or_else(|| panic!("device fixture"));
    let pending = fixture
        .state
        .device_control()
        .registry
        .get_or_spawn(second)
        .await
        .unwrap_or_else(|| panic!("registry closed"));
    let (outbound, mut awaiting) = mpsc::channel(1);
    pending
        .replace_current_lease(Arc::downgrade(fixture.state.device_control()), outbound)
        .await
        .unwrap_or_else(|| panic!("awaiting lease"));
    assert!(matches!(
        fixture
            .state
            .device_control()
            .registry
            .read_connection_state(second)
            .await,
        DeviceConnectionState::AwaitingFreshState
    ));
    let (mut pending_admission, server) = connect(&fixture).await;
    submit_enrollment_proof(&mut pending_admission).await;
    let accepted = admission(fixture.state.device_control());
    fixture.state.lifecycle().begin_shutdown();
    assert!(matches!(
        fixture.state.device_control().try_reserve_handshake(),
        Err(super::AdmissionUnavailable::ServerStopping)
    ));
    let machine =
        MachineHardwareId::parse(MACHINE_HARDWARE_ID).unwrap_or_else(|| panic!("machine"));
    let authority = fixture
        .state
        .device()
        .find_current_authority(machine)
        .await
        .unwrap_or_else(|error| panic!("authority: {error}"))
        .unwrap_or_else(|| panic!("authority missing"));
    timeout(
        Duration::from_secs(3),
        fixture.state.device_control().shutdown(),
    )
    .await
    .unwrap_or_else(|error| panic!("shutdown: {error}"));
    assert_closed(&mut pending_admission).await;
    server.abort();
    assert!(matches!(
        fixture
            .state
            .device_control()
            .attach_device_lease(&accepted, machine, authority, mpsc::channel(1).0)
            .await,
        Err(super::AdmissionUnavailable::ServerStopping)
    ));
    assert!(active.recv().await.is_none());
    assert!(awaiting.recv().await.is_none());
    assert!(
        fixture
            .state
            .device_control()
            .registry
            .get(first)
            .await
            .is_none()
    );
    assert!(
        fixture
            .state
            .device_control()
            .registry
            .get(second)
            .await
            .is_none()
    );
}

#[tokio::test]
async fn pending_enrollment_is_interrupted_by_shutdown_and_releases_its_admission()
-> Result<(), crate::test_error::TestError> {
    let fixture = Fixture::new().await;
    let (mut socket, server) = connect(&fixture).await;
    submit_enrollment_proof(&mut socket).await;
    fixture.state.lifecycle().begin_shutdown();
    assert!(matches!(
        connect_async(repeat_connection_request(&socket)).await,
        Err(tokio_tungstenite::tungstenite::Error::Http(response))
            if response.status() == axum::http::StatusCode::SERVICE_UNAVAILABLE
    ));
    timeout(Duration::from_secs(2), fixture.state.lifecycle().wait()).await?;
    assert_closed(&mut socket).await;
    fixture.state.device_control().shutdown().await;
    server.abort();
    Ok(())
}

#[tokio::test]
async fn stalled_proof_handshake_is_interrupted_and_releases_its_admission()
-> Result<(), crate::test_error::TestError> {
    let fixture = Fixture::new().await;
    let (mut socket, server) = connect(&fixture).await;
    assert!(matches!(
        receive_handshake(&mut socket).await.body,
        Some(server_handshake_envelope::Body::ServerChallenge(_))
    ));
    fixture.state.lifecycle().begin_shutdown();
    timeout(Duration::from_secs(2), fixture.state.lifecycle().wait()).await?;
    assert_closed(&mut socket).await;
    assert_eq!(
        fixture
            .state
            .device_control()
            .handshakes
            .available_permits(),
        super::MAX_HANDSHAKES
    );
    fixture.state.device_control().shutdown().await;
    server.abort();
    Ok(())
}

#[tokio::test]
async fn registry_drain_owns_an_attach_already_waiting_on_the_actor_fence()
-> Result<(), crate::test_error::TestError> {
    let fixture = Fixture::new().await;
    let device_id = fixture.activate(&SigningKey::from_bytes(&[0x41; 32])).await;
    let control = fixture.state.device_control();
    let accepted = admission(control);
    let machine = MachineHardwareId::parse(MACHINE_HARDWARE_ID)
        .ok_or(crate::test_error::TestError::missing("machine"))?;
    let authority = control
        .device
        .find_current_authority(machine)
        .await?
        .ok_or(crate::test_error::TestError::missing("authority"))?;
    let (outbound, _old_receiver) = mpsc::channel(1);
    let (_, handle) = replace_current_lease(&fixture.state, device_id, outbound).await;
    let gate = handle.authority_fence.lock().await;
    let prior_weak_count = Arc::weak_count(control);
    let (outbound, mut receiver) = mpsc::channel(1);
    let attached = control.attach_device_lease(&accepted, machine, authority, outbound);
    tokio::pin!(attached);
    assert!(futures_util::poll!(&mut attached).is_pending());
    timeout(Duration::from_secs(2), async {
        while Arc::weak_count(control) == prior_weak_count {
            assert!(futures_util::poll!(&mut attached).is_pending());
            tokio::task::yield_now().await;
        }
    })
    .await?;
    let stopped = control.shutdown();
    tokio::pin!(stopped);
    assert!(futures_util::poll!(&mut stopped).is_pending());
    assert!(timeout(Duration::from_secs(2), control.registry.is_closed()).await?);
    drop(gate);
    timeout(Duration::from_secs(2), stopped).await?;
    assert!(matches!(
        timeout(Duration::from_secs(2), attached).await?,
        Err(super::AdmissionUnavailable::ServerStopping)
    ));
    assert!(
        timeout(Duration::from_secs(2), receiver.recv())
            .await?
            .is_none()
    );
    assert!(control.registry.get(device_id).await.is_none());
    assert!(control.registry.get_or_spawn(device_id).await.is_none());
    Ok(())
}

#[tokio::test]
async fn stale_respawn_and_fence_lookup_cannot_reopen_a_drained_registry()
-> Result<(), crate::test_error::TestError> {
    let registry = DeviceRegistry::new();
    let device_id = DeviceId::parse(&Uuid::now_v7().to_string())
        .ok_or(crate::test_error::TestError::missing("device"))?;
    let (exit, exited) = tokio::sync::oneshot::channel();
    let (stale, supervisor) = super::actor::spawn_test_actor(&registry, device_id, async {
        let _ = exited.await;
    })
    .await
    .ok_or(crate::test_error::TestError::missing("actor"))?;
    let gate = stale.authority_fence.lock().await;
    assert!(exit.send(()).is_ok());
    wait_for_failed_actor(&stale).await;
    let replacement = registry.get_or_spawn(device_id);
    tokio::pin!(replacement);
    assert!(futures_util::poll!(&mut replacement).is_pending());
    let fenced = registry.lock_current_fence(device_id);
    tokio::pin!(fenced);
    assert!(futures_util::poll!(&mut fenced).is_pending());
    timeout(Duration::from_secs(2), registry.shutdown()).await?;
    drop(gate);
    assert!(
        timeout(Duration::from_secs(2), replacement)
            .await?
            .is_none()
    );
    assert!(timeout(Duration::from_secs(2), fenced).await?.is_none());
    supervisor.await;
    assert!(registry.get(device_id).await.is_none());
    assert!(registry.get_or_spawn(device_id).await.is_none());
    assert!(
        super::actor::spawn_test_actor(&registry, device_id, async {})
            .await
            .is_none()
    );
    Ok(())
}

#[tokio::test]
async fn closed_registry_cancels_mutations_before_business_commits()
-> Result<(), crate::test_error::TestError> {
    use crate::component::device::EnrollmentApprovalError;
    let fixture = Fixture::new().await;
    let device_id = fixture.activate(&SigningKey::from_bytes(&[0x42; 32])).await;
    let control = fixture.state.device_control();
    let accepted = admission(control);
    let machine = MachineHardwareId::parse(MACHINE_HARDWARE_ID)
        .ok_or(crate::test_error::TestError::missing("machine"))?;
    let authority = control.device.find_current_authority(machine).await?;
    let evidence = ValidatedEnrollmentEvidence::new(
        machine,
        ControlPublicKey::parse(
            &SigningKey::from_bytes(&[0x43; 32])
                .verifying_key()
                .to_bytes(),
        )
        .ok_or(crate::test_error::TestError::missing("key"))?,
        EvidenceQuality::Strong,
        "2.0.0".into(),
        "2.0.0".into(),
    );
    let EnrollmentStartOutcome::Pending(review, mut activation) =
        control.device.start_enrollment(evidence).await?
    else {
        return Err(crate::test_error::TestError::missing("review"));
    };
    control.registry.shutdown().await;
    assert!(matches!(
        control.disable_device(&accepted, device_id).await,
        Err(DeviceError::ServerStopping)
    ));
    assert!(matches!(
        control.revoke_device(&accepted, device_id).await,
        Err(DeviceError::ServerStopping)
    ));
    assert!(matches!(
        control
            .approve_enrollment(&accepted, review.review_id())
            .await,
        Err(EnrollmentApprovalError::ServerStopping)
    ));
    assert_eq!(
        control.device.find_current_authority(machine).await?,
        authority
    );
    assert!(
        matches!(control.device.find_device(device_id).await?, Some(device)
        if device.state() == crate::component::device::DeviceState::Enabled)
    );
    assert!(matches!(
        activation.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));
    assert!(
        control
            .device
            .pending_enrollment_reviews()
            .await
            .iter()
            .any(|pending| pending.review_id() == review.review_id())
    );
    Ok(())
}

#[tokio::test]
async fn saturated_journal_does_not_change_device_convergence() {
    let (component, _unconsumed) = crate::journal::JournalComponent::channel(1);
    let journal = component.emitter();
    let fixture = Fixture::with_journal(Some(component)).await;
    let first = fixture.activate(&SigningKey::from_bytes(&[0x32; 32])).await;
    for attempt in 0..=9 {
        journal.emit(
            crate::journal::event::Event::ReconcileRetry(
                crate::journal::event::payload::ReconcileRetry { attempt },
            ),
            crate::journal::JournalContext::default(),
        );
    }
    let (outbound, mut outgoing) = mpsc::channel(1);
    let (session, handle) = replace_current_lease(&fixture.state, first, outbound).await;
    handle
        .enqueue_client_state(session, valid_snapshot())
        .await
        .unwrap_or_else(|error| panic!("snapshot: {error}"));
    assert_complete_target(
        timeout(Duration::from_secs(2), outgoing.recv())
            .await
            .unwrap_or_else(|error| panic!("convergence: {error}"))
            .unwrap_or_else(|| panic!("lease")),
    );
    assert_eq!(fixture.state.journal().stats().categories.device.dropped, 9);
    fixture.state.device_control().shutdown().await;
}

#[tokio::test]
async fn exclusively_locked_journal_does_not_block_device_disconnect_and_reconnect()
-> Result<(), crate::test_error::TestError> {
    let root = tempfile::tempdir()?;
    let path = root.path().join("journal.db");
    let component = crate::journal::JournalComponent::start(
        path.clone(),
        crate::config::JournalConfig {
            min_free_bytes: 1,
            ..crate::config::JournalConfig::default()
        },
    );
    crate::journal::tests::ready(&component).await?;
    crate::journal::tests::fast_busy(&component).await?;
    let journal = component.emitter();
    let fixture = Fixture::with_journal(Some(component)).await;
    let first = fixture.activate(&SigningKey::from_bytes(&[0x33; 32])).await;
    let mut lock = diesel::sqlite::SqliteConnection::establish(
        path.to_str()
            .ok_or(crate::test_error::TestError::missing("path"))?,
    )?;
    lock.batch_execute("BEGIN EXCLUSIVE")?;
    journal.emit(
        crate::journal::event::Event::ReviewCreated(
            crate::journal::event::payload::ReviewCreated {},
        ),
        crate::journal::JournalContext::default(),
    );
    let (outbound, mut outgoing) = mpsc::channel(1);
    let (session, handle) = replace_current_lease(&fixture.state, first, outbound).await;
    assert!(
        handle
            .enqueue_client_state(session, valid_snapshot())
            .await
            .is_ok()
    );
    assert_complete_target(
        timeout(Duration::from_secs(2), outgoing.recv())
            .await?
            .ok_or(crate::test_error::TestError::missing("target"))?,
    );
    handle.clear_lease_if_current(session).await;
    let (outbound, mut outgoing) = mpsc::channel(1);
    let (session, handle) = replace_current_lease(&fixture.state, first, outbound).await;
    assert!(
        handle
            .enqueue_client_state(session, valid_snapshot())
            .await
            .is_ok()
    );
    assert_complete_target(
        timeout(Duration::from_secs(2), outgoing.recv())
            .await?
            .ok_or(crate::test_error::TestError::missing("reconnected target"))?,
    );
    timeout(Duration::from_secs(3), async {
        while fixture.state.journal().stats().write_failed == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    drop(lock);
    fixture.state.device_control().shutdown().await;
    fixture.state.journal().shutdown().await;
    Ok(())
}

struct Fixture {
    path: PathBuf,
    database: Database,
    state: Arc<ServerState>,
}

impl Fixture {
    async fn new() -> Self {
        Self::with_journal(None).await
    }

    async fn with_journal(journal: Option<crate::journal::JournalComponent>) -> Self {
        let path = std::env::temp_dir().join(format!(
            "natsume-device-control-test-{}.sqlite3",
            Uuid::now_v7()
        ));
        let database = Database::connect_and_migrate(&DatabaseConfig::new(&path, true))
            .await
            .unwrap_or_else(|error| panic!("test database creation failed: {error:?}"));
        database
            .write(|transaction| {
                diesel::insert_into(runtime_config::table)
                    .values((
                        runtime_config::singleton.eq(1),
                        runtime_config::domjudge_origin.eq(DOMJUDGE_ORIGIN),
                    ))
                    .execute(transaction.connection())
                    .map(|_| ())
                    .map_err(PersistenceError::from)
            })
            .await
            .unwrap_or_else(|error| panic!("Runtime Config fixture failed: {error:?}"));
        let state = Arc::new(
            match journal {
                None => server_state::tests::for_test(
                    database.clone(),
                    path.with_extension("print-documents"),
                ),
                Some(journal) => server_state::tests::for_test_with_journal(
                    database.clone(),
                    path.with_extension("print-documents"),
                    PathBuf::from("/nonexistent/natsume-test-logos"),
                    journal,
                ),
            }
            .unwrap_or_else(|error| panic!("ServerState fixture failed: {error}")),
        );
        Self {
            path,
            database,
            state,
        }
    }

    fn query_failures(&self) -> Arc<Notify> {
        let failures = Arc::new(Notify::new());
        for mut connection in self.database.test_exhaust_pool() {
            connection
                .batch_execute("PRAGMA busy_timeout = 0;")
                .unwrap_or_else(|error| panic!("{}", snafu::Report::from_error(error)));
            connection.set_instrumentation(QueryFailures(Arc::clone(&failures)));
        }
        failures
    }

    async fn activate(&self, signing_key: &SigningKey) -> DeviceId {
        let evidence = ValidatedEnrollmentEvidence::new(
            MachineHardwareId::parse(MACHINE_HARDWARE_ID)
                .unwrap_or_else(|| panic!("the fixture Machine Hardware ID is valid")),
            ControlPublicKey::parse(&signing_key.verifying_key().to_bytes())
                .unwrap_or_else(|| panic!("the fixture control key is valid")),
            EvidenceQuality::Strong,
            "2.0.0".to_owned(),
            "2.0.0".to_owned(),
        );
        let (review, activation) = match self
            .state
            .device()
            .start_enrollment(evidence)
            .await
            .unwrap_or_else(|error| panic!("Enrollment start failed: {error:?}"))
        {
            EnrollmentStartOutcome::Pending(review, activation) => (review, activation),
            EnrollmentStartOutcome::Replay(_) => panic!("a new fixture authority was replayed"),
        };
        let approval = self
            .state
            .device()
            .approve_enrollment(review.review_id())
            .await
            .unwrap_or_else(|error| panic!("Enrollment approval failed: {error:?}"));
        let authority = approval.authority();
        approval.complete();
        assert!(
            matches!(activation.await, Ok(Ok(actual)) if actual == super::EnrollmentReviewDecision::Activated(authority))
        );
        authority.device_id()
    }

    async fn component_row_counts(&self) -> [i64; 4] {
        self.database
            .read(|transaction| {
                Ok::<_, diesel::result::Error>([
                    gateway_credentials::table
                        .count()
                        .get_result(transaction.connection())?,
                    binding_negotiations::table
                        .count()
                        .get_result(transaction.connection())?,
                    device_session_targets::table
                        .count()
                        .get_result(transaction.connection())?,
                    device_home_targets::table
                        .count()
                        .get_result(transaction.connection())?,
                ])
            })
            .await
            .unwrap_or_else(|error| panic!("component row count failed: {error:?}"))
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _documents_result = fs::remove_dir_all(self.path.with_extension("print-documents"));
        for path in [
            self.path.clone(),
            PathBuf::from(format!("{}-wal", self.path.display())),
            PathBuf::from(format!("{}-shm", self.path.display())),
        ] {
            let _ = fs::remove_file(path);
        }
    }
}

#[tokio::test]
async fn old_or_mixed_control_protocols_are_rejected_before_handshake() {
    let fixture = Fixture::new().await;
    let (mut socket, server) = connect(&fixture).await;
    receive_handshake(&mut socket).await;
    for protocol in ["natsume.control", "natsume.control, natsume.control.v3"] {
        let mut request = repeat_connection_request(&socket);
        request.headers_mut().insert(
            "sec-websocket-protocol",
            protocol.parse().unwrap_or_else(|e| panic!("header: {e}")),
        );
        let error = connect_async(request)
            .await
            .err()
            .unwrap_or_else(|| panic!("old protocol was accepted"));
        let tokio_tungstenite::tungstenite::Error::Http(response) = error else {
            panic!("expected HTTP rejection");
        };
        assert_eq!(response.status(), 400);
        assert!(response.body().as_ref().is_some_and(|body| {
            String::from_utf8_lossy(body).contains("incompatible_device_control_protocol")
        }));
    }
    assert_eq!(
        fixture
            .state
            .device_control()
            .handshakes
            .available_permits(),
        63
    );
    server.abort();
}

#[tokio::test]
async fn automatic_approval_failure_is_reported_once_when_activation_wins() {
    use crate::{
        config::LogLevel,
        logging::tests::{CapturedLogs, SubscriberTestGuard},
    };
    let _guard = SubscriberTestGuard::acquire();
    let captured = CapturedLogs::default();
    let _subscriber = tracing::subscriber::set_default(captured.subscriber(LogLevel::Trace));
    let fixture = Fixture::new().await;
    Database::connect_and_migrate(&DatabaseConfig::new(&fixture.path, false)).await
        .unwrap_or_else(|error| panic!("{}", snafu::Report::from_error(error))).write(|tx| {
        tx.connection().batch_execute("CREATE TRIGGER reject_activation BEFORE INSERT ON devices BEGIN SELECT RAISE(ABORT, 'automatic activation evidence'); END;").map_err(PersistenceError::from)
    }).await.map_err(crate::db::TransactionError::into_error).unwrap_or_else(|error| panic!("{}", snafu::Report::from_error(error)));
    fixture.state.provisioning().open_window();
    let (mut socket, server) = connect(&fixture).await;
    submit_enrollment_proof(&mut socket).await;
    assert_closed(&mut socket).await;
    timeout(Duration::from_secs(2), async {
        loop {
            if captured
                .text()
                .unwrap_or_else(|()| panic!("logs"))
                .contains("Device automatic enrollment approval failed")
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("automatic failure report lost"));
    let logs = captured.text().unwrap_or_else(|()| panic!("logs"));
    assert_eq!(
        logs.matches("Device automatic enrollment approval failed")
            .count(),
        1
    );
    assert!(logs.contains("automatic activation evidence"));
    drop(socket);
    server.abort();
}
