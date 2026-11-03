use snafu::Snafu;

pub(crate) type Result<T> = std::result::Result<T, Error>;

/// Typed failures. Response bodies and credentials are never stored in errors.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
pub(crate) enum Error {
    #[snafu(display("local file operation failed"))]
    Io { source: std::io::Error },
    #[snafu(display("fleet state directory is locked"))]
    Lock { source: std::fs::TryLockError },
    #[snafu(display("size is outside the supported integer range"))]
    Size { source: std::num::TryFromIntError },
    #[snafu(display("invalid trust root PEM"))]
    Pem {
        source: rustls_pki_types::pem::Error,
    },
    #[snafu(display("invalid server URL"))]
    Url { source: url::ParseError },
    #[snafu(display("invalid TLS server name"))]
    ServerName {
        source: rustls_pki_types::InvalidDnsNameError,
    },
    #[snafu(display("invalid HTTP header"))]
    Header {
        source: reqwest::header::InvalidHeaderValue,
    },
    #[snafu(display("invalid X.509 encoding"))]
    X509 {
        source: x509_parser::nom::Err<x509_parser::error::X509Error>,
    },
    #[snafu(display("scenario TOML is invalid"))]
    Toml { source: toml::de::Error },
    #[snafu(display("invalid scenario duration"))]
    Duration { source: crate::cli::DurationError },
    #[snafu(display("XLSX archive generation failed"))]
    Zip { source: zip::result::ZipError },
    #[snafu(display("WebP generation failed"))]
    Image { source: image::ImageError },
    #[snafu(display("operator request {route} returned HTTP {status}"))]
    Status { route: &'static str, status: u16 },
    #[snafu(display("network transport failed"))]
    Transport { source: std::io::Error },
    #[snafu(display("fleet state encoding is invalid"))]
    Json { source: serde_json::Error },
    #[snafu(display("invalid configuration or durable state: {code}"))]
    Invalid { code: &'static str },
    #[snafu(display("WebSocket operation failed"))]
    WebSocket {
        source: tokio_tungstenite::tungstenite::Error,
    },
    #[snafu(display("TLS configuration failed"))]
    Tls { source: rustls::Error },
    #[snafu(display("HTTPS operation failed"))]
    Http { source: reqwest::Error },
    #[snafu(display("protobuf decoding failed"))]
    Decode { source: prost::DecodeError },
    #[snafu(display("gateway key or CSR generation failed"))]
    Certificate { source: rcgen::Error },
    #[snafu(display("operation exceeded its deadline"))]
    Timeout { source: tokio::time::error::Elapsed },
    #[snafu(display("worker terminated"))]
    Join { source: tokio::task::JoinError },
    #[snafu(display("metrics writer stopped"))]
    Metrics {
        source: tokio::sync::mpsc::error::TrySendError<crate::metrics::Event>,
    },
    #[snafu(display("peer ended the connection: {code}"))]
    Peer { code: &'static str },
}

impl Error {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::Io { .. } => "IO",
            Self::Transport { source } => {
                if source
                    .get_ref()
                    .and_then(|e| e.downcast_ref::<rustls::Error>())
                    .is_some()
                {
                    "TLS_HANDSHAKE"
                } else if source.kind() == std::io::ErrorKind::ConnectionRefused {
                    "TCP_REFUSED"
                } else {
                    "TRANSPORT"
                }
            }
            Self::Json { .. } => "STATE_ENCODING",
            Self::Invalid { code } | Self::Peer { code } => code,
            Self::WebSocket { .. } => "WEBSOCKET",
            Self::Tls { .. } => "TLS",
            Self::Http { .. } => "HTTPS",
            Self::Decode { .. } => "PROTOBUF",
            Self::Certificate { .. } => "CSR",
            Self::Timeout { .. } => "TIMEOUT",
            Self::Join { .. } => "WORKER",
            Self::Metrics { .. } => "METRICS",
            Self::Lock { .. } => "STATE_DIRECTORY_LOCKED",
            Self::Size { .. } => "SIZE",
            Self::Pem { .. } => "TRUST_ROOT",
            Self::Url { .. } => "SERVER_ORIGIN",
            Self::ServerName { .. } => "SERVER_NAME",
            Self::Header { .. } => "HEADER",
            Self::X509 { .. } => "X509",
            Self::Toml { .. } => "SCENARIO",
            Self::Duration { .. } => "DURATION",
            Self::Zip { .. } => "XLSX",
            Self::Image { .. } => "WEBP",
            Self::Status { .. } => "HTTP_STATUS",
        }
    }

    pub(crate) const fn fatal(&self) -> bool {
        matches!(
            self,
            Self::Io { .. }
                | Self::Json { .. }
                | Self::Metrics { .. }
                | Self::Join { .. }
                | Self::Invalid { .. }
        )
    }
}
