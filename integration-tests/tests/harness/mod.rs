use std::{ffi::OsStr, path::Path, process::Stdio};

use tokio::{io::AsyncWriteExt as _, process::Command};
use zeroize::{Zeroize as _, Zeroizing};

const BOOTSTRAP_CONFIG_ENVIRONMENT: &str = "NATSUME_TEST_SERVER_CONFIG";
const BOOTSTRAP_DRIVER: &str = env!("CARGO_BIN_EXE_server-bootstrap-driver");

pub(crate) async fn bootstrap_operator(config_path: &Path, login_name: &str, password: &str) {
    let command = shell_quote(OsStr::new(BOOTSTRAP_DRIVER));
    let mut child = require_ok(
        Command::new("script")
            .args(["-qefc", &command, "/dev/null"])
            .env(BOOTSTRAP_CONFIG_ENVIRONMENT, config_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn(),
        "bootstrap composition process must start",
    );
    let mut input = Zeroizing::new(format!("{login_name}\n{password}\n{password}\n"));
    let mut stdin = child
        .stdin
        .take()
        .unwrap_or_else(|| panic!("bootstrap composition stdin must exist"));
    require_ok(
        stdin.write_all(input.as_bytes()).await,
        "bootstrap credentials must be supplied through the PTY",
    );
    input.zeroize();
    drop(stdin);
    let status = require_ok(
        child.wait().await,
        "bootstrap composition process must finish",
    );
    assert!(status.success(), "bootstrap composition must succeed");
}

fn shell_quote(value: &OsStr) -> String {
    format!("'{}'", value.to_string_lossy().replace('\'', "'\"'\"'"))
}

fn require_ok<T, E>(result: Result<T, E>, message: &str) -> T {
    match result {
        Ok(value) => value,
        Err(error) => {
            drop(error);
            panic!("{message}");
        }
    }
}
