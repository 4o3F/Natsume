#![cfg(windows)]
use natsume_photo_client::{
    capture::{Error as CaptureError, GroupId, GroupState, Session},
    roster::Roster,
    workspace::Workspace,
};
use snafu::{OptionExt as _, ResultExt as _, Snafu};

const FIXTURE: &[u8] = include_bytes!("fixtures/roster.json");
const TEAM_A: &str = "0199a000-0000-7000-8000-000000000001";
const TEAM_B: &str = "0199a000-0000-7000-8000-000000000002";

#[derive(Debug, Snafu)]
enum Error {
    Roster {
        source: natsume_photo_client::roster::Error,
    },
    Capture {
        source: CaptureError,
    },
    Workspace {
        source: natsume_photo_client::workspace::Error,
    },
    Io {
        source: std::io::Error,
    },
    Fixture,
}
type Result<T = ()> = std::result::Result<T, Error>;

fn session() -> Result<Session> {
    Ok(Session::new(Roster::parse(FIXTURE).context(RosterSnafu)?))
}

fn start(session: &mut Session) -> Result<GroupId> {
    session.select_team(TEAM_A).context(CaptureSnafu)?;
    session.start_group().context(CaptureSnafu)
}

#[test]
fn selection_is_not_a_group_and_requires_internal_identity() -> Result {
    let mut session = session()?;
    assert!(session.selected_team().is_none());
    assert!(session.group().is_none());
    assert!(matches!(
        session.start_group(),
        Err(CaptureError::NoSelection)
    ));
    for invalid in ["", "missing", "T-001", "测试一队"] {
        assert!(matches!(
            session.select_team(invalid),
            Err(CaptureError::UnknownTeam)
        ));
    }
    session.select_team(TEAM_A).context(CaptureSnafu)?;
    assert_eq!(
        session.selected_team().context(FixtureSnafu)?.account,
        "T-001"
    );
    assert!(session.group().is_none());
    session.select_team(TEAM_B).context(CaptureSnafu)?;
    assert_eq!(
        session.selected_team().context(FixtureSnafu)?.account_id,
        TEAM_B
    );
    assert!(matches!(
        session.select_team("missing"),
        Err(CaptureError::UnknownTeam)
    ));
    assert_eq!(
        session.selected_team().context(FixtureSnafu)?.account_id,
        TEAM_B
    );
    Ok(())
}

#[test]
fn starting_locks_team_and_rejects_duplicate_start_even_for_same_team() -> Result {
    let mut session = session()?;
    let id = start(&mut session)?;
    for team in [TEAM_A, TEAM_B] {
        assert!(matches!(
            session.select_team(team),
            Err(CaptureError::GroupActive)
        ));
    }
    assert!(matches!(
        session.start_group(),
        Err(CaptureError::GroupActive)
    ));
    let group = session.group().context(FixtureSnafu)?;
    assert_eq!(group.id(), id);
    assert_eq!(group.account_id(), TEAM_A);
    assert_eq!(group.state(), GroupState::Capturing);
    Ok(())
}

#[test]
fn unconfirmed_group_cannot_finish_but_can_be_explicitly_abandoned() -> Result {
    let mut session = session()?;
    let id = start(&mut session)?;
    assert!(matches!(
        session.finish_group(id),
        Err(CaptureError::Unconfirmed)
    ));
    assert!(session.group().is_some());
    session.abandon_group(id).context(CaptureSnafu)?;
    assert!(session.group().is_none());
    // Retained selection permits an explicit new group for the same team.
    let new_id = session.start_group().context(CaptureSnafu)?;
    assert_ne!(id, new_id);
    assert_eq!(session.group().context(FixtureSnafu)?.account_id(), TEAM_A);
    Ok(())
}

