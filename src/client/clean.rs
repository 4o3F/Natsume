use std::process::Command;

use anyhow::{Ok, bail};
use tracing_unwrap::OptionExt;

pub fn clean_user() -> anyhow::Result<()> {
    let user_name = crate::GLOBAL_CONFIG
        .get()
        .expect_or_log("Global config not initialized")
        .client
        .player_user
        .clone();
    let user_password = crate::GLOBAL_CONFIG
        .get()
        .expect_or_log("Global config not initialized")
        .client
        .player_user_password
        .clone();

    let output = Command::new("userdel")
        .arg("-r")
        .arg(&user_name)
        .output()
        .expect("failed to execute process");

    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        bail!("Failed to delete user, stdout {} stderr {}", stdout, stderr)
    }

    let output = Command::new("useradd")
        .arg("-m")
        .arg(&user_name)
        .output()
        .expect("failed to execute process");

    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        bail!("Failed to create user, stdout {} stderr {}", stdout, stderr)
    }

    let output = Command::new("sh")
        .arg("-C")
        .arg(format!(
            "echo '{}:{}' | sudo chpasswd",
            &user_name, &user_password
        ))
        .output()
        .expect("failed to execute process");

    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        bail!("Failed to change user password, stdout {} stderr {}", stdout, stderr)
    }

    Ok(())
}
