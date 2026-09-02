use snafu::Snafu;

use crate::server_state::ServerStateError;

/// Redacted failure while starting the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Snafu)]
pub enum CommandError {
    #[snafu(display("server configuration failed"))]
    Configuration,
    #[snafu(display("structured logging startup failed"))]
    Logging,
    #[snafu(display("database startup failed"))]
    Database,
    #[snafu(display("site configuration startup failed"))]
    SiteConfiguration,
    #[snafu(display("vault startup failed"))]
    Vault,
    #[snafu(display("web assets startup failed"))]
    WebAssets,
    #[snafu(display("TLS startup failed"))]
    Tls,
    #[snafu(display("Origin CA startup failed"))]
    OriginCa,
    #[snafu(display("Origin CA issuing certificate and packaged trust root differ"))]
    OriginCaTrustRootMismatch,
    #[snafu(display("HTTP serving failed"))]
    Http,
    #[snafu(display("shutdown signal setup failed"))]
    Signal,
    #[snafu(display("server bootstrap failed"))]
    Bootstrap,
    #[snafu(display("operator password reset failed"))]
    PasswordReset,
}

impl From<ServerStateError> for CommandError {
    fn from(error: ServerStateError) -> Self {
        match error {
            ServerStateError::Configuration => Self::Configuration,
            ServerStateError::SiteConfiguration => Self::SiteConfiguration,
            ServerStateError::Vault => Self::Vault,
            ServerStateError::OriginCa => Self::OriginCa,
            ServerStateError::OriginCaTrustRootMismatch => Self::OriginCaTrustRootMismatch,
        }
    }
}
