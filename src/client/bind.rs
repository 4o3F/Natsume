use std::{
    net::IpAddr,
    os::unix::process::CommandExt,
    process::{self, Command, Stdio},
};

use anyhow::bail;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use tracing_unwrap::OptionExt;

pub struct BindOptions {
    pub id: Option<String>,
    pub prompt: bool,
    pub background: bool,
}

enum BindInput {
    Cli(String),
    Gui {
        id: String,
        desktop_env: super::desktop::DesktopSessionEnv,
    },
}

pub fn get_mac(target_ip: String) -> anyhow::Result<String> {
    if target_ip == "localhost" {
        bail!("Can't get MAC of loop addr localhost");
    }

    if let Ok(ip) = target_ip.parse::<IpAddr>()
        && ip.is_loopback()
    {
        bail!("Can't get MAC of loop addr {}", target_ip);
    }

    let full_cmd = format!(
        r#"
        ip route get {} | awk '{{for(i=1;i<=NF;i++) if($i=="dev") print $(i+1)}}' | xargs -r -I{{}} ip -o link show {{}} | awk '{{for(i=1;i<=NF;i++) if($i=="link/ether") print $(i+1)}}'
        "#,
        target_ip
    );
    tracing::debug!("Full command: {}", full_cmd);
    let output = Command::new("sh").arg("-c").arg(full_cmd).output()?;
    let errout = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if output.status.success() && !stdout.is_empty() {
        return Ok(stdout);
    }

    let err = format!(
        "exit status: {:?}\nstderr: \n{}\nstdout: \n{}",
        output.status.code(),
        errout,
        stdout
    );
    bail!(err)
}

fn get_netinfo() -> anyhow::Result<String> {
    let output = Command::new("sh").arg("-c").arg("ip addr").output()?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let errout = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if output.status.success() && !stdout.is_empty() {
        return Ok(stdout);
    }
    let err = format!(
        "exit status: {:?}\nstderr: \n{}\nstdout: \n{}",
        output.status.code(),
        errout,
        stdout
    );
    bail!(err)
}

fn validate_direct_connection(url: &String) -> anyhow::Result<bool> {
    let request_url = format!("{}/ip", url);

    let client = super::build_server_http_client()?;

    // Fetch IP from remote server
    let response = client.get(request_url).send()?;
    #[derive(Deserialize)]
    struct IP {
        ip: String,
    }
    let ip: IP;
    match response.status() {
        StatusCode::OK => {
            ip = response.json()?;
            tracing::info!("IP fetched {}", ip.ip);
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

    // Fetch IP info from local
    let netinfo = get_netinfo()?;

    Ok(netinfo.contains(ip.ip.as_str()))
}

#[derive(Serialize)]
struct RequestBody {
    mac: String,
    id: String,
    client_version: String,
}

fn send_bind_req(url: &String, id: &str, mac: &str) -> anyhow::Result<()> {
    let request_url = format!("{}/bind", url);
    let client = super::build_server_http_client()?;
    let body = RequestBody {
        mac: mac.to_string(),
        id: id.to_string(),
        client_version: version!().to_string(),
    };

    let response = client.post(request_url).json(&body).send()?;
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

    anyhow::Ok(())
}

fn perform_bind(id: &str) -> anyhow::Result<()> {
    let base_url = &crate::GLOBAL_CONFIG
        .get()
        .unwrap_or_log()
        .client
        .server_addr;

    let skip_check = crate::GLOBAL_CONFIG
        .get()
        .expect_or_log("Global config not initialized!")
        .client
        .skip_ip_check;

    if !validate_direct_connection(base_url)? {
        tracing::warn!("Real IP does not match self IP! Possibly behind a NAT!");
        if !skip_check {
            bail!("IP mismatch, stop processing!")
        }
        tracing::warn!("Skip check enabled, will continue procedding")
    }

    let parsed_url = reqwest::Url::parse(base_url)
        .map_err(|_| anyhow::Error::msg("Failed to parse base URL"))?;
    let target_ip = parsed_url
        .host_str()
        .expect_or_log("Failed to get host str from base URL")
        .to_string();
    let mac = get_mac(target_ip)?;
    tracing::info!("Current device MAC is {}", mac);

    match send_bind_req(base_url, &id, &mac) {
        Ok(_) => {
            tracing::info!("Bind success!");
            Ok(())
        }
        Err(e) => {
            tracing::error!("Bind FAILED!");
            tracing::error!("Error: {:#}", e);
            Err(e)
        }
    }
}

fn spawn_background_bind(config_path: &str) -> anyhow::Result<()> {
    let exe = std::env::current_exe()?;
    Command::new(exe)
        .arg("--config")
        .arg(config_path)
        .arg("bind")
        .arg("--prompt")
        .arg("--_bg")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .setsid(true)
        .spawn()?;

    tracing::info!("Background bind process spawned");
    Ok(())
}

pub fn bind_ip(options: BindOptions, config_path: &str) -> anyhow::Result<()> {
    if options.prompt && !options.background {
        // Verify prerequisites in parent so failures are visible to pssh/SSH
        let player_user = crate::GLOBAL_CONFIG
            .get()
            .expect_or_log("Global config not initialized")
            .client
            .player_user
            .clone();
        super::desktop::ensure_prompt_prerequisites(&player_user)?;

        spawn_background_bind(config_path)?;
        process::exit(0);
    }

    let bind_input = match (options.id, options.prompt, options.background) {
        (Some(id), _, _) => BindInput::Cli(id),
        (None, true, true) => {
            let player_user = crate::GLOBAL_CONFIG
                .get()
                .expect_or_log("Global config not initialized")
                .client
                .player_user
                .clone();
            let prompt_result = super::desktop::prompt_bind_id(&player_user)?;
            BindInput::Gui {
                id: prompt_result.id,
                desktop_env: prompt_result.desktop_env,
            }
        }
        _ => bail!("Bind ID not provided"),
    };

    let player_user = crate::GLOBAL_CONFIG
        .get()
        .expect_or_log("Global config not initialized")
        .client
        .player_user
        .clone();

    match bind_input {
        BindInput::Cli(id) => perform_bind(&id),
        BindInput::Gui { id, desktop_env } => {
            let bind_result = perform_bind(&id);
            let result_text = match &bind_result {
                Ok(_) => format!("Bind succeeded for contestant ID {id}"),
                Err(err) => format!("Bind failed: {err:#}"),
            };
            super::desktop::show_bind_result(
                &player_user,
                &desktop_env,
                bind_result.is_ok(),
                &result_text,
            );
            bind_result
        }
    }
}
