use std::{
    collections::HashMap,
    fs,
    process::{Command, Output, Stdio},
};

use anyhow::{Context, bail};

const SAFE_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

pub struct DesktopSessionEnv {
    pub display: Option<String>,
    pub wayland_display: Option<String>,
    pub xdg_runtime_dir: String,
    pub dbus_session_bus_address: Option<String>,
    pub xauthority: Option<String>,
    pub home: String,
}

pub struct PromptResult {
    pub id: String,
    pub desktop_env: DesktopSessionEnv,
}

struct SessionMetadata {
    name: String,
    leader_pid: u32,
    session_type: String,
}

fn safe_command(program: &str) -> Command {
    let mut command = Command::new(program);
    command.env_clear().env("PATH", SAFE_PATH);
    command
}

fn get_command_output(mut command: Command, description: &str) -> anyhow::Result<Output> {
    let output = command
        .output()
        .with_context(|| format!("Failed to run {description}"))?;
    if output.status.success() {
        return Ok(output);
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    bail!("{description} failed: {stderr}")
}

fn lookup_user_id(player_user: &str) -> anyhow::Result<u32> {
    let output = get_command_output(
        {
            let mut command = safe_command("id");
            command.arg("-u").arg(player_user);
            command
        },
        "id -u",
    )?;
    let uid = String::from_utf8_lossy(&output.stdout).trim().to_string();
    uid.parse::<u32>()
        .with_context(|| format!("Failed to parse UID for user {player_user}"))
}

fn lookup_home_dir(player_user: &str) -> anyhow::Result<String> {
    let output = get_command_output(
        {
            let mut command = safe_command("getent");
            command.arg("passwd").arg(player_user);
            command
        },
        "getent passwd",
    )?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let entry = stdout
        .lines()
        .next()
        .context("No passwd entry returned")?;
    let fields: Vec<&str> = entry.split(':').collect();
    if fields.len() < 6 {
        bail!("Invalid passwd entry for user {player_user}");
    }

    Ok(fields[5].to_string())
}

fn read_process_environ(pid: u32) -> anyhow::Result<HashMap<String, String>> {
    let environ_path = format!("/proc/{pid}/environ");
    let raw = fs::read(&environ_path)
        .with_context(|| format!("Failed to read environment from {environ_path}"))?;
    let mut values = HashMap::new();

    for entry in raw.split(|byte| *byte == 0).filter(|entry| !entry.is_empty()) {
        let text = String::from_utf8_lossy(entry);
        if let Some((key, value)) = text.split_once('=') {
            values.insert(key.to_string(), value.to_string());
        }
    }

    Ok(values)
}

fn parse_session_metadata(session_id: &str) -> anyhow::Result<Option<SessionMetadata>> {
    // Use Key=Value format (no --value) because loginctl does not guarantee
    // output order matches the -p argument order.
    let output = get_command_output(
        {
            let mut command = safe_command("loginctl");
            command
                .arg("show-session")
                .arg(session_id)
                .args(["-p", "Name", "-p", "Remote", "-p", "Active", "-p", "State", "-p", "Type", "-p", "Leader"]);
            command
        },
        "loginctl show-session",
    )?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut props = HashMap::new();
    for line in stdout.lines() {
        if let Some((key, value)) = line.split_once('=') {
            props.insert(key.trim().to_string(), value.trim().to_string());
        }
    }

    let name = props.get("Name").context("Missing Name property")?;
    let remote = props.get("Remote").context("Missing Remote property")?;
    let active = props.get("Active").context("Missing Active property")?;
    let state = props.get("State").context("Missing State property")?;
    let session_type = props.get("Type").context("Missing Type property")?;
    let leader_pid = props
        .get("Leader")
        .context("Missing Leader property")?
        .parse::<u32>()
        .context("Failed to parse session leader PID")?;

    let is_active = active == "yes" || state == "active";
    let is_graphical = session_type == "x11" || session_type == "wayland";
    if !is_active || !is_graphical || remote != "no" {
        return Ok(None);
    }

    Ok(Some(SessionMetadata {
        name: name.to_string(),
        leader_pid,
        session_type: session_type.to_string(),
    }))
}

fn is_valid_bind_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
}

fn get_hostname() -> anyhow::Result<String> {
    let output = get_command_output(
        {
            let command = safe_command("hostname");
            command
        },
        "hostname",
    )?;
    let hostname = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if hostname.is_empty() {
        bail!("Hostname command returned an empty value");
    }

    Ok(hostname)
}

/// Check all prerequisites for prompt mode before spawning the background child.
/// This runs in the parent process so failures are visible to pssh/SSH.
pub fn ensure_prompt_prerequisites(player_user: &str) -> anyhow::Result<()> {
    // Verify runuser is available
    get_command_output(
        {
            let mut command = safe_command("runuser");
            command
                .arg("--version")
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            command
        },
        "runuser --version",
    )
    .map_err(|_| anyhow::Error::msg("runuser is not available"))?;

    ensure_yad_available()?;
    find_graphical_session(player_user).map(|_| ())
}

pub fn ensure_yad_available() -> anyhow::Result<()> {
    get_command_output(
        {
            let mut command = safe_command("which");
            command
                .arg("yad")
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            command
        },
        "which yad",
    )
    .map(|_| ())
    .map_err(|_| anyhow::Error::msg("yad is not installed or not available in PATH"))
}

