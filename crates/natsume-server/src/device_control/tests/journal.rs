use crate::journal::JournalContext;
use std::net::IpAddr;

use super::*;
use crate::{
    journal::tests::{CapturedEvent, drain_events},
    journal::{
        Envelope, JournalComponent,
        event::{
            Event,
            payload::Disconnected,
            types::{
                ActualResource, ActualState as JournalActualState, DisconnectReason, OperatorId,
                ReviewCancelCause, SessionId,
            },
        },
    },
};

pub(super) struct RecordedFixture {
    /// Business fixture using a real process-local device coordinator.
    pub(super) fixture: Fixture,
    /// Bounded journal receiver retained only by the fixture, without a database writer.
    pub(super) receiver: mpsc::Receiver<Envelope>,
}

impl RecordedFixture {
    pub(super) async fn new() -> Self {
        let (journal, receiver) = JournalComponent::channel(1024);
        Self {
            fixture: Fixture::with_journal(Some(journal)).await,
            receiver,
        }
    }
}

fn disconnects(records: &[CapturedEvent], session: Uuid) -> Vec<&Disconnected> {
    records
        .iter()
        .filter_map(|record| match &record.event {
            Event::Disconnected(event) if event.session_id.value() == session => Some(event),
            _ => None,
        })
        .collect()
}

pub(super) fn assert_terminal(records: &[CapturedEvent], session: Uuid, reason: &DisconnectReason) {
    let terminal = disconnects(records, session);
    assert_eq!(
        terminal.len(),
        1,
        "exactly one terminal disconnect per lease"
    );
    assert_eq!(&terminal[0].reason, reason);
    let context = records.iter().find(|record| matches!(&record.event, Event::Disconnected(event) if event.session_id.value() == session)).unwrap_or_else(|| panic!("disconnect context"));
    assert_eq!(context.session_id, Some(SessionId::from_lease(session)));
    assert!(context.device_id.is_some());
}

#[derive(Clone, Copy)]
enum EndAttempt {
    Connection,
    Eviction,
    Shutdown,
    Replace,
}

async fn end_attempt(
    control: &Arc<super::super::DeviceControl>,
    handle: &DeviceHandle,
    device: DeviceId,
    session: Uuid,
    attempt: EndAttempt,
) {
    match attempt {
        EndAttempt::Connection => {
            handle
                .clear_lease_if_current(session, super::super::ConnectionEnd::ClientClose)
                .await;
            control.registry.read_connection_state(device).await;
        }
        EndAttempt::Eviction => {
            handle
                .evict_current_lease(LeaseEnd::local(DisconnectReason::ServerFault))
                .await;
        }
        EndAttempt::Shutdown => control.shutdown().await,
        EndAttempt::Replace => {
            handle
                .replace_current_lease(Arc::downgrade(control), mpsc::channel(1).0)
                .await
                .unwrap_or_else(|| panic!("replacement"));
        }
    }
}

#[tokio::test]
async fn committed_fence_reason_wins_over_connection_eviction_shutdown_and_replacement() {
    for kind in [
        FenceKind::Disabled,
        FenceKind::Revoked,
        FenceKind::ControlKeyReplaced,
    ] {
        for attempt in [
            EndAttempt::Connection,
            EndAttempt::Eviction,
            EndAttempt::Shutdown,
            EndAttempt::Replace,
        ] {
            let mut recorded = RecordedFixture::new().await;
            let fixture = &recorded.fixture;
            let device = fixture.activate(&SigningKey::from_bytes(&[0x51; 32])).await;
            let (outbound, mut outgoing) = mpsc::channel(1);
            let (session, handle) = replace_current_lease(&fixture.state, device, outbound).await;
            let operator = OperatorId::from_authenticated_operator(Uuid::now_v7());
            let mut fence = handle.authority_fence.lock().await;
            let ending = end_attempt(
                fixture.state.device_control(),
                &handle,
                device,
                session,
                attempt,
            );
            tokio::pin!(ending);
            assert!(futures_util::poll!(&mut ending).is_pending());
            *fence = Some(FenceReason {
                kind,
                context: JournalContext::default().with_actor(operator.clone()),
            });
            drop(fence);
            timeout(Duration::from_secs(2), ending)
                .await
                .unwrap_or_else(|error| panic!("terminal timeout: {error}"));
            assert!(outgoing.recv().await.is_none());
            let records = drain_events(&mut recorded.receiver);
            assert_terminal(&records, session, &kind.reason());
            let terminal = records
                .iter()
                .find(|record| matches!(record.event, Event::Disconnected(_)))
                .unwrap_or_else(|| panic!("terminal row"));
            assert_eq!(terminal.actor_operator_id, Some(operator));
            assert!(
                records
                    .iter()
                    .filter(|record| matches!(record.event, Event::LeaseAttached(_)))
                    .all(|record| record.actor_operator_id.is_none())
            );
        }
    }
}

