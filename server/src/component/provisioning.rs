use tokio::sync::Mutex;

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

/// Enrollment admission authority. Every Server start creates its process-local
/// gate closed.
pub(crate) struct ProvisioningComponent {
    state: Mutex<ProvisioningWindowState>,
}

impl ProvisioningComponent {
    pub(crate) const fn new() -> Self {
        Self {
            state: Mutex::const_new(ProvisioningWindowState::Closed),
        }
    }

    pub(crate) async fn read_window(&self) -> ProvisioningWindow {
        ProvisioningWindow {
            state: *self.state.lock().await,
        }
    }

    pub(crate) async fn open_window(&self) -> ProvisioningWindow {
        self.set_window(ProvisioningWindowState::Open).await
    }

    pub(crate) async fn close_window(&self) -> ProvisioningWindow {
        self.set_window(ProvisioningWindowState::Closed).await
    }

    async fn set_window(&self, target: ProvisioningWindowState) -> ProvisioningWindow {
        *self.state.lock().await = target;
        ProvisioningWindow { state: target }
    }
}
