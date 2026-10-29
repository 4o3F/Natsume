use super::*;
use crate::journal::{
    event::{
        payload::*,
        types::{Category, PauseReason, SessionId, Severity},
    },
    read::{EventDetails, RecordHeader},
};

fn session() -> SessionId {
    SessionId::from_lease(Uuid::now_v7())
}
fn record(id: i64, at: i64, event: Event) -> JournalRecord {
    JournalRecord::Readable {
        header: Box::new(RecordHeader {
            id,
            occurred_at_ms: at,
            boot_id: Uuid::from_u128(1),
            category: event.category(),
            severity: Severity::Info,
            device_id: None,
            session_id: None,
            actor_operator_id: None,
            request_id: None,
            operation_id: None,
            trace_id: None,
            seat_code: None,
        }),
        details: EventDetails { v: 1, event },
    }
}
fn attach(id: i64, at: i64, session_id: SessionId) -> JournalRecord {
    record(id, at, Event::LeaseAttached(LeaseAttached { session_id }))
}
fn disconnect(id: i64, at: i64, session_id: SessionId) -> JournalRecord {
    record(
        id,
        at,
        Event::Disconnected(Disconnected {
            session_id,
            reason: DisconnectReason::PingSilence,
        }),
    )
}
fn reconstruction(from: i64, to: i64) -> Reconstruction {
    Reconstruction::new(from, to, None, &RetainedBounds::default())
}

#[test]
fn reconnect_has_reason_duration_truncation_and_only_verified_live_is_ongoing() {
    let first = session();
    let second = session();
    let mut r = reconstruction(25, 80);
    r.apply(record(
        1,
        5,
        Event::BindingBound(BindingBound {
            binding_id: Uuid::now_v7(),
            seat_code: SeatCode::from_committed_binding("B4")
                .unwrap_or_else(|e| panic!("seat: {e}")),
        }),
    ));
    r.apply(attach(2, 20, first));
    r.apply(disconnect(3, 40, first));
    r.apply(attach(4, 50, second));
    let result = r.finish(Some(second.value()));
    assert_eq!(result.sessions.len(), 2);
    let first = &result.sessions[0];
    assert_eq!(first.duration_ms, Some(15));
    assert!(first.truncated_start);
    assert!(matches!(
        first.end,
        SessionEnd::Disconnected {
            reason: Some(DisconnectReason::PingSilence),
            at: 40
        }
    ));
    assert_eq!(first.seat_code.as_ref().map(SeatCode::as_str), Some("B4"));
    assert!(matches!(result.sessions[1].end, SessionEnd::Ongoing));
    assert!(result.sessions[1].truncated_end);
}
#[test]
fn replacement_and_actor_stop_close_only_the_matching_session() {
    let first = session();
    let second = session();
    let mut r = reconstruction(0, 100);
    r.apply(attach(1, 10, first));
    r.apply(record(
        2,
        20,
        Event::LeaseReplaced(LeaseReplaced {
            old: first,
            new: second,
        }),
    ));
    r.apply(disconnect(3, 25, first));
    r.apply(record(
        4,
        30,
        Event::ActorStopped(ActorStopped {
            panicked: true,
            session_id: Some(second),
        }),
    ));
    let result = r.finish(None);
    assert_eq!(result.sessions.len(), 2);
    assert_eq!(result.sessions[0].duration_ms, Some(10));
    assert_eq!(result.sessions[1].duration_ms, Some(10));
    assert!(matches!(
        result.sessions[1].end,
        SessionEnd::Disconnected {
            reason: Some(DisconnectReason::ActorFailed),
            ..
        }
    ));
}
#[test]
fn restart_and_stopping_bound_unknown_ends_without_claiming_a_disconnect_time() {
    for event in [
        Event::ServerStarted(ServerStarted {
            boot_id: Uuid::from_u128(2),
        }),
        Event::ServerStopping(ServerStopping {
            lost_on_flush: None,
        }),
    ] {
        let first = session();
        let mut r = reconstruction(0, 100);
        r.apply(attach(1, 10, first));
        r.apply(record(2, 30, event));
        let result = r.finish(Some(first.value()));
        assert!(matches!(
            result.sessions[0].end,
            SessionEnd::Unknown {
                no_later_than: Some(30)
            }
        ));
    }
}
#[test]
fn old_terminal_sessions_do_not_revive_and_window_edges_are_clipped() {
    let id = session();
    let mut r = reconstruction(30, 35);
    r.apply(attach(1, 10, id));
    r.apply(disconnect(2, 20, id));
    assert!(r.finish(Some(id.value())).sessions.is_empty());
    let mut r = reconstruction(30, 35);
    r.apply(attach(1, 10, id));
    r.apply(disconnect(2, 40, id));
    let result = r.finish(None);
    let row = &result.sessions[0];
    assert!(row.truncated_start && row.truncated_end);
    assert_eq!(row.duration_ms, Some(5));
}
#[test]
fn backwards_wall_clock_clamps_duration_and_marks_it_imprecise() {
    let id = session();
    let mut r = reconstruction(0, 100);
    r.apply(attach(1, 20, id));
    r.apply(disconnect(2, 10, id));
    let result = r.finish(None);
    assert_eq!(result.sessions[0].duration_ms, Some(0));
    assert!(result.sessions[0].imprecise);
}
#[test]
fn retention_drop_pause_and_unreadable_gaps_are_explicit() {
    let mut r = Reconstruction::new(
        0,
        100,
        None,
        &RetainedBounds {
            oldest_id: Some(10),
            oldest_at: Some(5),
            high_water: Some(20),
        },
    );
    r.apply(record(
        10,
        20,
        Event::JournalDropped(JournalDropped {
            category: Category::Device,
            count: 2,
            since: 10,
        }),
    ));
    r.apply(record(
        11,
        30,
        Event::JournalPaused(JournalPaused {
            reason: PauseReason::DiskBudget,
        }),
    ));
    r.apply(record(12, 40, Event::JournalResumed(JournalResumed {})));
    r.apply(record(
        13,
        50,
        Event::JournalPaused(JournalPaused {
            reason: PauseReason::DiskBudget,
        }),
    ));
    r.apply(JournalRecord::Unreadable {
        id: 14,
        kind: "unknown".into(),
    });
    let result = r.finish(None);
    assert_eq!(result.gaps.len(), 5);
    assert!(
        result
            .gaps
            .iter()
            .any(|g| matches!(g.kind, GapKind::Pruned))
    );
    assert!(
        result
            .gaps
            .iter()
            .any(|g| matches!(g.kind, GapKind::Dropped) && g.from == 10 && g.to == Some(20))
    );
    assert!(
        result
            .gaps
            .iter()
            .any(|g| matches!(g.kind, GapKind::Paused) && g.from == 50 && g.to.is_none())
    );
}
#[test]
fn missing_or_different_live_lease_never_implies_online() {
    let id = session();
    let mut r = reconstruction(0, 100);
    r.apply(attach(1, 10, id));
    let result = r.finish(Some(Uuid::now_v7()));
    assert!(matches!(
        result.sessions[0].end,
        SessionEnd::Unknown {
            no_later_than: None
        }
    ));
}

