#![cfg(target_os = "linux")]

use std::{fs, os::unix::fs::PermissionsExt, process::Command};

#[test]
fn executable_loads_explicit_configuration() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let config = directory.path().join("print config.toml");
    fs::write(
        &config,
        r#"
server_url = "https://print.example.org"
token_file = "/nonexistent/source.token"
"#,
    )?;
    fs::set_permissions(&config, fs::Permissions::from_mode(0o600))?;

    let output = Command::new(env!("CARGO_BIN_EXE_natsume-print-domjudge"))
        .arg("--config")
        .arg(&config)
        .args([
            "--file=/nonexistent/source.cpp",
            "--account=team01",
            "--original=source.cpp",
            "--request-id=invalid",
        ])
        .output()?;

    // Input validation follows configuration loading and precedes token reads or HTTPS.
    assert!(!output.status.success());
    let stdout = String::from_utf8(output.stdout)?;
    assert!(stdout.contains("ERROR"), "{stdout}");
    assert!(stdout.contains("打印参数或 UTF-8 源文件无效"), "{stdout}");
    assert!(output.stderr.is_empty());
    assert!(!stdout.contains('\u{1b}'));
    Ok(())
}

#[test]
fn configuration_failures_use_tracing_stdout() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let output = Command::new(env!("CARGO_BIN_EXE_natsume-print-domjudge"))
        .arg("--config")
        .arg(directory.path().join("missing.toml"))
        .args([
            "--file=/nonexistent/source.cpp",
            "--account=team01",
            "--original=source.cpp",
        ])
        .output()?;

    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8(output.stdout)?;
    assert!(stdout.contains("ERROR"), "{stdout}");
    assert!(
        stdout.contains("源文件或部署文件不安全、过大或不可读"),
        "{stdout}"
    );
    assert!(output.stderr.is_empty());
    Ok(())
}

#[test]
fn cli_help_and_errors_use_tracing_stdout() -> Result<(), Box<dyn std::error::Error>> {
    for (argument, code, level) in [("--help", 0, "INFO"), ("--unknown-option", 2, "ERROR")] {
        let output = Command::new(env!("CARGO_BIN_EXE_natsume-print-domjudge"))
            .arg(argument)
            .output()?;
        assert_eq!(output.status.code(), Some(code));
        let stdout = String::from_utf8(output.stdout)?;
        assert!(stdout.contains(level), "{stdout}");
        assert!(stdout.contains("Usage:"), "{stdout}");
        assert!(output.stderr.is_empty());
        assert!(!stdout.contains('\u{1b}'));
    }
    Ok(())
}
