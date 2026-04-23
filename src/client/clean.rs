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

    let user_home_output = Command::new("getent")
        .arg("passwd")
        .arg(&user_name)
        .output()
        .expect("failed to execute process");

    if !user_home_output.status.success() {
        let stdout = String::from_utf8_lossy(&user_home_output.stdout)
            .trim()
            .to_string();
        let stderr = String::from_utf8_lossy(&user_home_output.stderr)
            .trim()
            .to_string();
        bail!("Failed to lookup user home, stdout {} stderr {}", stdout, stderr)
    }

    let user_home = String::from_utf8_lossy(&user_home_output.stdout)
        .trim()
        .split(':')
        .nth(5)
        .map(str::to_string)
        .unwrap_or_else(|| format!("/home/{user_name}"));

    for mount_point in [
        format!("{user_home}/.vscode/extensions"),
        format!("{user_home}/.vscode/extensions-root"),
    ] {
        let _ = Command::new("umount")
            .arg("-f")
            .arg(&mount_point)
            .output()
            .expect("failed to execute process");
    }

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
        .arg("-c")
        .arg(format!(
            "echo '{}:{}' | sudo chpasswd",
            &user_name, &user_password
        ))
        .output()
        .expect("failed to execute process");

    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        bail!(
            "Failed to change user password, stdout {} stderr {}",
            stdout,
            stderr
        )
    }

    Ok(())
}