#[tokio::test]
async fn first_actor_terminal_transition_wins_and_late_notifications_are_ignored() {
    let mut recorded = RecordedFixture::new().await;
    let fixture = &recorded.fixture;
    let device = fixture.activate(&SigningKey::from_bytes(&[0x52; 32])).await;
    let control = fixture.state.device_control();
    let (outbound, _outgoing) = mpsc::channel(1);
    let (session, handle) = replace_current_lease(&fixture.state, device, outbound).await;
    handle
        .evict_current_lease(LeaseEnd::local(DisconnectReason::Revoked))
        .await;
    handle
        .clear_lease_if_current(session, super::super::ConnectionEnd::ClientClose)
        .await;
    control.registry.read_connection_state(device).await;
    assert_terminal(
        &drain_events(&mut recorded.receiver),
        session,
        &DisconnectReason::Revoked,
    );
    let (outbound, _outgoing) = mpsc::channel(1);
    let (session, _) = replace_current_lease(&fixture.state, device, outbound).await;
    handle
        .clear_lease_if_current(session, super::super::ConnectionEnd::ClientClose)
        .await;
    control.registry.read_connection_state(device).await;
    control
        .disable_device(&admission(control), device, JournalContext::default())
        .await
        .unwrap_or_else(|error| panic!("disable: {error}"));
    control.shutdown().await;
    assert_terminal(
        &drain_events(&mut recorded.receiver),
        session,
        &DisconnectReason::ClientClose,
    );
}

