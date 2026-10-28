use super::*;
use snafu::ResultExt as _;

#[derive(Debug, Snafu)]
enum TestError {
    Roster { source: crate::roster::Error },
    Capture { source: Error },
}

/// Only the fixture supplies a confirmed fact. No public confirmation shortcut.
fn confirmed_session() -> std::result::Result<(Session, GroupId), TestError> {
    let roster = Roster::parse(include_bytes!("../fixtures/roster.json")).context(RosterSnafu)?;
    let account_id = roster.teams()[0].account_id.clone();
    let mut session = Session::new(roster);
    session.select_team(&account_id).context(CaptureSnafu)?;
    let id = session.start_group().context(CaptureSnafu)?;
    if let Some(group) = &mut session.group {
        group.state = GroupState::Confirmed;
    }
    Ok((session, id))
}

#[test]
fn confirmed_group_waits_for_all_tasks_and_rejects_abandonment()
-> std::result::Result<(), TestError> {
    let (mut session, id) = confirmed_session()?;
    let task = session.task_started(id).context(CaptureSnafu)?;
    assert!(matches!(
        session.finish_group(id),
        Err(Error::PendingTasks { count: 1 })
    ));
    session.task_finished(id, task).context(CaptureSnafu)?;
    assert!(matches!(
        session.abandon_group(id),
        Err(Error::AlreadyConfirmed)
    ));
    session.finish_group(id).context(CaptureSnafu)?;
    assert!(session.group().is_none());
    let next = session.start_group().context(CaptureSnafu)?;
    assert_ne!(id, next);
    assert_eq!(
        session.group().map(Group::state),
        Some(GroupState::Capturing)
    );
    Ok(())
}

#[test]
fn confirmed_group_cannot_finish_while_paused() -> std::result::Result<(), TestError> {
    let (mut session, id) = confirmed_session()?;
    session.pause();
    assert!(matches!(session.finish_group(id), Err(Error::Paused)));
    assert_eq!(
        session.group().map(Group::state),
        Some(GroupState::Confirmed)
    );
    session.resume();
    session.finish_group(id).context(CaptureSnafu)?;
    Ok(())
}
