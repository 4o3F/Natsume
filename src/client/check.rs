use std::fs::OpenOptions;
use std::process::{Command, Stdio};

#[cfg(not(target_os = "windows"))]
fn can_sudo_help(command: &str) -> bool {
    let full_cmd = format!("sudo -n {} --help", command);
    Command::new("sh")
        .arg("-c")
        .arg(&full_cmd)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

pub fn check_permission(path: String) -> bool {
    #[cfg(not(target_os = "windows"))]
    {
        let useradd_ok = can_sudo_help("useradd");
        let userdel_ok = can_sudo_help("userdel");
        let systemctl_reload_ok = can_sudo_help("systemctl reload");
        if !useradd_ok || !userdel_ok{
            tracing::error!("No permission to ADD or DEL user!");
            return false;
        }
        if !systemctl_reload_ok  {
            tracing::error!("No permission to reload service!");
            return false;
        }
    }
    match OpenOptions::new().write(true).open(path) {
        Ok(_) => true,
        Err(_) => false,
    }
}

fn check_caddy() -> bool {
    Command::new("which")
        .arg("caddy")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

pub fn check_caddy_active() -> bool {
    let output = Command::new("systemctl")
        .arg("is-active")
        .arg("caddy")
        .output()
        .expect("failed to execute process");
    let status = String::from_utf8_lossy(&output.stdout).trim().to_string();
    status.as_str() == "active"
}

pub fn check_prerequisite() -> bool {
    check_caddy() && check_caddy_active()
}
