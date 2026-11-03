//! Latest-value operation progress, independent of the busy application command queue.
use tokio::sync::watch;

/// Ordered stages shared by initialization, workspace opening and ZIP export.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Stage {
    #[default]
    Prepare,
    Process,
    Verify,
    Publish,
}

/// Actual completed work within the current stage. Zero total means unknown work.
#[derive(Debug, Clone, Default)]
pub struct Progress {
    pub stage: Stage,
    pub label: &'static str,
    pub completed: u64,
    pub total: u64,
}

/// Optional observer; dropping the receiver never cancels an accepted operation.
#[derive(Clone, Default)]
pub struct Reporter(Option<watch::Sender<Progress>>);

impl Reporter {
    /// Creates an observer whose latest value can be read without queueing a command.
    #[must_use]
    pub fn channel() -> (Self, watch::Receiver<Progress>) {
        let (sender, receiver) = watch::channel(Progress::default());
        (Self(Some(sender)), receiver)
    }

    /// Publishes measured work, or an indeterminate stage when total is zero.
    pub fn report(&self, stage: Stage, label: &'static str, completed: u64, total: u64) {
        if let Some(sender) = &self.0 {
            sender.send_replace(Progress {
                stage,
                label,
                completed,
                total,
            });
        }
    }
}
