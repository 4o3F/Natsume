use std::fs::OpenOptions;

#[cfg(not(target_os = "windows"))]
fn can_sudo_help(command: &str) -> bool {
    use std::process::{Command, Stdio};
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
        if !useradd_ok || !userdel_ok {
            tracing::error!("No permission to ADD or DEL user!");
            return false;
        }
    }
    match OpenOptions::new().write(true).open(path) {
        Ok(_) => true,
        Err(_) => false,
    }
}
