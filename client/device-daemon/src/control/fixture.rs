#![cfg(feature = "fixture")]

use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::watch;

use super::ControlClient;

pub(super) struct FixtureState {
    attempts: AttemptCounter,
    successful_hellos: AttemptCounter,
    journaled_commands: AttemptCounter,
}

impl FixtureState {
    pub(super) fn new() -> Self {
        Self {
            attempts: AttemptCounter::new(),
            successful_hellos: AttemptCounter::new(),
            journaled_commands: AttemptCounter::new(),
        }
    }

    pub(super) fn record_connection_attempt(&self) {
        self.attempts.record();
    }

    pub(super) fn record_successful_hello(&self) {
        self.successful_hellos.record();
    }

    pub(super) fn record_journaled_command(&self) {
        self.journaled_commands.record();
    }
}

impl ControlClient {
    #[must_use]
    pub fn connection_attempt_count(&self) -> u64 {
        self.fixture.attempts.value.load(Ordering::Relaxed)
    }

    pub async fn wait_for_connection_attempt_count(&self, minimum: u64) {
        self.fixture.attempts.wait_for(minimum).await;
    }

    #[must_use]
    pub fn successful_hello_count(&self) -> u64 {
        self.fixture.successful_hellos.value.load(Ordering::Relaxed)
    }

    pub async fn wait_for_successful_hello_count(&self, minimum: u64) {
        self.fixture.successful_hellos.wait_for(minimum).await;
    }

    #[must_use]
    pub fn journaled_command_count(&self) -> u64 {
        self.fixture
            .journaled_commands
            .value
            .load(Ordering::Relaxed)
    }

    pub async fn wait_for_journaled_command_count(&self, minimum: u64) {
        self.fixture.journaled_commands.wait_for(minimum).await;
    }
}

struct AttemptCounter {
    value: AtomicU64,
    changes: watch::Sender<u64>,
}

impl AttemptCounter {
    fn new() -> Self {
        let (changes, _receiver) = watch::channel(0);
        Self {
            value: AtomicU64::new(0),
            changes,
        }
    }

    fn record(&self) {
        let value = self.value.fetch_add(1, Ordering::Relaxed).saturating_add(1);
        self.changes.send_replace(value);
    }

    async fn wait_for(&self, minimum: u64) {
        let mut changes = self.changes.subscribe();
        loop {
            if *changes.borrow_and_update() >= minimum {
                return;
            }
            if changes.changed().await.is_err() {
                return;
            }
        }
    }
}
