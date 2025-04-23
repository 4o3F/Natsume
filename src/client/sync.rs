use anyhow::bail;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use tracing_unwrap::OptionExt;

#[derive(Serialize)]
struct SyncRequestBody {
    mac: String,
}

#[derive(Deserialize, Debug)]
struct SyncResponseBody {
    username: String,
    password: String,
}

pub fn sync_info() -> anyhow::Result<()> {
    let base_url = &crate::GLOBAL_CONFIG
        .get()
        .unwrap_or_log()
        .client
        .server_addr;
    let parsed_url = reqwest::Url::parse(&base_url)
        .map_err(|_| anyhow::Error::msg("Failed to parse base URL"))?;
    let target_ip = parsed_url
        .host_str()
        .expect_or_log("Failed to get host str from base URL")
        .to_string();
    let mac = super::bind::get_mac(target_ip)?;
    tracing::info!("Current device MAC is {}", mac);

    let token = crate::GLOBAL_CONFIG
        .get()
        .expect_or_log("Global config not initialized!")
        .client
        .token
        .clone();

    let request_url = format!("{}/sync", base_url);
    let client = reqwest::blocking::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()?;

    let request_body = SyncRequestBody { mac };

    let response = client
        .post(request_url)
        .header("token", token)
        .json(&request_body)
        .send()?;
    match response.status() {
        StatusCode::OK => {}
        other => {
            let error: crate::client::ErrorResponse = response.json()?;
            tracing::error!(
                "Wrong response code {}, error {} {}",
                other,
                error.msg,
                error.error
            );
            bail!("")
        }
    }
    let info: SyncResponseBody = response.json()?;
    tracing::info!("Synced info: {:?}", info);

    // TODO: write caddy file into /etc/caddy/Caddyfile
    todo!()
}
