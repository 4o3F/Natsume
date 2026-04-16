mod bind;
mod check;
mod desktop;
mod clean;
mod monitor;
mod session;
mod sync;

pub use bind::{BindOptions, bind_ip};
pub use check::{check_permission, check_prerequisite};
pub use clean::clean_user;
pub use monitor::do_monitor;
pub use session::{autologin_session, terminate_sessions};
pub use sync::sync_info;

use serde::Deserialize;

fn build_server_http_client() -> anyhow::Result<reqwest::blocking::Client> {
    let config = crate::GLOBAL_CONFIG
        .get()
        .ok_or_else(|| anyhow::Error::msg("Global config not initialized"))?;
    let client_config = &config.client;
    let ca_cert_path = &client_config.tls_ca_cert_path;
    let ca_cert_pem = std::fs::read(ca_cert_path).map_err(|err| {
        anyhow::Error::msg(format!(
            "Failed to read CA certificate {ca_cert_path}: {err}"
        ))
    })?;
    let ca_cert = reqwest::Certificate::from_pem(&ca_cert_pem).map_err(|err| {
        anyhow::Error::msg(format!(
            "Failed to parse CA certificate PEM from {ca_cert_path}: {err}"
        ))
    })?;

    reqwest::blocking::Client::builder()
        .tls_certs_only(vec![ca_cert])
        .tls_danger_accept_invalid_hostnames(true)
        .https_only(true)
        .build()
        .map_err(|err| {
            anyhow::Error::msg(format!(
                "Failed to build HTTPS client using CA certificate {ca_cert_path}: {err:?}"
            ))
        })
}

#[derive(Deserialize)]
struct ErrorResponse {
    msg: String,
    error: String,
}