#[tokio::test]
async fn actual_baseline_is_emitted_once_then_only_closed_state_changes_are_recorded() {
    let mut recorded = RecordedFixture::new().await;
    let fixture = &recorded.fixture;
    let device = fixture.activate(&SigningKey::from_bytes(&[0x53; 32])).await;
    let (outbound, mut outgoing) = mpsc::channel(1);
    let (session, handle) = replace_current_lease(&fixture.state, device, outbound).await;
    for _ in 0..2 {
        handle
            .enqueue_client_state(session, valid_snapshot())
            .await
            .unwrap_or_else(|error| panic!("snapshot: {error}"));
        assert_complete_target(outgoing.recv().await.unwrap_or_else(|| panic!("target")));
        fixture
            .state
            .device_control()
            .registry
            .read_connection_state(device)
            .await;
    }
    let baseline = drain_events(&mut recorded.receiver);
    let actual = baseline
        .iter()
        .filter(|record| matches!(record.event, Event::ActualChanged(_)))
        .collect::<Vec<_>>();
    assert_eq!(actual.len(), 7);
    assert!(actual.iter().all(
        |record| matches!(&record.event, Event::ActualChanged(event) if event.from.is_none())
    ));
    assert!(actual.iter().any(|record| matches!(&record.event, Event::ActualChanged(event) if event.resource == ActualResource::Foreground && event.to == JournalActualState::Unknown)));
    assert!(actual.iter().all(
        |record| record.session_id == Some(SessionId::from_lease(session))
            && record.actor_operator_id.is_none()
    ));
    let mut snapshot = valid_snapshot();
    snapshot
        .actual
        .as_mut()
        .unwrap_or_else(|| panic!("actual"))
        .runtime_config = Some(RuntimeConfigActualState {
        state: RuntimeConfigState::Applied.into(),
        applied_domjudge_origin: Some(DOMJUDGE_ORIGIN.into()),
    });
    handle
        .enqueue_client_state(session, snapshot.clone())
        .await
        .unwrap_or_else(|error| panic!("snapshot: {error}"));
    outgoing.recv().await.unwrap_or_else(|| panic!("target"));
    fixture
        .state
        .device_control()
        .registry
        .read_connection_state(device)
        .await;
    let changed = drain_events(&mut recorded.receiver);
    assert_eq!(changed.len(), 1);
    assert!(
        matches!(&changed[0].event, Event::ActualChanged(event) if event.resource == ActualResource::RuntimeConfig && event.from == Some(JournalActualState::Absent) && event.to == JournalActualState::Applied)
    );
    snapshot
        .actual
        .as_mut()
        .unwrap_or_else(|| panic!("actual"))
        .session_control
        .as_mut()
        .unwrap_or_else(|| panic!("session control"))
        .foreground = natsume_workstation_protocol::generated::SessionForeground::Waiting.into();
    handle
        .enqueue_client_state(session, snapshot)
        .await
        .unwrap_or_else(|error| panic!("snapshot: {error}"));
    outgoing.recv().await.unwrap_or_else(|| panic!("target"));
    fixture
        .state
        .device_control()
        .registry
        .read_connection_state(device)
        .await;
    let changed = drain_events(&mut recorded.receiver);
    assert_eq!(changed.len(), 1);
    assert!(
        matches!(&changed[0].event, Event::ActualChanged(event) if event.resource == ActualResource::Foreground && event.from == Some(JournalActualState::Unknown) && event.to == JournalActualState::Waiting)
    );
    fixture.state.device_control().shutdown().await;
}

#[tokio::test]
async fn lifecycle_changes_and_review_terminal_claims_emit_only_for_winners() {
    let mut recorded = RecordedFixture::new().await;
    let fixture = &recorded.fixture;
    let device = fixture.activate(&SigningKey::from_bytes(&[0x54; 32])).await;
    let control = fixture.state.device_control();
    let accepted = admission(control);
    assert_eq!(
        control
            .device
            .enable(device, JournalContext::default())
            .await
            .unwrap_or_else(|error| panic!("enable: {error}")),
        LifecycleOutcome::Unchanged
    );
    for _ in 0..2 {
        control
            .disable_device(&accepted, device, JournalContext::default())
            .await
            .unwrap_or_else(|error| panic!("disable: {error}"));
    }
    control
        .device
        .enable(device, JournalContext::default())
        .await
        .unwrap_or_else(|error| panic!("enable: {error}"));
    for _ in 0..2 {
        control
            .revoke_device(&accepted, device, JournalContext::default())
            .await
            .unwrap_or_else(|error| panic!("revoke: {error}"));
    }
    let records = drain_events(&mut recorded.receiver);
    assert_eq!(
        records
            .iter()
            .filter(|record| matches!(record.event, Event::LifecycleChanged(_)))
            .count(),
        3
    );
    let (mut socket, server) = connect(fixture).await;
    submit_enrollment_proof(&mut socket).await;
    let reviews = control.device.pending_enrollment_reviews().await;
    let review = reviews[0].review_id();
    assert!(
        control
            .device
            .remove_enrollment_review(
                review,
                ReviewCancelCause::ClientClose,
                JournalContext::default()
            )
            .await
    );
    assert!(
        !control
            .device
            .remove_enrollment_review(
                review,
                ReviewCancelCause::Timeout,
                JournalContext::default()
            )
            .await
    );
    assert!(
        !control
            .device
            .deny_enrollment_review(review, JournalContext::default())
            .await
    );
    assert_closed(&mut socket).await;
    let records = drain_events(&mut recorded.receiver);
    assert_eq!(
        records
            .iter()
            .filter(|record| matches!(record.event, Event::ReviewCreated(_)))
            .count(),
        1
    );
    assert_eq!(records.iter().filter(|record| matches!(&record.event, Event::ReviewCancelled(event) if event.cause == ReviewCancelCause::ClientClose)).count(), 1);
    let review_id = crate::journal::event::types::ReviewId::from_enrollment_review(review);
    assert!(records.iter().any(|record| matches!(&record.event, Event::ReviewCreated(event) if event.review_id == review_id && event.peer_ip == IpAddr::from([127, 0, 0, 1]))));
    assert!(records.iter().any(|record| matches!(&record.event, Event::ReviewCancelled(event) if event.review_id == review_id)));
    assert!(
        !records
            .iter()
            .any(|record| matches!(record.event, Event::EnrollmentDenied(_)))
    );
    server.abort();
    control.shutdown().await;
}

