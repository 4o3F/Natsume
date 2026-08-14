use snafu::Snafu;

/// Redacted failure while starting the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Snafu)]
pub enum AppError {
    #[snafu(display("server configuration failed"))]
    Configuration,
    #[snafu(display("structured logging startup failed"))]
    Logging,
    #[snafu(display("database startup failed"))]
    Database,
    #[snafu(display("vault startup failed"))]
    Vault,
    #[snafu(display("TLS startup failed"))]
    Tls,
    #[snafu(display("HTTP serving failed"))]
    Http,
    #[snafu(display("shutdown signal setup failed"))]
    Signal,
    #[snafu(display("server bootstrap failed"))]
    Bootstrap,
}
