use std::sync::{Arc, Mutex};

use natsume_workstation_ipc::{DEVICE1_PATH, SessionPresentation, WaitingTeam};
use tokio::net::UnixStream;

use super::*;

#[derive(Clone)]
struct DeviceFixture(Arc<Mutex<State>>);

struct State {
    lease: SessionAgentLease,
    snapshot: SessionUiSnapshot,
    live: bool,
    confirm_error: Option<zbus::fdo::Error>,
    get_error: Option<&'static str>,
    get_delay: Duration,
    gets: usize,
    confirms: usize,
    accepted: Option<SessionPresentation>,
}

#[zbus::interface(name = "org.natsume.Device1")]
impl DeviceFixture {
    fn confirm_session_presentation(
        &self,
        lease_id: &str,
        frame: SessionPresentation,
    ) -> zbus::fdo::Result<()> {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(|error| panic!("fixture: {error}"));
        state.confirms += 1;
        state.accepted = None;
        if let Some(error) = &state.confirm_error {
            return Err(error.clone());
        }
        if !state.live || lease_id != state.lease.lease_id {
            return Err(zbus::fdo::Error::Failed(
                "Session Agent lease is stale".into(),
            ));
        }
        if frame.session != state.lease.session {
            return Err(zbus::fdo::Error::Failed(
                "Session Agent caller was rejected".into(),
            ));
        }
        let dimensions_valid = if frame.first_frame_presented {
            frame.fullscreen_width > 0 && frame.fullscreen_height > 0
        } else {
            frame.fullscreen_width == 0 && frame.fullscreen_height == 0
        };
        if frame.agent_lease_id != lease_id
            || frame.ui_revision != state.snapshot.ui_revision
            || !dimensions_valid
        {
            return Err(zbus::fdo::Error::Failed(
                "Session presentation is incomplete or stale".into(),
            ));
        }
        state.accepted = frame.first_frame_presented.then_some(frame);
        Ok(())
    }

    async fn get_session_ui_snapshot(
        &self,
        lease_id: &str,
        session: GraphicalSession,
    ) -> zbus::fdo::Result<SessionUiSnapshot> {
        let (delay, result) = {
            let mut state = self
                .0
                .lock()
                .unwrap_or_else(|error| panic!("fixture: {error}"));
            state.gets += 1;
            let result = if let Some(error) = state.get_error {
                Err(zbus::fdo::Error::Failed(error.into()))
            } else if !state.live || lease_id != state.lease.lease_id {
                Err(zbus::fdo::Error::Failed(
                    "Session Agent lease is stale".into(),
                ))
            } else if session != state.lease.session {
                Err(zbus::fdo::Error::Failed(
                    "Session Agent caller was rejected".into(),
                ))
            } else {
                Ok(state.snapshot.clone())
            };
            (state.get_delay, result)
        };
        tokio::time::sleep(delay).await;
        result
    }
}

fn fixture() -> (DeviceFixture, SessionAgentLease, SessionPresentation) {
    let session = GraphicalSession {
        logind_session_id: "waiting".into(),
        boot_id: "7138aed1-5921-4c4b-8e35-110806942def".into(),
    };
    let lease = SessionAgentLease {
        lease_id: "existing-lease".into(),
        session: session.clone(),
        expires_at_unix_ms: i64::MAX,
    };
    let frame = SessionPresentation {
        agent_lease_id: lease.lease_id.clone(),
        session: session.clone(),
        ui_revision: 10,
        first_frame_presented: true,
        fullscreen_width: 1920,
        fullscreen_height: 1080,
    };
    let snapshot = SessionUiSnapshot {
        agent_lease_id: lease.lease_id.clone(),
        ui_revision: 11,
        ..waiting_snapshot(&session)
    };
    (
        DeviceFixture(Arc::new(Mutex::new(State {
            lease: lease.clone(),
            snapshot,
            live: true,
            confirm_error: None,
            get_error: None,
            get_delay: Duration::ZERO,
            gets: 0,
            confirms: 0,
            accepted: None,
        }))),
        lease,
        frame,
    )
}

