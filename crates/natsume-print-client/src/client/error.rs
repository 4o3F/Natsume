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
    #[snafu(display("print client configuration encoding failed"))]
    ConfigurationWrite { source: toml::ser::Error },
    #[snafu(display("the client application control channel closed"))]
    ControlClosed,
    #[snafu(display("print client window operation failed"))]
    Gui { source: slint::PlatformError },
    #[cfg(windows)]
    #[snafu(display("print client tray operation failed"))]
    Tray { source: tray_icon::Error },
    #[cfg(windows)]
    #[snafu(display("print client tray menu operation failed"))]
    Menu { source: tray_icon::menu::Error },
    #[cfg(windows)]
    #[snafu(display("print client tray icon is invalid"))]
    Icon { source: tray_icon::BadIcon },
    #[snafu(display("print client application thread stopped unexpectedly"))]
    ApplicationThread,
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
    #[snafu(display("native print operation failed: {source}"))]
    Print { source: crate::PrintError },
    #[snafu(display("print worker stopped unexpectedly"))]
    Worker { source: tokio::task::JoinError },
    #[cfg(windows)]
    #[snafu(display("Windows credential storage failed"))]
    Credential { source: windows::core::Error },
    #[cfg(windows)]
    #[snafu(display("Windows clipboard operation failed"))]
    Clipboard { source: arboard::Error },
}

pub(super) type Result<T> = std::result::Result<T, ClientError>;

#[cfg(test)]
mod tests;
