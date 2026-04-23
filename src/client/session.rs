use std::{
    fs::{OpenOptions, read_to_string, write},
    io::Write,
    process::Command,
};

use anyhow::bail;
use tracing_unwrap::{OptionExt, ResultExt};

pub fn terminate_sessions() -> anyhow::Result<()> {
    let lightdm_config = "/etc/lightdm/lightdm.conf";
    let username = crate::GLOBAL_CONFIG
        .get()
        .expect_or_log("Global config not initialized")
        .client
        .player_user
        .clone();
    let contents = match read_to_string(lightdm_config) {
        Ok(c) => c,
        Err(_) => {
            bail!("Failed to read {}", lightdm_config)
        }
    };

    let filtered: Vec<String> = contents
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            trimmed != format!("autologin-user={username}")
                && trimmed != "autologin-user-timeout=0"
        })
        .map(String::from)
        .collect();

    if let Err(e) = write(lightdm_config, filtered.join("\n") + "\n") {
        bail!("Failed to write to {}: {}", lightdm_config, e)
    }

    // Check if user is logged in
    let who_output = Command::new("who")
        .output()
        .expect_or_log("Failed to execute 'who'");
    let output_str = String::from_utf8_lossy(&who_output.stdout);
    let user_logged_in = output_str.lines().any(|line| line.starts_with(&username));

    if user_logged_in {
        let status = Command::new("loginctl")
            .arg("terminate-user")
            .arg(&username)
            .status()
            .expect_or_log("Failed to terminate user session");
        if !status.success() {
            bail!("Failed to terminate user session for {}", username);
        }

        let status = Command::new("systemctl")
            .arg("restart")
            .arg("lightdm")
            .status()
            .expect_or_log("Failed to restart lightdm");
        if !status.success() {
            bail!("Failed to restart lightdm")
        }
        Ok(())
    } else {
        tracing::info!(
            "User {} is not currently logged in. Skipping terminate.",
            username
        );
        Ok(())
    }
}

pub fn autologin_session() -> anyhow::Result<()> {
    let lightdm_config = "/etc/lightdm/lightdm.conf";
    let username = crate::GLOBAL_CONFIG
        .get()
        .expect_or_log("Global config not initialized")
        .client
        .player_user
        .clone();
    if let Ok(mut file) = OpenOptions::new().append(true).open(lightdm_config) {
        writeln!(file, "[Seat:*]").expect("Failed to write to config");
        writeln!(file, "autologin-user={}", username).expect("Failed to write to config");
        writeln!(file, "autologin-user-timeout=0").expect("Failed to write to config");
    } else {
        bail!("Failed to open {}", lightdm_config);
    }

    let status = Command::new("systemctl")
        .arg("restart")
        .arg("lightdm")
        .status()
        .expect_or_log("Failed to restart lightdm");

    if status.success() {
        tracing::info!("lightdm restarted successfully.");
        Ok(())
    } else {
        bail!("Failed to restart lightdm.");
    }
}