async fn peer(service: DeviceFixture) -> (zbus::Connection, zbus::Connection) {
    let (server, client) = UnixStream::pair().unwrap_or_else(|error| panic!("socket: {error}"));
    let server = zbus::connection::Builder::unix_stream(
        server
            .into_std()
            .unwrap_or_else(|error| panic!("server socket: {error}")),
    )
    .server(zbus::Guid::generate())
    .p2p()
    .serve_at(DEVICE1_PATH, service);
    let client = zbus::connection::Builder::unix_stream(
        client
            .into_std()
            .unwrap_or_else(|error| panic!("client socket: {error}")),
    )
    .p2p();
    let (server, client) = tokio::join!(server.build(), client.build());
    (
        server.unwrap_or_else(|error| panic!("server connection: {error}")),
        client.unwrap_or_else(|error| panic!("client connection: {error}")),
    )
}

#[tokio::test]
async fn superseded_frame_refreshes_same_lease_and_waits_for_actual_current_frame() {
    let (service, lease, frame) = fixture();
    let (_server, client) = peer(service.clone()).await;
    let proxy = Device1Proxy::new(&client)
        .await
        .unwrap_or_else(|error| panic!("proxy: {error}"));
    // Fetch revision 10, then advance the daemon before that frame is confirmed.
    service
        .0
        .lock()
        .unwrap_or_else(|error| panic!("fixture: {error}"))
        .snapshot
        .ui_revision = 10;
    let fetched = proxy
        .get_session_ui_snapshot(&lease.lease_id, &lease.session)
        .await
        .unwrap_or_else(|error| panic!("initial snapshot: {error}"));
    assert_eq!(fetched.ui_revision, frame.ui_revision);
    service
        .0
        .lock()
        .unwrap_or_else(|error| panic!("fixture: {error}"))
        .snapshot
        .ui_revision = 11;
    let deadline = Instant::now() + Duration::from_secs(2);
    let result = confirm_presentation(&proxy, &lease, deadline, &frame).await;
    let Ok(Some(snapshot)) = result else {
        panic!("superseded frame must refresh on the existing lease");
    };
    assert_eq!(snapshot.agent_lease_id, lease.lease_id);
    assert_eq!(snapshot.session, lease.session);
    assert_eq!(snapshot.ui_revision, 11);
    {
        let state = service
            .0
            .lock()
            .unwrap_or_else(|error| panic!("fixture: {error}"));
        assert!(
            state.accepted.is_none(),
            "rejected frame must never become ready"
        );
        assert_eq!(
            state.confirms, 1,
            "refresh must not confirm a synthetic frame"
        );
    }
    let current_frame = SessionPresentation {
        ui_revision: snapshot.ui_revision,
        ..frame
    };
    assert!(matches!(
        confirm_presentation(&proxy, &lease, deadline, &current_frame).await,
        Ok(None)
    ));
    let state = service
        .0
        .lock()
        .unwrap_or_else(|error| panic!("fixture: {error}"));
    assert_eq!(state.accepted.as_ref(), Some(&current_frame));
    assert_eq!(state.confirms, 2);
    assert_eq!(
        state.gets, 2,
        "current frame acceptance must not fetch again"
    );
}

