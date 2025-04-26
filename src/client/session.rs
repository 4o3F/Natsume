use std::{
    fs::{OpenOptions, read_to_string, write},
    io::Write,
    process::Command,
};

use anyhow::bail;
use tracing_unwrap::{OptionExt, ResultExt};

pub fn terminate_sessions() -> anyhow::Result<()> {
    let gdm_config = "/etc/gdm3/custom.conf";
    let username = crate::GLOBAL_CONFIG
        .get()
        .expect_or_log("Global config not initialized")
        .client
        .player_user
        .clone();
    let contents = match read_to_string(gdm_config) {
        Ok(c) => c,
        Err(_) => {
            bail!("Failed to read {}", gdm_config)
        }
    };

    let filtered: Vec<String> = contents
        .lines()
        .filter(|line| {
            !line.trim().eq("AutomaticLoginEnable=true")
                && !line.trim().eq(&format!("AutomaticLogin={}", username))
        })
        .map(String::from)
        .collect();

    if let Err(e) = write(gdm_config, filtered.join("\n") + "\n") {
        bail!("Failed to write to {}: {}", gdm_config, e)
    }

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
        .arg("gdm")
        .status()
        .expect_or_log("Failed to restart gdm");
    if !status.success() {
        bail!("Failed to retart gdm")
    }
    Ok(())
}

pub fn autologin_session() -> anyhow::Result<()> {
    let gdm_config = "/etc/gdm3/custom.conf";
    let username = crate::GLOBAL_CONFIG
        .get()
        .expect_or_log("Global config not initialized")
        .client
        .player_user
        .clone();
    if let Ok(mut file) = OpenOptions::new().append(true).open(gdm_config) {
        writeln!(file, "[daemon]").expect("Failed to write to config");
        writeln!(file, "AutomaticLoginEnable=true").expect("Failed to write to config");
        writeln!(file, "AutomaticLogin={}", username).expect("Failed to write to config");
    } else {
        bail!("Failed to open {}", gdm_config);
    }

    let status = Command::new("systemctl")
        .arg("restart")
        .arg("gdm")
        .status()
        .expect_or_log("Failed to restart gdm");

    if status.success() {
        tracing::info!("gdm restarted successfully.");
        Ok(())
    } else {
        bail!("Failed to restart gdm.");
    }
}
