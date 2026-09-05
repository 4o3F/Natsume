use std::{collections::HashSet, fs, path::PathBuf, sync::Arc, time::Duration};

use diesel::{ExpressionMethods, QueryDsl, RunQueryDsl, connection::SimpleConnection};
use ed25519_dalek::SigningKey;
use futures_util::{SinkExt as _, StreamExt as _};
use natsume_device_protocol::{
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
    sync::mpsc,
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
    component::session::LockState,
    db::{Database, DatabaseConfig, PersistenceError},
    diesel_schema::{
        binding_negotiations, device_home_targets, device_session_targets, gateway_credentials,
        runtime_config,
    },
    server_state::{self, ServerState},
};

use super::actor::{DeviceConnectionState, DeviceHandle, DeviceRegistry, TARGET_REFRESH_INTERVAL};

const MACHINE_HARDWARE_ID: &str = "a9aa9d04-3ece-5567-8260-910930ff5e03";
const DOMJUDGE_ORIGIN: &str = "https://domjudge.example.test";

#[tokio::test]
async fn coordinator_survives_its_composition_and_leases_do_not_keep_it_alive() {
    let fixture = Fixture::new().await;
    let device_id = fixture.activate(&SigningKey::from_bytes(&[0x77; 32])).await;
    // The fixture keeps database files alive, independently of this composition.
    let composition = Arc::new(
        server_state::tests::for_test(fixture.database.clone())
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
        .attach_device_lease(machine_hardware_id, authority, outbound)
        .await
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
    assert_eq!(fixture.component_row_counts().await, [1, 1, 1, 1]);
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
        .set_lock(device_id, LockState::Locked)
        .await
        .unwrap_or_else(|error| panic!("Session Control mutation failed: {error:?}"));
    fixture.state.device_control().dirty_device(device_id).await;

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
            .map(|target| target.lock_state),
        Some(natsume_device_protocol::generated::LockState::Locked.into())
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
        .set_lock(device_id, LockState::Locked)
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
            .map(|target| target.lock_state),
        Some(natsume_device_protocol::generated::LockState::Locked.into())
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
    let disable =
        tokio::spawn(async move { state.device_control().disable_device(device_id).await });
    tokio::task::yield_now().await;
    assert!(
        handle
            .enqueue_client_state(session_id, valid_snapshot())
            .await
            .is_ok()
    );
    drop(gate);

    assert_eq!(
        disable
            .await
            .unwrap_or_else(|error| panic!("disable task failed: {error}")),
        Ok(LifecycleOutcome::Changed)
    );
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
    let mut disable = Box::pin(fixture.state.device_control().disable_device(device_id));
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
    fixture.state.device_control().dirty_device(missing).await;
    fixture.state.device_control().dirty_all_devices().await;
    assert_eq!(
        fixture.state.device_control().disable_device(missing).await,
        Err(DeviceError::DeviceNotFound)
    );
    assert!(
        fixture
            .state
            .device_control()
            .registry
            .get(missing)
            .await
            .is_none()
    );
    assert_eq!(
        fixture.state.device_control().revoke_device(missing).await,
        Err(DeviceError::DeviceNotFound)
    );
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
    fixture.state.provisioning().open_window().await;
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
        .start_enrollment(fixture.state.provisioning(), first_evidence)
        .await
        .unwrap_or_else(|error| panic!("first Enrollment start failed: {error:?}"))
    else {
        panic!("the first authority unexpectedly replayed");
    };
    let EnrollmentStartOutcome::Pending(second_review, second_activation) = fixture
        .state
        .device()
        .start_enrollment(fixture.state.provisioning(), second_evidence)
        .await
        .unwrap_or_else(|error| panic!("second Enrollment start failed: {error:?}"))
    else {
        panic!("the second authority unexpectedly replayed");
    };
    let first_authority = fixture
        .state
        .device_control()
        .approve_enrollment(first_review.review_id())
        .await
        .unwrap_or_else(|error| panic!("first approval failed: {error:?}"));
    assert_eq!(
        first_activation.await,
        Ok(Ok(EnrollmentReviewDecision::Activated(first_authority)))
    );

    let device_id = first_authority.device_id();
    let (outbound, mut outgoing) = mpsc::channel(1);
    let (session_id, handle) = replace_current_lease(&fixture.state, device_id, outbound).await;

    let gate = handle.authority_fence.lock().await;
    let state = Arc::clone(&fixture.state);
    let approval = tokio::spawn(async move {
        state
            .device_control()
            .approve_enrollment(second_review.review_id())
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
    assert_eq!(
        second_activation.await,
        Ok(Ok(EnrollmentReviewDecision::Activated(authority)))
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
            .attach_device_lease(machine_hardware_id, authority, outbound)
            .await
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
                .map_err(|_| PersistenceError::OperationFailed)
        })
        .await
        .unwrap_or_else(|error| panic!("failure fixture: {error:?}"));

    assert_eq!(
        fixture
            .state
            .device_control()
            .disable_device(device_id)
            .await,
        Err(DeviceError::PersistenceFailed)
    );
    assert_eq!(
        fixture
            .state
            .device_control()
            .revoke_device(device_id)
            .await,
        Err(DeviceError::PersistenceFailed)
    );
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
        .start_enrollment(fixture.state.provisioning(), replacement)
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
        .approve_enrollment(review.review_id())
        .await;
    let authority =
        authority.unwrap_or_else(|error| panic!("replacement approval failed: {error:?}"));
    assert_eq!(authority.device_id(), device_id);
    assert!(outgoing.recv().await.is_none());
    assert_eq!(
        activation.await,
        Ok(Ok(EnrollmentReviewDecision::Activated(authority)))
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
            let handle = registry.get_or_spawn(device_id).await;
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
        .set_lock(device_id, LockState::Locked)
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
        .read_all_device_statuses()
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
async fn production_websocket_delivers_enrollment_and_a_complete_target() {
    let fixture = Fixture::new().await;
    fixture.state.provisioning().open_window().await;
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
    fixture.state.provisioning().open_window().await;
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
    fixture.state.provisioning().open_window().await;
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
    fixture.state.provisioning().open_window().await;
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
    assert_eq!(
        EnrollmentReviewState::try_from(status.state),
        Ok(EnrollmentReviewState::Denied)
    );
    assert_eq!(status.error_code, "ENROLLMENT_DENIED");

    drop(socket);
    server.abort();
}

#[tokio::test]
async fn pending_enrollment_heartbeat_keeps_the_review_attached() {
    let fixture = Fixture::new().await;
    fixture.state.provisioning().open_window().await;
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
    fixture.state.provisioning().open_window().await;
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
        let _ = axum::serve(listener, application).await;
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
        .approve_enrollment(reviews[0].review_id())
        .await
        .unwrap_or_else(|error| panic!("Enrollment approval failed: {error:?}"));
    let Some(server_handshake_envelope::Body::EnrollmentActivated(enrollment_authority)) =
        receive_handshake(socket).await.body
    else {
        panic!("Server did not deliver the committed Enrollment authority");
    };
    assert_eq!(
        enrollment_authority.device_id,
        authority.device_id().as_text()
    );
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
    ready.session_id
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
    let handle = control.registry.get_or_spawn(device_id).await;
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
                session_state: SessionState::None.into(),
                completed_terminate_epoch: None,
            }),
            home: Some(HomeActualState {
                state: HomeState::Steady.into(),
                completed_reset_epoch: None,
            }),
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

struct Fixture {
    path: PathBuf,
    database: Database,
    state: Arc<ServerState>,
}

impl Fixture {
    async fn new() -> Self {
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
                    .map_err(|_| PersistenceError::OperationFailed)
            })
            .await
            .unwrap_or_else(|error| panic!("Runtime Config fixture failed: {error:?}"));
        let state = Arc::new(
            server_state::tests::for_test(database.clone())
                .unwrap_or_else(|error| panic!("ServerState fixture failed: {error}")),
        );
        Self {
            path,
            database,
            state,
        }
    }

    async fn activate(&self, signing_key: &SigningKey) -> DeviceId {
        self.state.provisioning().open_window().await;
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
            .start_enrollment(self.state.provisioning(), evidence)
            .await
            .unwrap_or_else(|error| panic!("Enrollment start failed: {error:?}"))
        {
            EnrollmentStartOutcome::Pending(review, activation) => (review, activation),
            EnrollmentStartOutcome::Replay(_) => panic!("a new fixture authority was replayed"),
        };
        let approval = self
            .state
            .device()
            .approve_enrollment(self.state.provisioning(), review.review_id())
            .await
            .unwrap_or_else(|error| panic!("Enrollment approval failed: {error:?}"));
        let authority = approval.authority();
        approval.complete();
        assert_eq!(
            activation.await,
            Ok(Ok(super::EnrollmentReviewDecision::Activated(authority)))
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
        for path in [
            self.path.clone(),
            PathBuf::from(format!("{}-wal", self.path.display())),
            PathBuf::from(format!("{}-shm", self.path.display())),
        ] {
            let _ = fs::remove_file(path);
        }
    }
}
