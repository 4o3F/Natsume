//! Pure certificate-source selection rules for first-install setup.

/// The available Control CA material for this first installation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlCaChoice {
    /// A newly generated CA and its local private key.
    Generated,
    /// An existing CA certificate and its imported private key.
    ImportedWithKey,
    /// An existing CA certificate whose private key remains offline.
    CertificateOnly,
}

/// A way to obtain the Server TLS identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerTlsOption {
    /// Issue a new leaf with the available CA key.
    Issue,
    /// Import an already issued leaf and its private key.
    Import,
    /// Export a signing request to an offline issuer.
    SigningRequest,
}

/// A non-secret explanation for a disabled TLS source choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unavailability {
    /// Issuance requires the CA private key, which is offline.
    ControlKeyOffline,
    /// A newly generated CA has never issued an existing certificate.
    NewCaHasNoExistingCertificates,
    /// A new CA's key is already available for local issuance.
    NewCaKeyAvailable,
}

/// Lists all choices and their availability, in Issue/Import/SigningRequest order.
#[must_use]
pub fn server_tls_options(
    control: ControlCaChoice,
) -> Vec<(ServerTlsOption, Result<(), Unavailability>)> {
    use ControlCaChoice::{CertificateOnly, Generated, ImportedWithKey};
    use ServerTlsOption::{Import, Issue, SigningRequest};
    use Unavailability::{ControlKeyOffline, NewCaHasNoExistingCertificates, NewCaKeyAvailable};

    match control {
        Generated => vec![
            (Issue, Ok(())),
            (Import, Err(NewCaHasNoExistingCertificates)),
            (SigningRequest, Err(NewCaKeyAvailable)),
        ],
        ImportedWithKey => vec![(Issue, Ok(())), (Import, Ok(())), (SigningRequest, Ok(()))],
        CertificateOnly => vec![
            (Issue, Err(ControlKeyOffline)),
            (Import, Ok(())),
            (SigningRequest, Ok(())),
        ],
    }
}

/// Defaults to local issuance when a key is available, otherwise to an offline CSR.
#[must_use]
pub fn default_server_tls_option(control: ControlCaChoice) -> ServerTlsOption {
    match control {
        ControlCaChoice::Generated | ControlCaChoice::ImportedWithKey => ServerTlsOption::Issue,
        ControlCaChoice::CertificateOnly => ServerTlsOption::SigningRequest,
    }
}
