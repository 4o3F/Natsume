use std::process::Command;

use anyhow::bail;
use tracing_unwrap::OptionExt;

fn get_mac(target_ip: String) -> anyhow::Result<String> {
    let full_cmd = format!(
        r#"
        ip route get {} | awk '{{for(i=1;i<=NF;i++) if($i=="dev") print $(i+1)}}' | xargs -r -I{{}} ip -o link show {{}} | awk '{{for(i=1;i<=NF;i++) if($i=="link/ether") print $(i+1)}}'
        "#,
        target_ip
    );
    let output = Command::new("sh").arg("-c").arg(full_cmd).output()?;
    let errout = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if output.status.success() {
        if !stdout.is_empty() {
            return Ok(stdout);
        }
    }

    let err = format!("errout: \n {} \n stdout: \n {}", errout, stdout);
    bail!(err)
}

pub fn bind_ip(id: String) -> anyhow::Result<()> {
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
    let mac = get_mac(target_ip)?;
    tracing::info!("Current device MAC is {}", mac);
    // let request_url = format!("{}/ip", base_url);

    // let client = reqwest::blocking::Client::new();

    // // Fetch IP from remote server
    // let response = client.get(request_url).send()?;
    // #[derive(Deserialize)]
    // struct IP {
    //     ip: String,
    // }
    // let ip: IP;
    // match response.status() {
    //     StatusCode::OK => {
    //         ip = response.json()?;
    //         tracing::info!("IP fetched {}", ip.ip);
    //     }
    //     other => {
    //         tracing::error!("Wrong response code {}", other);
    //     }
    // }

    todo!()
}
