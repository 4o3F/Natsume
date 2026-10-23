use snafu::Snafu;

#[derive(Debug, Snafu)]
#[snafu(visibility(pub(super)))]
pub enum ClientError {
    #[snafu(display("invalid print client configuration or record: {field}"))]
    Invalid { field: &'static str },
    #[snafu(display("print client storage operation failed: {operation}"))]
    Storage {
        operation: &'static str,
        source: std::io::Error,
    },
    #[snafu(display("print client record encoding failed"))]
    Record { source: serde_json::Error },
    #[snafu(display("print client configuration parsing failed"))]
    Configuration { source: toml::de::Error },
    #[snafu(display("print client request failed"))]
    Transport { source: reqwest::Error },
    #[snafu(display("print server rejected the operation (HTTP {status})"))]
    Rejected { status: u16 },
    #[snafu(display("print server response is inconsistent"))]
    Response,
    #[snafu(display("local print state requires operator attention: {reason}"))]
    Attention { reason: &'static str },
    #[snafu(display("execution permission was not confirmed before the first page"))]
    Permission,
    #[snafu(display("native print operation failed"))]
    Print { source: crate::PrintError },
    #[snafu(display("print worker stopped unexpectedly"))]
    Worker { source: tokio::task::JoinError },
    #[cfg(windows)]
    #[snafu(display("Windows credential storage failed"))]
    Credential { source: windows::core::Error },
}

pub(super) type Result<T> = std::result::Result<T, ClientError>;
