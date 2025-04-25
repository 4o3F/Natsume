use std::time::Duration;

use anyhow::bail;
use reqwest::StatusCode;
use serde::Serialize;
use tracing_unwrap::OptionExt;

use crate::client::bind;

#[derive(Serialize)]
struct ReportRequest {
    mac: String,
    synced: bool,
}

pub fn send_report(synced: bool) -> anyhow::Result<()> {
    let base_url = &crate::GLOBAL_CONFIG
        .get()
        .expect_or_log("Global config not initialized")
        .client
        .server_addr;

    let parsed_url = reqwest::Url::parse(&base_url)
        .map_err(|_| anyhow::Error::msg("Failed to parse base URL"))?;
    let target_ip = parsed_url
        .host_str()
        .expect_or_log("Failed to get host str from base URL")
        .to_string();

    let client = reqwest::blocking::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()?;

    let request_url = format!("{}/report", base_url);

    let mac = bind::get_mac(target_ip)?;
    let response = client
        .post(&request_url)
        .json(&ReportRequest {
            mac: mac.clone(),
            synced,
        })
        .send()?;

    match response.status() {
        StatusCode::OK => {
            tracing::info!("Report MAC {} synced {} successful!", mac, synced);
            Ok(())
        }
        other => {
            let error: crate::client::ErrorResponse = response.json()?;
            bail!(
                "Wrong response code {}, error {} {}",
                other,
                error.msg,
                error.error
            )
        }
    }
}

pub fn do_monitor() -> anyhow::Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async {
        let forever: tokio::task::JoinHandle<Result<(), anyhow::Error>> =
            tokio::task::spawn(async {
                let mut interval = tokio::time::interval(Duration::from_secs(60 * 1));
                loop {
                    interval.tick().await;
                    let result = send_report(false);
                    match result {
                        Ok(_) => {}
                        Err(err) => {
                            tracing::error!("Error sending report {}", err);
                        }
                    }
                }
            });
        forever.await??;
        Ok(())
    })
}
