use snafu::Snafu;

#[derive(Debug, Snafu)]
pub(crate) enum TestError {
    #[snafu(context(false), display("test filesystem operation failed"))]
    Io { source: std::io::Error },
    #[snafu(context(false), display("test database startup failed"))]
    Database { source: crate::db::DatabaseError },
    #[snafu(context(false), display("test database operation failed"))]
    Persistence { source: crate::db::PersistenceError },
    #[snafu(context(false), display("test query failed"))]
    Query { source: diesel::result::Error },
    #[snafu(context(false), display("test connection failed"))]
    Connection { source: diesel::ConnectionError },
    #[snafu(context(false), display("test operator operation failed"))]
    Operator {
        source: crate::component::operator::OperatorError,
    },
    #[snafu(context(false), display("test import operation failed"))]
    Import {
        source: crate::component::import::ImportError,
    },
    #[snafu(context(false), display("test print operation failed"))]
    Print {
        source: crate::component::print::PrintError,
    },
    #[snafu(context(false), display("test vault operation failed"))]
    Vault { source: crate::vault::VaultError },
    #[snafu(context(false), display("test configuration failed"))]
    Config { source: crate::config::ConfigError },
    #[snafu(context(false), display("test TLS operation failed"))]
    Tls { source: crate::tls::TlsError },
    #[snafu(context(false), display("test HTTP request construction failed"))]
    Request { source: axum::http::Error },
    #[snafu(context(false), display("test HTTP body failed"))]
    Body { source: axum::Error },
    #[snafu(context(false), display("test JSON operation failed"))]
    Json { source: serde_json::Error },
    #[snafu(context(false), display("test UTF-8 decoding failed"))]
    Utf8 { source: std::str::Utf8Error },
    #[snafu(context(false), display("test task failed"))]
    Join {
        #[snafu(source(from(tokio::task::JoinError, crate::task::TaskFailure::from)))]
        source: crate::task::TaskFailure,
    },
    #[snafu(context(false), display("test timed out"))]
    Timeout { source: tokio::time::error::Elapsed },
    #[snafu(context(false), display("test Device operation failed"))]
    Device {
        source: crate::component::device::DeviceError,
    },
    #[snafu(context(false), display("test Activation operation failed"))]
    Activation {
        source: crate::component::device::ActivationError,
    },
    #[snafu(context(false), display("test EnrollmentStart operation failed"))]
    EnrollmentStart {
        source: crate::component::device::EnrollmentStartError,
    },
    #[snafu(context(false), display("test EnrollmentApproval operation failed"))]
    EnrollmentApproval {
        source: crate::component::device::EnrollmentApprovalError,
    },
    #[snafu(context(false), display("test Convergence operation failed"))]
    Convergence {
        source: crate::device_control::DeviceConvergenceError,
    },
    #[snafu(context(false), display("test TargetSubmission operation failed"))]
    TargetSubmission {
        source: crate::component::target_submission::TargetSubmissionError,
    },
    #[snafu(context(false), display("test State operation failed"))]
    State {
        source: crate::server_state::ServerStateError,
    },
    #[snafu(context(false), display("test Address operation failed"))]
    Address { source: std::net::AddrParseError },
    #[snafu(context(false), display("test Header operation failed"))]
    Header {
        source: axum::http::header::InvalidHeaderValue,
    },
    #[snafu(context(false), display("test HeaderText operation failed"))]
    HeaderText {
        source: axum::http::header::ToStrError,
    },
    #[snafu(context(false), display("test Protocol operation failed"))]
    Protocol { source: prost::DecodeError },
    #[snafu(context(false), display("test Socket operation failed"))]
    Socket {
        source: tokio_tungstenite::tungstenite::Error,
    },
    #[snafu(context(false), display("test Receive operation failed"))]
    Receive {
        source: tokio::sync::oneshot::error::RecvError,
    },
    #[snafu(context(false), display("test Acquire operation failed"))]
    Acquire { source: tokio::sync::AcquireError },
    #[snafu(context(false), display("test Watch operation failed"))]
    Watch {
        source: tokio::sync::watch::error::SendError<bool>,
    },
    #[snafu(context(false), display("test Uuid operation failed"))]
    Uuid { source: uuid::Error },
    #[snafu(context(false), display("test Integer operation failed"))]
    Integer { source: std::num::TryFromIntError },
    #[snafu(context(false), display("test Phc operation failed"))]
    Phc {
        source: argon2::password_hash::phc::Error,
    },
    #[snafu(context(false), display("test Hash operation failed"))]
    Hash {
        source: argon2::password_hash::Error,
    },
    #[snafu(context(false), display("test Argon operation failed"))]
    Argon { source: argon2::Error },
    #[snafu(context(false), display("test X509 operation failed"))]
    X509 {
        source: x509_parser::error::X509Error,
    },
    #[snafu(context(false), display("test Der operation failed"))]
    Der {
        source: x509_parser::asn1_rs::Err<x509_parser::error::X509Error>,
    },
    #[snafu(context(false), display("test Zip operation failed"))]
    Zip { source: zip::result::ZipError },
    #[snafu(context(false), display("test Image operation failed"))]
    Image { source: image::ImageError },
    #[snafu(context(false), display("test Domjudge operation failed"))]
    Domjudge {
        source: natsume_print_domjudge::Error,
    },
    #[snafu(context(false), display("test Format operation failed"))]
    Format { source: std::fmt::Error },
    #[snafu(context(false), display("test Support operation failed"))]
    Support {
        #[snafu(source(from(crate::http::tests::SupportFailure, Box::new)))]
        source: Box<crate::http::tests::SupportFailure>,
    },
    #[snafu(context(false), display("test TlsFixture operation failed"))]
    TlsFixture {
        #[snafu(source(from(crate::tls::tests::TestSupportError, Box::new)))]
        source: Box<crate::tls::tests::TestSupportError>,
    },
    #[snafu(context(false), display("test TlsTest operation failed"))]
    TlsTest {
        #[snafu(source(from(crate::tls::tests::TestFailure, Box::new)))]
        source: Box<crate::tls::tests::TestFailure>,
    },
    #[snafu(context(false), display("test LoggingTest operation failed"))]
    LoggingTest {
        #[snafu(source(from(crate::logging::tests::TestFailure, Box::new)))]
        source: Box<crate::logging::tests::TestFailure>,
    },
    #[snafu(display("test cryptographic fixture was rejected"))]
    CryptoFixture,
    #[snafu(context(false), display("test gateway operation failed"))]
    Gateway {
        source: crate::component::gateway::GatewayError,
    },
    #[snafu(display("test text fixture was rejected"))]
    TextFixture,
    #[snafu(display("test thread panicked"))]
    ThreadPanic,
    #[snafu(display("test environment was unavailable"))]
    Environment,
    #[snafu(display("test notification was unavailable"))]
    Notification,
    #[snafu(display("test telemetry shutdown failed"))]
    Telemetry,
    #[snafu(display("test fixture is missing {expected}"))]
    Missing { expected: &'static str },
}

impl<E: Into<TestError>> From<crate::db::TransactionError<E>> for TestError {
    fn from(error: crate::db::TransactionError<E>) -> Self {
        match error {
            crate::db::TransactionError::Operation(error) => error.into(),
            crate::db::TransactionError::Persistence(source) => Self::Persistence { source },
        }
    }
}

impl From<std::convert::Infallible> for TestError {
    fn from(error: std::convert::Infallible) -> Self {
        match error {}
    }
}
impl From<rcgen::Error> for TestError {
    fn from(_: rcgen::Error) -> Self {
        // Certificate-library diagnostics can retain key or certificate context.
        Self::CryptoFixture
    }
}
impl From<rustls::Error> for TestError {
    fn from(_: rustls::Error) -> Self {
        // Rustls diagnostics can retain certificate or key context.
        Self::CryptoFixture
    }
}
impl From<std::string::FromUtf8Error> for TestError {
    fn from(_: std::string::FromUtf8Error) -> Self {
        // This error owns its rejected buffer, which may contain fixture credentials.
        Self::TextFixture
    }
}
impl From<std::env::VarError> for TestError {
    fn from(_: std::env::VarError) -> Self {
        // Non-Unicode environment errors own the variable's possibly secret value.
        Self::Environment
    }
}
impl From<Box<dyn std::any::Any + Send>> for TestError {
    fn from(_: Box<dyn std::any::Any + Send>) -> Self {
        // Panic payloads are arbitrary and may contain fixture secrets.
        Self::ThreadPanic
    }
}
impl From<()> for TestError {
    fn from((): ()) -> Self {
        Self::Notification
    }
}
impl From<opentelemetry_sdk::error::OTelSdkError> for TestError {
    fn from(_: opentelemetry_sdk::error::OTelSdkError) -> Self {
        // Exporter failures can embed configured endpoint or header values.
        Self::Telemetry
    }
}

impl TestError {
    pub(crate) fn missing(expected: &'static str) -> Self {
        Self::Missing { expected }
    }
}
