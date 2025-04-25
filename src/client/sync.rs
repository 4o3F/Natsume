use std::{fs::OpenOptions, io::Write, process::Command};

use anyhow::bail;
use base64::{Engine, prelude::BASE64_STANDARD};
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

fn fetch_info() -> anyhow::Result<SyncResponseBody> {
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

            bail!(
                "Wrong response code {}, error {} {}",
                other,
                error.msg,
                error.error
            )
        }
    }
    let info: SyncResponseBody = response.json()?;
    Ok(info)
}

fn format_caddyfile(username: String, password: String) -> String {
    let domjudge_addr = crate::GLOBAL_CONFIG
        .get()
        .expect_or_log("Global config not initialized")
        .client
        .domjudge_addr
        .clone();
    let mut encoded_password = String::new();
    BASE64_STANDARD.encode_string(password, &mut encoded_password);
    format!(
        r#"
{{
	admin localhost:20190
	auto_https off
}}

:80 {{
	@autologin path /login*

	handle @autologin {{
		reverse_proxy {} {{
			header_up X-DOMjudge-Login "{}"
			header_up X-DOMjudge-Pass "{}"
        }}
	}}

	handle {{
		reverse_proxy {}
	}}
}}

    "#,
        domjudge_addr, username, encoded_password, domjudge_addr
    )
}

fn reload_caddy_service() -> bool {
    let output = Command::new("sudo")
        .arg("-n")
        .arg("systemctl")
        .arg("reload")
        .arg("caddy")
        .output();
    match output {
        Ok(result) => result.status.success(),
        Err(err) => {
            tracing::error!("Failed to run systemctl reload, err {}", err);
            false
        }
    }
}

pub fn sync_info() -> anyhow::Result<()> {
    if !crate::client::check::check_caddy_active() {
        return Err(anyhow::Error::msg("Caddy service not running!"));
    }

    let info = fetch_info()?;
    // Write caddy file into /etc/caddy/Caddyfile
    let formated_caddyfile = format_caddyfile(info.username, info.password);
    let caddyfile_path = crate::GLOBAL_CONFIG
        .get()
        .expect_or_log("Global config not initialized")
        .client
        .caddyfile
        .clone();

    std::fs::remove_file(&caddyfile_path)?;

    match OpenOptions::new()
        .write(true)
        .create(true)
        .open(caddyfile_path)
    {
        Ok(mut file) => {
            file.write_all(formated_caddyfile.as_bytes())?;
        }
        Err(e) => {
            bail!("Failed to write to Caddyfile, err {}", e)
        }
    }

    if !reload_caddy_service() {
        bail!("Failed to reload caddy service!")
    }
    Ok(())
}