#[tokio::test]
async fn panicking_actor_reports_its_real_current_session_without_panic_contents() {
    let mut recorded = RecordedFixture::new().await;
    let fixture = &recorded.fixture;
    let device = fixture.activate(&SigningKey::from_bytes(&[0x58; 32])).await;
    let (trigger, triggered) = tokio::sync::oneshot::channel();
    let (_handle, supervisor) = super::super::actor::spawn_panicking_actor(
        &fixture.state.device_control().registry,
        device,
        triggered,
    )
    .await
    .unwrap_or_else(|| panic!("actor"));
    let (outbound, mut outgoing) = mpsc::channel(1);
    let (session, _handle) = replace_current_lease(&fixture.state, device, outbound).await;
    assert!(trigger.send(()).is_ok());
    supervisor.await;
    assert!(outgoing.recv().await.is_none());
    let records = drain_events(&mut recorded.receiver);
    let stopped = records.iter().filter(|record| matches!(&record.event, Event::ActorStopped(event) if event.panicked && event.session_id == Some(SessionId::from_lease(session)))).collect::<Vec<_>>();
    assert_eq!(stopped.len(), 1);
    assert_eq!(stopped[0].session_id, Some(SessionId::from_lease(session)));
    assert!(!stopped[0].details().contains("actor-session-secret-canary"));
    assert!(disconnects(&records, session).is_empty());
    assert!(
        fixture
            .state
            .device_control()
            .registry
            .get(device)
            .await
            .is_none()
    );
}

#[tokio::test]
async fn approval_commit_does_not_record_the_connection_activation_exchange() {
    let mut recorded = RecordedFixture::new().await;
    let control = recorded.fixture.state.device_control();
    let evidence = ValidatedEnrollmentEvidence::new(
        MachineHardwareId::parse(MACHINE_HARDWARE_ID).unwrap_or_else(|| panic!("machine ID")),
        ControlPublicKey::parse(
            &SigningKey::from_bytes(&[0x71; 32])
                .verifying_key()
                .to_bytes(),
        )
        .unwrap_or_else(|| panic!("control key")),
        EvidenceQuality::Strong,
        "2.0.0".to_owned(),
        "2.0.0".to_owned(),
    );
    let EnrollmentStartOutcome::Pending(review, activation) = control
        .device
        .start_enrollment(
            evidence,
            std::net::IpAddr::from([127, 0, 0, 1]),
            JournalContext::default(),
        )
        .await
        .unwrap_or_else(|error| panic!("review: {error}"))
    else {
        panic!("new enrollment replayed");
    };
    drain_events(&mut recorded.receiver);
    let authority = control
        .approve_enrollment(
            &admission(control),
            review.review_id(),
            JournalContext::default(),
        )
        .await
        .unwrap_or_else(|error| panic!("approval: {error}"));
    assert!(
        matches!(activation.await, Ok(Ok(EnrollmentReviewDecision::Activated(actual))) if actual == authority)
    );
    let records = drain_events(&mut recorded.receiver);
    assert_eq!(records.len(), 1);
    assert!(
        matches!(&records[0].event, Event::EnrollmentApproved(event) if event.review_id == crate::journal::event::types::ReviewId::from_enrollment_review(review.review_id()))
    );
    assert_eq!(records[0].device_id, Some(authority.device_id()));
    control.shutdown().await;
}

