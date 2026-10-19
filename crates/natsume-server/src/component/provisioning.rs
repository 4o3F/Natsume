use tokio::sync::watch;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProvisioningWindowState {
    Closed,
    Open,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProvisioningWindow {
    state: ProvisioningWindowState,
}

impl ProvisioningWindow {
    pub(crate) const fn is_open(self) -> bool {
        matches!(self.state, ProvisioningWindowState::Open)
    }
}

/// Automatic Enrollment approval policy. Every Server start requires manual review.
pub(crate) struct ProvisioningComponent {
    state: watch::Sender<ProvisioningWindow>,
}

impl ProvisioningComponent {
    pub(crate) fn new() -> Self {
        Self {
            state: watch::Sender::new(ProvisioningWindow {
                state: ProvisioningWindowState::Closed,
            }),
        }
    }

    pub(crate) fn read_window(&self) -> ProvisioningWindow {
        *self.state.borrow()
    }

    pub(crate) fn subscribe(&self) -> watch::Receiver<ProvisioningWindow> {
        self.state.subscribe()
    }

    pub(crate) fn open_window(&self) -> ProvisioningWindow {
        self.set_window(ProvisioningWindowState::Open)
    }

    pub(crate) fn close_window(&self) -> ProvisioningWindow {
        self.set_window(ProvisioningWindowState::Closed)
    }

    fn set_window(&self, target: ProvisioningWindowState) -> ProvisioningWindow {
        let window = ProvisioningWindow { state: target };
        self.state.send_replace(window);
        window
    }
}
