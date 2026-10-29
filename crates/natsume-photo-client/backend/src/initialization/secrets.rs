//! Credential adapter contract; slots cannot be confused with Server origins.
use super::ClientId;
use snafu::Snafu;
use zeroize::Zeroizing;

/// Closed set of initialization secrets stored outside the workspace.
#[derive(Debug, Clone, Copy)]
pub enum SecretKind {
    Signing,
    Enrollment,
}

/// A credential belonging to one local client and one initialization purpose.
#[derive(Debug, Clone)]
pub struct SecretSlot {
    client: ClientId,
    kind: SecretKind,
}
impl SecretSlot {
    /// Selects a credential without accepting a free-form slot string.
    #[must_use]
    pub fn new(client: ClientId, kind: SecretKind) -> Self {
        Self { client, kind }
    }
    /// Stable adapter key; never contains a credential value.
    #[must_use]
    pub fn storage_key(&self) -> String {
        let kind = match self.kind {
            SecretKind::Signing => "signing",
            SecretKind::Enrollment => "enrollment",
        };
        format!("{}/{kind}", self.client)
    }
}
/// Credential storage failures are independent of HTTP or workspace errors.
#[derive(Debug, Snafu)]
pub enum CredentialError {
    #[snafu(
        display("Windows credential vault operation failed"),
        visibility(pub(super))
    )]
    Windows { source: ::windows::core::Error },
    #[snafu(display("credential adapter is unavailable"))]
    Unavailable,
}
/// Result returned by platform credential adapters.
pub type CredentialResult<T> = std::result::Result<T, CredentialError>;

/// Platform storage must durably save a value before returning success.
pub trait Secrets: Clone + Send + Sync + 'static {
    /// Reads without logging; absence differs from failure.
    /// # Errors
    /// Reports a credential-adapter failure.
    fn load(&self, origin: &str, slot: &SecretSlot) -> CredentialResult<Option<Zeroizing<String>>>;
    /// Saves under a typed initialization slot.
    /// # Errors
    /// Reports a credential-adapter failure.
    fn save(&self, origin: &str, slot: &SecretSlot, value: &str) -> CredentialResult<()>;
}