#[tokio::test]
async fn superseded_binding_frame_preserves_authoritative_waiting_team_on_same_lease() {
    for screen in [
        SessionScreenKind::BindingPrompt,
        SessionScreenKind::BindingPending,
    ] {
        let (service, lease, frame) = fixture();
        let binding = SessionUiSnapshot {
            agent_lease_id: lease.lease_id.clone(),
            ui_revision: frame.ui_revision,
            screen,
            offline: false,
            negotiation_id: Some("completed-negotiation".into()),
            submission_epoch: Some(7),
            ..waiting_snapshot(&lease.session)
        };
        service
            .0
            .lock()
            .unwrap_or_else(|error| panic!("fixture: {error}"))
            .snapshot = binding.clone();
        let (_server, client) = peer(service.clone()).await;
        let proxy = Device1Proxy::new(&client)
            .await
            .unwrap_or_else(|error| panic!("proxy: {error}"));
        let fetched = proxy
            .get_session_ui_snapshot(&lease.lease_id, &lease.session)
            .await
            .unwrap_or_else(|error| panic!("binding snapshot: {error}"));
        assert_eq!(fetched, binding);

        let team = WaitingTeam {
            binding_id: "completed-binding".into(),
            account_id: "team-account".into(),
            seat_code: "B-12".into(),
            organization_id: "INST-002".into(),
            team_name_zh: "星辰队".into(),
            team_name_en: "Team Stars".into(),
            school_name_zh: "示例大学".into(),
            school_name_en: "Example University".into(),
        };
        let logo_path = "/var/lib/natsume-display/completed-team.png";
        let waiting = SessionUiSnapshot {
            agent_lease_id: lease.lease_id.clone(),
            ui_revision: frame.ui_revision + 1,
            team: Some(team.clone()),
            logo_path: Some(logo_path.into()),
            offline: false,
            ..waiting_snapshot(&lease.session)
        };
        // Binding completes after the Agent fetch, before its old frame RPC.
        service
            .0
            .lock()
            .unwrap_or_else(|error| panic!("fixture: {error}"))
            .snapshot = waiting.clone();
        let deadline = Instant::now() + Duration::from_secs(2);
        let Ok(Some(snapshot)) = confirm_presentation(&proxy, &lease, deadline, &frame).await
        else {
            panic!("superseded {screen:?} must recover the authoritative Waiting snapshot");
        };
        assert_eq!(
            snapshot, waiting,
            "all typed post-binding data must survive recovery"
        );
        assert_eq!(snapshot.team.as_ref(), Some(&team));
        assert_eq!(snapshot.logo_path.as_deref(), Some(logo_path));
        assert!(!snapshot.offline);
        {
            let state = service
                .0
                .lock()
                .unwrap_or_else(|error| panic!("fixture: {error}"));
            assert!(
                state.accepted.is_none(),
                "old Binding frame must never become ready"
            );
            assert_eq!(
                state.confirms, 1,
                "no synthetic Waiting frame may be confirmed"
            );
        }

        // A separate presentation event, not the successful snapshot read,
        // supplies frame evidence for the authoritative Waiting revision.
        let waiting_frame = SessionPresentation {
            agent_lease_id: snapshot.agent_lease_id.clone(),
            session: snapshot.session.clone(),
            ui_revision: snapshot.ui_revision,
            ..frame
        };
        assert!(matches!(
            confirm_presentation(&proxy, &lease, deadline, &waiting_frame).await,
            Ok(None)
        ));
        let state = service
            .0
            .lock()
            .unwrap_or_else(|error| panic!("fixture: {error}"));
        assert_eq!(state.accepted.as_ref(), Some(&waiting_frame));
        assert_eq!(state.lease, lease);
        assert_eq!(state.confirms, 2);
        assert_eq!(
            state.gets, 2,
            "acceptance must not fetch or replace the snapshot"
        );
    }
}