#[test]
fn a_backwards_terminal_does_not_reappear_in_later_windows() {
    let id = session();
    let mut r = reconstruction(200, 300);
    r.apply(attach(1, 100, id));
    r.apply(disconnect(2, 50, id));
    assert!(r.finish(None).sessions.is_empty());
}

#[test]
fn late_actor_stop_repairs_its_old_session_without_closing_a_new_lease() {
    for stopped in [Some(session()), None] {
        let first = stopped.unwrap_or_else(session);
        let second = session();
        let mut reconstruction = reconstruction(0, 100);
        reconstruction.apply(attach(1, 10, first));
        reconstruction.apply(attach(2, 20, second));
        reconstruction.apply(record(
            3,
            30,
            Event::ActorStopped(ActorStopped {
                panicked: true,
                session_id: stopped,
            }),
        ));
        let result = reconstruction.finish(Some(second.value()));
        assert!(matches!(result.sessions[1].end, SessionEnd::Ongoing));
        if stopped.is_some() {
            assert!(matches!(
                result.sessions[0].end,
                SessionEnd::Disconnected {
                    reason: Some(DisconnectReason::ActorFailed),
                    at: 30
                }
            ));
        }
    }
}

#[tokio::test]
async fn timeline_regression_anchor_never_reads_past_the_captured_high_water()
-> Result<(), crate::test_error::TestError> {
    let _guard = crate::component::operator::tests::PasswordVerificationTestGuard::acquire().await;
    let fixture = crate::journal_query::tests::Fixture::new().await?;
    fixture
        .seed(vec![fixture.row(
            10,
            Event::LeaseAttached(LeaseAttached {
                session_id: session(),
            }),
        )])
        .await?;
    let ceiling = fixture
        .state
        .journal()
        .reader()
        .bounds()
        .await
        .map_err(QueryError::from)?
        .high_water
        .ok_or(crate::test_error::TestError::missing("ceiling"))?;
    fixture
        .seed(vec![fixture.row(
            20,
            Event::LeaseAttached(LeaseAttached {
                session_id: session(),
            }),
        )])
        .await?;
    let baseline = fixture
        .state
        .journal_query()
        .timeline_baseline(fixture.device_id, 25, Some(ceiling))
        .await?;
    assert!(baseline.first_id.is_none_or(|id| id <= ceiling));
    fixture.state.journal().shutdown().await;
    Ok(())
}

#[test]
fn timeline_memory_discards_completed_sessions_and_gaps_outside_the_window() {
    let mut reconstruction = reconstruction(100, 200);
    for index in 0..2000 {
        let session_id = session();
        reconstruction.apply(attach(index * 3 + 1, 10, session_id));
        reconstruction.apply(disconnect(index * 3 + 2, 20, session_id));
        reconstruction.apply(record(
            index * 3 + 3,
            20,
            Event::JournalDropped(JournalDropped {
                count: 1,
                since: 10,
                category: Category::Device,
            }),
        ));
        assert!(reconstruction.sessions.is_empty());
        assert!(reconstruction.gaps.is_empty());
    }
}

#[test]
fn timeline_memory_caps_sessions_and_gaps_and_marks_omitted_history() {
    let mut reconstruction = reconstruction(0, 10000);
    for index in 0..1200 {
        let session_id = session();
        reconstruction.apply(attach(index * 3 + 1, index * 2, session_id));
        reconstruction.apply(disconnect(index * 3 + 2, index * 2 + 1, session_id));
        reconstruction.apply(record(
            index * 3 + 3,
            index * 2 + 1,
            Event::JournalDropped(JournalDropped {
                count: 1,
                since: index * 2,
                category: Category::Device,
            }),
        ));
        assert!(
            reconstruction.sessions.len() <= 500,
            "bounded during replay"
        );
        assert!(reconstruction.gaps.len() <= 500, "bounded during replay");
    }
    let result = reconstruction.finish(None);
    assert_eq!(result.sessions.len(), 500);
    assert_eq!(result.gaps.len(), 500);
    assert!(result.sessions[0].truncated_start);
    assert_eq!(result.sessions[0].started_at, 1400);
}
