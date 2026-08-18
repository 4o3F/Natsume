use std::time::Duration;

use super::session::SessionProgress;

pub(super) const CONTROL_RECONNECT_MIN_SECONDS: u64 = 1;
pub(super) const CONTROL_RECONNECT_MAX_SECONDS: u64 = 30;

pub(super) struct ReconnectBackoff {
    next_seconds: u64,
}

impl ReconnectBackoff {
    pub(super) const fn new() -> Self {
        Self {
            next_seconds: CONTROL_RECONNECT_MIN_SECONDS,
        }
    }

    pub(super) fn take_delay(&mut self) -> Duration {
        let delay = self.next_seconds;
        self.next_seconds = self
            .next_seconds
            .saturating_mul(2)
            .min(CONTROL_RECONNECT_MAX_SECONDS);
        Duration::from_secs(delay)
    }

    const fn reset(&mut self) {
        self.next_seconds = CONTROL_RECONNECT_MIN_SECONDS;
    }

    pub(super) const fn record_session_progress(&mut self, progress: SessionProgress) {
        if matches!(progress, SessionProgress::CommandHandled) {
            self.reset();
        }
    }

    pub(super) const fn force_maximum(&mut self) {
        self.next_seconds = CONTROL_RECONNECT_MAX_SECONDS;
    }
}
