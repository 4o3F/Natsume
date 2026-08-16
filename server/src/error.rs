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
    #[snafu(display("site configuration startup failed"))]
    SiteConfiguration,
    #[snafu(display("provisioning window revision overflow prevented startup"))]
    ProvisioningRevisionOverflow,
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