#[test]
fn all_known_tasks_must_finish_before_abandonment_and_switching() -> Result {
    let mut session = session()?;
    let id = start(&mut session)?;
    let first = session.task_started(id).context(CaptureSnafu)?;
    let second = session.task_started(id).context(CaptureSnafu)?;
    assert_ne!(first, second);
    for result in [session.finish_group(id), session.abandon_group(id)] {
        assert!(matches!(
            result,
            Err(CaptureError::PendingTasks { count: 2 })
        ));
    }
    session.task_finished(id, first).context(CaptureSnafu)?;
    assert!(matches!(
        session.abandon_group(id),
        Err(CaptureError::PendingTasks { count: 1 })
    ));
    assert!(matches!(
        session.select_team(TEAM_B),
        Err(CaptureError::GroupActive)
    ));
    session.task_finished(id, second).context(CaptureSnafu)?;
    session.abandon_group(id).context(CaptureSnafu)?;
    session.select_team(TEAM_B).context(CaptureSnafu)?;
    session.start_group().context(CaptureSnafu)?;
    assert_eq!(session.group().context(FixtureSnafu)?.account_id(), TEAM_B);
    Ok(())
}

#[test]
fn duplicate_completion_does_not_drain_another_task() -> Result {
    let mut session = session()?;
    let id = start(&mut session)?;
    let first = session.task_started(id).context(CaptureSnafu)?;
    let second = session.task_started(id).context(CaptureSnafu)?;
    session.task_finished(id, first).context(CaptureSnafu)?;
    assert!(matches!(
        session.task_finished(id, first),
        Err(CaptureError::UnknownTask)
    ));
    assert_eq!(session.group().context(FixtureSnafu)?.pending_tasks(), 1);
    session.task_finished(id, second).context(CaptureSnafu)?;
    Ok(())
}

#[test]
fn stale_group_callbacks_cannot_change_the_next_group() -> Result {
    let mut session = session()?;
    let old_id = start(&mut session)?;
    let old_task = session.task_started(old_id).context(CaptureSnafu)?;
    session
        .task_finished(old_id, old_task)
        .context(CaptureSnafu)?;
    session.abandon_group(old_id).context(CaptureSnafu)?;
    session.select_team(TEAM_B).context(CaptureSnafu)?;
    let id = session.start_group().context(CaptureSnafu)?;
    let task = session.task_started(id).context(CaptureSnafu)?;
    assert!(matches!(
        session.task_started(old_id),
        Err(CaptureError::StaleGroup)
    ));
    assert!(matches!(
        session.task_finished(old_id, old_task),
        Err(CaptureError::StaleGroup)
    ));
    assert!(matches!(
        session.task_finished(id, old_task),
        Err(CaptureError::UnknownTask)
    ));
    assert!(matches!(
        session.abandon_group(old_id),
        Err(CaptureError::StaleGroup)
    ));
    assert!(matches!(
        session.finish_group(old_id),
        Err(CaptureError::StaleGroup)
    ));
    assert_eq!(session.group().context(FixtureSnafu)?.pending_tasks(), 1);
    session.task_finished(id, task).context(CaptureSnafu)?;
    Ok(())
}

#[test]
fn callbacks_and_closure_without_a_group_fail() -> Result {
    let mut session = session()?;
    let id = start(&mut session)?;
    let task = session.task_started(id).context(CaptureSnafu)?;
    session.task_finished(id, task).context(CaptureSnafu)?;
    session.abandon_group(id).context(CaptureSnafu)?;
    assert!(matches!(
        session.task_started(id),
        Err(CaptureError::NoGroup)
    ));
    assert!(matches!(
        session.task_finished(id, task),
        Err(CaptureError::NoGroup)
    ));
    assert!(matches!(
        session.abandon_group(id),
        Err(CaptureError::NoGroup)
    ));
    assert!(matches!(
        session.finish_group(id),
        Err(CaptureError::NoGroup)
    ));
    Ok(())
}