#[derive(Clone, Copy)]
pub(super) enum ExpectedApproval {
    Manual,
    Automatic,
}

pub(super) fn assert_admission_order(records: &[CapturedEvent], approval: ExpectedApproval) {
    let kinds = records
        .iter()
        .map(|record| record.event.kind())
        .collect::<Vec<_>>();
    let approved = kinds
        .iter()
        .position(|kind| *kind == "enrollment_approved")
        .unwrap_or_else(|| panic!("approval"));
    let committed = kinds
        .iter()
        .position(|kind| *kind == "activation_committed")
        .unwrap_or_else(|| panic!("activation"));
    let attached = kinds
        .iter()
        .position(|kind| *kind == "lease_attached")
        .unwrap_or_else(|| panic!("attach"));
    let completed = kinds
        .iter()
        .position(|kind| *kind == "admission_completed")
        .unwrap_or_else(|| panic!("ready"));
    assert!(approved < committed && committed < attached && attached < completed);
    assert_eq!(
        kinds
            .iter()
            .filter(|kind| **kind == "review_created")
            .count(),
        1
    );
    assert_eq!(
        kinds
            .iter()
            .filter(|kind| **kind == "activation_committed")
            .count(),
        1
    );
    assert!(records.iter().any(|record| matches!(&record.event, Event::EnrollmentApproved(event) if event.automatic == matches!(approval, ExpectedApproval::Automatic))));
    let review_id = records
        .iter()
        .find_map(|record| match &record.event {
            Event::ReviewCreated(event) => Some(event.review_id),
            _ => None,
        })
        .unwrap_or_else(|| panic!("created review"));
    assert!(records.iter().any(|record| matches!(&record.event, Event::EnrollmentApproved(event) if event.review_id == review_id)));
    assert!(
        !records
            .iter()
            .any(|record| matches!(record.event, Event::ReviewCancelled(_)))
    );
    for record in records {
        assert!(!record.details().contains(MACHINE_HARDWARE_ID));
    }
}