pub fn find_graphical_session(player_user: &str) -> anyhow::Result<DesktopSessionEnv> {
    let output = get_command_output(
        {
            let mut command = safe_command("loginctl");
            command.arg("list-sessions").arg("--no-legend");
            command
        },
        "loginctl list-sessions",
    )?;

    let session_ids: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.split_whitespace().next().map(str::to_string))
        .collect();
    let uid = lookup_user_id(player_user)?;
    let fallback_home = lookup_home_dir(player_user)?;

    for session_id in session_ids {
        let metadata = match parse_session_metadata(&session_id) {
            Ok(Some(metadata)) => metadata,
            Ok(None) => continue,
            Err(err) => {
                tracing::warn!("Failed to inspect session {session_id}: {err:#}");
                continue;
            }
        };
        if metadata.name != player_user {
            continue;
        }

        let env = match read_process_environ(metadata.leader_pid) {
            Ok(env) => env,
            Err(err) => {
                tracing::warn!(
                    "Failed to read environment for session {session_id} leader {}: {err:#}",
                    metadata.leader_pid
                );
                continue;
            }
        };

        let home = match env.get("HOME") {
            Some(home) if !home.is_empty() => home.to_string(),
            _ => fallback_home.clone(),
        };

        let xdg_runtime_dir = env
            .get("XDG_RUNTIME_DIR")
            .cloned()
            .unwrap_or_else(|| format!("/run/user/{uid}"));
        let dbus_session_bus_address = env
            .get("DBUS_SESSION_BUS_ADDRESS")
            .cloned()
            .or_else(|| Some(format!("unix:path={xdg_runtime_dir}/bus")));
        let display = env.get("DISPLAY").cloned().or_else(|| {
            (metadata.session_type == "x11").then(|| ":0".to_string())
        });
        let wayland_display = env.get("WAYLAND_DISPLAY").cloned().or_else(|| {
            (metadata.session_type == "wayland").then(|| "wayland-0".to_string())
        });
        let xauthority = env
            .get("XAUTHORITY")
            .cloned()
            .or_else(|| Some(format!("{home}/.Xauthority")));

        return Ok(DesktopSessionEnv {
            display,
            wayland_display,
            xdg_runtime_dir,
            dbus_session_bus_address,
            xauthority,
            home,
        });
    }

    bail!("No active graphical session found for user {player_user}")
}

pub fn run_yad_as_user(
    player_user: &str,
    env: &DesktopSessionEnv,
    args: &[&str],
) -> anyhow::Result<Output> {
    let mut command = safe_command("runuser");
    command
        .arg("-u")
        .arg(player_user)
        .arg("--")
        .arg("yad")
        .args(args)
        .env("HOME", &env.home)
        .env("XDG_RUNTIME_DIR", &env.xdg_runtime_dir);

    if let Some(dbus_session_bus_address) = env.dbus_session_bus_address.as_deref() {
        command.env("DBUS_SESSION_BUS_ADDRESS", dbus_session_bus_address);
    }
    if let Some(display) = env.display.as_deref() {
        command.env("DISPLAY", display);
    }
    if let Some(wayland_display) = env.wayland_display.as_deref() {
        command.env("WAYLAND_DISPLAY", wayland_display);
    }
    if let Some(xauthority) = env.xauthority.as_deref() {
        command.env("XAUTHORITY", xauthority);
    }

    command
        .output()
        .with_context(|| format!("Failed to run yad as user {player_user}"))
}

pub fn prompt_bind_id(player_user: &str) -> anyhow::Result<PromptResult> {
    ensure_yad_available()?;
    let desktop_env = find_graphical_session(player_user)?;
    let hostname = get_hostname()?;
    let text = format!("Location: {hostname}\nEnter contestant ID");
    let args = vec![
        "--entry".to_string(),
        "--title=Natsume Bind".to_string(),
        format!("--text={text}"),
        "--ok-label=Bind".to_string(),
        "--cancel-label=Cancel".to_string(),
        "--modal".to_string(),
        "--width=420".to_string(),
    ];
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let output = run_yad_as_user(player_user, &desktop_env, &arg_refs)?;

    match output.status.code() {
        Some(0) => {
            let id = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !is_valid_bind_id(&id) {
                bail!("Bind ID must match ^[A-Za-z0-9._-]+$");
            }

            Ok(PromptResult { id, desktop_env })
        }
        Some(1) => bail!("Bind prompt cancelled by user"),
        Some(code) => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            bail!("Yad prompt failed with exit code {code}: {stderr}")
        }
        None => bail!("Yad prompt terminated by signal"),
    }
}

pub fn show_bind_result(
    player_user: &str,
    env: &DesktopSessionEnv,
    success: bool,
    text: &str,
) {
    let dialog_type = if success { "--info" } else { "--error" };
    let args = vec![
        dialog_type.to_string(),
        "--title=Natsume Bind".to_string(),
        format!("--text={text}"),
        "--timeout=5".to_string(),
        "--width=420".to_string(),
    ];
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();

    match run_yad_as_user(player_user, env, &arg_refs) {
        Ok(output) => match output.status.code() {
            Some(0) | Some(5) => {}
            Some(code) => {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                tracing::warn!("Failed to show bind result dialog, exit code {code}: {stderr}");
            }
            None => tracing::warn!("Failed to show bind result dialog: terminated by signal"),
        },
        Err(err) => tracing::warn!("Failed to show bind result dialog: {err:#}"),
    }
}
