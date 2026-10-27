//! In-memory capture rules. This module does not admit a real hot-folder session.
//! A future capture coordinator must check monitoring and display prerequisites
//! before allowing shooting. No CLI shooting entry point is exposed in W02.

use std::collections::BTreeSet;

use snafu::{OptionExt as _, Snafu, ensure};
use uuid::Uuid;

use crate::roster::{Roster, Team};

/// An opaque group identity, unique across sessions and process restarts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GroupId(Uuid);

/// An opaque task identity. Callbacks must return this exact identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TaskId(Uuid);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupState {
    Capturing,
    /// Reserved for the successful presentation-and-save boundary in W05.
    /// Public callers cannot set this state.
    Confirmed,
}

/// Read-only facts about the active group; mutations go through `Session`.
#[derive(Debug)]
pub struct Group {
    id: GroupId,
    account_id: String,
    state: GroupState,
    tasks: BTreeSet<TaskId>,
}

impl Group {
    #[must_use]
    pub fn id(&self) -> GroupId {
        self.id
    }

    #[must_use]
    pub fn account_id(&self) -> &str {
        &self.account_id
    }

    #[must_use]
    pub fn state(&self) -> GroupState {
        self.state
    }

    #[must_use]
    pub fn pending_tasks(&self) -> usize {
        self.tasks.len()
    }
}

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("队伍不在当前名单中，请使用内部 account_id 选队"))]
    UnknownTeam,
    #[snafu(display("当前拍摄组尚未关闭，不能切队或重复开始"))]
    GroupActive,
    #[snafu(display("请先选择并核对队伍"))]
    NoSelection,
    #[snafu(display("当前没有拍摄组"))]
    NoGroup,
    #[snafu(display("会话已暂停，不能选队、开始或关闭拍摄组"))]
    Paused,
    #[snafu(display("还有 {count} 个已知任务未结束，不能关闭拍摄组"))]
    PendingTasks { count: usize },
    #[snafu(display("当前组尚未确认并成功保存成品，只能继续拍摄或明确放弃"))]
    Unconfirmed,
    #[snafu(display("该组已有确认成品，请正常结束本组"))]
    AlreadyConfirmed,
    #[snafu(display("任务回调不属于当前拍摄组"))]
    StaleGroup,
    #[snafu(display("任务不存在或已经结束"))]
    UnknownTask,
}

pub type Result<T> = std::result::Result<T, Error>;

/// Domain state only: constructing this value does not authorize real shooting.
/// Dropping it discards selection, groups and tasks, without writing any files.
#[derive(Debug)]
pub struct Session {
    id: Uuid,
    roster: Roster,
    selected_account_id: Option<String>,
    group: Option<Group>,
    paused: bool,
}

impl Session {
    #[must_use]
    pub fn new(roster: Roster) -> Self {
        Self {
            id: Uuid::now_v7(),
            roster,
            selected_account_id: None,
            group: None,
            paused: false,
        }
    }

    #[must_use]
    pub fn id(&self) -> Uuid {
        self.id
    }

    #[must_use]
    pub fn selected_team(&self) -> Option<&Team> {
        let account_id = self.selected_account_id.as_deref()?;
        self.roster
            .teams()
            .iter()
            .find(|team| team.account_id == account_id)
    }

    #[must_use]
    pub fn group(&self) -> Option<&Group> {
        self.group.as_ref()
    }

    #[must_use]
    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// Selects by internal identity; this does not start a group.
    ///
    /// # Errors
    /// Rejects paused sessions, active groups, or identities absent from the roster.
    pub fn select_team(&mut self, account_id: &str) -> Result<()> {
        ensure!(!self.paused, PausedSnafu);
        ensure!(self.group.is_none(), GroupActiveSnafu);
        ensure!(
            self.roster
                .teams()
                .iter()
                .any(|team| team.account_id == account_id),
            UnknownTeamSnafu
        );
        self.selected_account_id = Some(account_id.to_owned());
        Ok(())
    }

    /// Explicitly starts a group for the selected team, locking its identity.
    /// This domain operation does not perform hot-folder or display admission.
    ///
    /// # Errors
    /// Rejects paused sessions, duplicate starts, or missing selection.
    pub fn start_group(&mut self) -> Result<GroupId> {
        ensure!(!self.paused, PausedSnafu);
        ensure!(self.group.is_none(), GroupActiveSnafu);
        let account_id = self
            .selected_account_id
            .as_ref()
            .context(NoSelectionSnafu)?;
        let id = GroupId(Uuid::now_v7());
        self.group = Some(Group {
            id,
            account_id: account_id.clone(),
            state: GroupState::Capturing,
            tasks: BTreeSet::new(),
        });
        Ok(id)
    }

    /// Pausing preserves the locked team and all tasks. It does not close a group.
    pub fn pause(&mut self) {
        self.paused = true;
    }

    /// A future coordinator must recheck the failed prerequisite before resuming.
    pub fn resume(&mut self) {
        self.paused = false;
    }

    /// Registers known work, even while paused or draining a confirmed group.
    ///
    /// # Errors
    /// Rejects missing or stale groups; never attaches work to the next group.
    pub fn task_started(&mut self, group_id: GroupId) -> Result<TaskId> {
        let group = self.matching_group(group_id)?;
        let task_id = TaskId(Uuid::now_v7());
        group.tasks.insert(task_id);
        Ok(task_id)
    }

    /// Ends known work. Failure policy (retry or pause) belongs to the worker.
    ///
    /// # Errors
    /// Rejects missing/stale groups and unknown or duplicate task completions.
    pub fn task_finished(&mut self, group_id: GroupId, task_id: TaskId) -> Result<()> {
        let group = self.matching_group(group_id)?;
        ensure!(group.tasks.remove(&task_id), UnknownTaskSnafu);
        Ok(())
    }

    /// Closes a successfully confirmed group, retaining the team selection.
    /// W02 provides no operation that confirms a photo; W05 owns that boundary.
    ///
    /// # Errors
    /// Rejects pauses, missing/stale groups, pending work, or unconfirmed groups.
    pub fn finish_group(&mut self, group_id: GroupId) -> Result<()> {
        self.check_closable(group_id)?;
        ensure!(
            self.matching_group(group_id)?.state == GroupState::Confirmed,
            UnconfirmedSnafu
        );
        self.group = None;
        Ok(())
    }

    /// Explicitly abandons an unconfirmed group. Does not delete photos or finals.
    ///
    /// # Errors
    /// Rejects pauses, missing/stale groups, pending work, or confirmed groups.
    pub fn abandon_group(&mut self, group_id: GroupId) -> Result<()> {
        self.check_closable(group_id)?;
        ensure!(
            self.matching_group(group_id)?.state == GroupState::Capturing,
            AlreadyConfirmedSnafu
        );
        self.group = None;
        Ok(())
    }

    fn matching_group(&mut self, group_id: GroupId) -> Result<&mut Group> {
        let group = self.group.as_mut().context(NoGroupSnafu)?;
        ensure!(group.id == group_id, StaleGroupSnafu);
        Ok(group)
    }

    fn check_closable(&mut self, group_id: GroupId) -> Result<()> {
        ensure!(!self.paused, PausedSnafu);
        let group = self.matching_group(group_id)?;
        ensure!(
            group.tasks.is_empty(),
            PendingTasksSnafu {
                count: group.tasks.len()
            }
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests;