pub(super) fn retry_attempts(records: &[CapturedEvent]) -> Vec<u32> {
    records
        .iter()
        .filter_map(|record| match &record.event {
            Event::ReconcileRetry(event) => Some(event.attempt),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn narrow_live_lease_view_tracks_attach_clear_and_registry_shutdown() {
    let recorded = RecordedFixture::new().await;
    let fixture = &recorded.fixture;
    let device_id =
        DeviceId::parse(&Uuid::now_v7().to_string()).unwrap_or_else(|| panic!("device fixture id"));
    let control = fixture.state.device_control();
    let view = control.live_lease_view();
    assert_eq!(view.session(device_id).await, None);
    let (outbound, _outgoing) = mpsc::channel(8);
    let (session_id, handle) = replace_current_lease(&fixture.state, device_id, outbound).await;
    assert_eq!(
        view.session(device_id).await,
        Some(SessionId::from_lease(session_id))
    );
    handle
        .clear_lease_if_current(session_id, super::super::ConnectionEnd::ClientClose)
        .await;
    control.registry.read_connection_state(device_id).await;
    assert_eq!(view.session(device_id).await, None);
    let (outbound, _outgoing) = mpsc::channel(8);
    replace_current_lease(&fixture.state, device_id, outbound).await;
    control.shutdown().await;
    assert_eq!(view.session(device_id).await, None);
}

#[tokio::test]
async fn pending_session_ready_is_tracked_and_rejected_before_shutdown_disconnects() {
    let mut recorded = RecordedFixture::new().await;
    let fixture = &recorded.fixture;
    let device_id = fixture.activate(&SigningKey::from_bytes(&[0x61; 32])).await;
    let control = fixture.state.device_control();
    let (session_id, handle) =
        replace_current_lease(&fixture.state, device_id, mpsc::channel(1).0).await;
    let ready = super::super::finish_ready_exchange(
        control,
        admission(control),
        device_id,
        session_id,
        IpAddr::from([127, 0, 0, 1]),
        &handle,
        std::future::pending(),
    );
    tokio::pin!(ready);
    assert!(futures_util::poll!(&mut ready).is_pending());
    fixture.state.lifecycle().begin_shutdown();
    let waiting = fixture.state.lifecycle().wait();
    tokio::pin!(waiting);
    assert!(
        futures_util::poll!(&mut waiting).is_pending(),
        "SessionReady still owns admission"
    );
    assert!(
        !timeout(Duration::from_secs(3), &mut ready)
            .await
            .unwrap_or_else(|error| panic!("ready shutdown: {error}"))
    );
    timeout(Duration::from_secs(3), waiting)
        .await
        .unwrap_or_else(|error| panic!("producer wait: {error}"));
    control.shutdown().await;
    fixture.state.journal().shutdown().await;
    let records = drain_events(&mut recorded.receiver);
    let rejected = records.iter().position(|record| matches!(&record.event, Event::AdmissionRejected(event) if event.stage == crate::journal::event::types::AdmissionStage::ServerStopping)).unwrap_or_else(|| panic!("rejection"));
    let disconnected = records.iter().position(|record| matches!(&record.event, Event::Disconnected(event) if event.session_id.value() == session_id)).unwrap_or_else(|| panic!("disconnect"));
    assert!(rejected < disconnected);
    assert!(
        !records
            .iter()
            .any(|record| matches!(&record.event, Event::AdmissionCompleted(_)))
    );
    assert_terminal(&records, session_id, &DisconnectReason::ServerShutdown);
}

struct FailedQueryGate {
    /// Notifies the fixture while reconciliation is suspended inside the failing query.
    entered: Arc<Notify>,
    /// Releases the blocking database callback after the connection end is queued.
    release: Arc<std::sync::Barrier>,
}

impl diesel::connection::Instrumentation for FailedQueryGate {
    fn on_connection_event(&mut self, event: diesel::connection::InstrumentationEvent<'_>) {
        if matches!(
            event,
            diesel::connection::InstrumentationEvent::FinishQuery { error: Some(_), .. }
        ) {
            self.entered.notify_one();
            self.release.wait();
        }
    }
}

#[tokio::test]
async fn queued_client_close_wins_over_a_permanent_reconcile_failure()
-> Result<(), crate::test_error::TestError> {
    let mut recorded = RecordedFixture::new().await;
    let fixture = &recorded.fixture;
    let device_id = fixture.activate(&SigningKey::from_bytes(&[0x62; 32])).await;
    let (outbound, _outgoing) = mpsc::channel(1);
    let (session_id, handle) = replace_current_lease(&fixture.state, device_id, outbound).await;
    fixture
        .database
        .write(|transaction| {
            // Fault injection removes the table needed by materialization.
            transaction
                .connection()
                .batch_execute("DROP TABLE runtime_config")
                .map_err(PersistenceError::from)
        })
        .await?;
    let entered = Arc::new(Notify::new());
    let release = Arc::new(std::sync::Barrier::new(2));
    for mut connection in fixture.database.test_exhaust_pool() {
        connection.set_instrumentation(FailedQueryGate {
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
        });
    }
    handle
        .enqueue_client_state(session_id, valid_snapshot())
        .await
        .unwrap_or_else(|error| panic!("enqueue: {error}"));
    timeout(Duration::from_secs(3), entered.notified()).await?;
    handle
        .clear_lease_if_current(session_id, super::super::ConnectionEnd::ClientClose)
        .await;
    release.wait();
    fixture
        .state
        .device_control()
        .registry
        .read_connection_state(device_id)
        .await;
    assert_terminal(
        &drain_events(&mut recorded.receiver),
        session_id,
        &DisconnectReason::ClientClose,
    );
    Ok(())
}