#[test]
fn pause_blocks_selection_and_start_until_resumed() -> Result {
    let mut session = session()?;
    session.select_team(TEAM_A).context(CaptureSnafu)?;
    session.pause();
    session.pause();
    assert!(session.is_paused());
    assert!(matches!(
        session.select_team(TEAM_B),
        Err(CaptureError::Paused)
    ));
    assert!(matches!(session.start_group(), Err(CaptureError::Paused)));
    assert_eq!(
        session.selected_team().context(FixtureSnafu)?.account_id,
        TEAM_A
    );
    session.resume();
    session.resume();
    assert!(!session.is_paused());
    session.start_group().context(CaptureSnafu)?;
    Ok(())
}

#[test]
fn paused_group_keeps_identity_and_can_drain_tasks_but_cannot_close() -> Result {
    let mut session = session()?;
    let id = start(&mut session)?;
    let first = session.task_started(id).context(CaptureSnafu)?;
    session.pause();
    let late = session.task_started(id).context(CaptureSnafu)?;
    session.task_finished(id, first).context(CaptureSnafu)?;
    session.task_finished(id, late).context(CaptureSnafu)?;
    assert!(matches!(
        session.finish_group(id),
        Err(CaptureError::Paused)
    ));
    assert!(matches!(
        session.abandon_group(id),
        Err(CaptureError::Paused)
    ));
    assert!(matches!(
        session.select_team(TEAM_B),
        Err(CaptureError::Paused)
    ));
    assert_eq!(session.group().context(FixtureSnafu)?.id(), id);
    session.resume();
    session.abandon_group(id).context(CaptureSnafu)?;
    Ok(())
}

#[test]
fn callbacks_from_another_session_are_rejected() -> Result {
    let mut first = session()?;
    let mut second = session()?;
    let first_id = start(&mut first)?;
    let second_id = start(&mut second)?;
    assert_ne!(first.id(), second.id());
    assert_ne!(first_id, second_id);
    let task = first.task_started(first_id).context(CaptureSnafu)?;
    assert!(matches!(
        second.task_started(first_id),
        Err(CaptureError::StaleGroup)
    ));
    assert!(matches!(
        second.task_finished(second_id, task),
        Err(CaptureError::UnknownTask)
    ));
    assert_eq!(second.group().context(FixtureSnafu)?.pending_tasks(), 0);
    Ok(())
}

#[tokio::test]
async fn reopening_workspace_restores_roster_but_not_selection_group_or_pending_tasks() -> Result {
    let directory = tempfile::tempdir().context(IoSnafu)?;
    let root = directory.path().join("工作区");
    let roster = Roster::parse(FIXTURE).context(RosterSnafu)?;
    let workspace = Workspace::create(&root, &roster)
        .await
        .context(WorkspaceSnafu)?;
    let mut old = Session::new(workspace.roster().await.context(WorkspaceSnafu)?);
    let old_id = start(&mut old)?;
    let old_task = old.task_started(old_id).context(CaptureSnafu)?;
    old.pause();
    drop(old);
    drop(workspace);
    let database_path = root.join("workspace.sqlite3");
    let before = std::fs::read(&database_path).context(IoSnafu)?;
    let workspace = Workspace::open(&root).await.context(WorkspaceSnafu)?;
    let mut fresh = Session::new(workspace.roster().await.context(WorkspaceSnafu)?);
    assert!(fresh.selected_team().is_none());
    assert!(fresh.group().is_none());
    assert!(!fresh.is_paused());
    assert!(matches!(
        fresh.start_group(),
        Err(CaptureError::NoSelection)
    ));
    let fresh_id = start(&mut fresh)?;
    assert_ne!(old_id, fresh_id);
    assert!(matches!(
        fresh.task_finished(old_id, old_task),
        Err(CaptureError::StaleGroup)
    ));
    drop(workspace);
    assert_eq!(std::fs::read(database_path).context(IoSnafu)?, before);
    // No session state file or new database is created.
    assert_eq!(std::fs::read_dir(root).context(IoSnafu)?.count(), 1);
    Ok(())
}