#[tokio::test]
async fn rejected_confirmation_negative_controls_fail_closed() {
    // Even if a newer snapshot could be fetched, only the known stale-frame
    // rejection may enter recovery. Authentication and lease errors are terminal.
    for case in [
        "expired",
        "replaced",
        "authentication",
        "access_denied",
        "auth_failed",
        "current_revision",
        "regressed_revision",
        "snapshot_session",
        "snapshot_lease",
        "get_authentication",
        "get_expired",
        "get_replaced",
        "get_invalid",
        "invalid_frame",
        "frame_lease",
        "frame_session",
        "other_error",
    ] {
        let (service, lease, mut frame) = fixture();
        let expected_gets = match case {
            "expired" | "replaced" | "authentication" | "access_denied" | "auth_failed"
            | "invalid_frame" | "frame_lease" | "frame_session" | "other_error" => 0,
            _ => 1,
        };
        {
            let mut state = service
                .0
                .lock()
                .unwrap_or_else(|error| panic!("fixture: {error}"));
            match case {
                "expired" => state.live = false,
                "replaced" => state.lease.lease_id = "replacement".into(),
                "authentication" => {
                    state.confirm_error = Some(zbus::fdo::Error::Failed(
                        "Session Agent caller was rejected".into(),
                    ));
                }
                "access_denied" => {
                    state.confirm_error = Some(zbus::fdo::Error::AccessDenied(
                        "Session presentation is incomplete or stale".into(),
                    ));
                }
                "auth_failed" => {
                    state.confirm_error = Some(zbus::fdo::Error::AuthFailed(
                        "Session presentation is incomplete or stale".into(),
                    ));
                }
                "current_revision" => {
                    state.confirm_error = Some(zbus::fdo::Error::Failed(
                        "Session presentation is incomplete or stale".into(),
                    ));
                    state.snapshot.ui_revision = frame.ui_revision;
                }
                "regressed_revision" => state.snapshot.ui_revision = frame.ui_revision - 1,
                "snapshot_session" => state.snapshot.session.logind_session_id = "different".into(),
                "snapshot_lease" => state.snapshot.agent_lease_id = "different".into(),
                "get_authentication" => state.get_error = Some("Session Agent caller was rejected"),
                "get_expired" | "get_replaced" => {
                    state.get_error = Some("Session Agent lease is stale");
                }
                "get_invalid" => state.get_error = Some("Session UI state is unavailable"),
                "invalid_frame" => frame.fullscreen_width = 0,
                "frame_lease" => frame.agent_lease_id = "different".into(),
                "frame_session" => frame.session.logind_session_id = "different".into(),
                "other_error" => {
                    state.confirm_error = Some(zbus::fdo::Error::Failed(
                        "Session UI state is unavailable".into(),
                    ));
                }
                _ => unreachable!(),
            }
        }
        let (_server, client) = peer(service.clone()).await;
        let proxy = Device1Proxy::new(&client)
            .await
            .unwrap_or_else(|error| panic!("proxy: {error}"));
        assert!(
            matches!(
                confirm_presentation(
                    &proxy,
                    &lease,
                    Instant::now() + Duration::from_secs(2),
                    &frame
                )
                .await,
                Err(ConnectionEnd::Disconnected(_))
            ),
            "case {case} must remain terminal"
        );
        let state = service
            .0
            .lock()
            .unwrap_or_else(|error| panic!("fixture: {error}"));
        assert!(
            state.accepted.is_none(),
            "case {case} must not become ready"
        );
        assert_eq!(state.gets, expected_gets, "case {case}");
    }
}

#[tokio::test]
async fn recovery_snapshot_cannot_outlive_existing_deadline() {
    let (service, lease, frame) = fixture();
    service
        .0
        .lock()
        .unwrap_or_else(|error| panic!("fixture: {error}"))
        .get_delay = Duration::from_secs(5);
    let (_server, client) = peer(service.clone()).await;
    let proxy = Device1Proxy::new(&client)
        .await
        .unwrap_or_else(|error| panic!("proxy: {error}"));
    let result = tokio::time::timeout(
        Duration::from_secs(1),
        confirm_presentation(
            &proxy,
            &lease,
            Instant::now() + Duration::from_millis(100),
            &frame,
        ),
    )
    .await;
    assert!(matches!(result, Ok(Err(ConnectionEnd::Disconnected(_)))));
    let state = service
        .0
        .lock()
        .unwrap_or_else(|error| panic!("fixture: {error}"));
    assert_eq!(state.gets, 1);
    assert!(state.accepted.is_none());
}

#[tokio::test]
async fn expired_deadline_prevents_confirmation_or_recovery() {
    let (service, lease, frame) = fixture();
    let (_server, client) = peer(service.clone()).await;
    let proxy = Device1Proxy::new(&client)
        .await
        .unwrap_or_else(|error| panic!("proxy: {error}"));
    assert!(matches!(
        confirm_presentation(&proxy, &lease, Instant::now(), &frame).await,
        Err(ConnectionEnd::Disconnected(_))
    ));
    let state = service
        .0
        .lock()
        .unwrap_or_else(|error| panic!("fixture: {error}"));
    assert_eq!(state.confirms, 0);
    assert_eq!(state.gets, 0);
    assert!(state.accepted.is_none());
}
