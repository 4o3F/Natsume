use std::{
    env, fs, future,
    io::{self, Write as _},
    os::unix::fs::MetadataExt as _,
    path::Path,
    process::{self, ExitCode},
};

use natsume_local_control_api::{PRIVILEGED1_PATH, PRIVILEGED1_SERVICE};
use natsume_privileged_helper::PrivilegedService;
use snafu::Snafu;
use tokio::time::{Duration, timeout};

const LOGGING_FAILURE_ID: &str = "NATSUME_PRIVILEGED_HELPER_LOGGING_INIT_FAILED";
const SYSTEM_BUS_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Snafu)]
enum ServiceError {
    #[snafu(display("privileged helper requires its systemd host mount namespace descriptor"))]
    MountNamespaceDescriptor,
    #[snafu(display("privileged helper could not inspect its host mount namespace"))]
    MountNamespaceInspection,
    #[snafu(display("privileged helper must share the host mount namespace"))]
    MountNamespaceMismatch,
    #[snafu(display("privileged helper could not acquire its system D-Bus service"))]
    Bus,
}

fn initialize_logging() -> Result<(), ()> {
    tracing_subscriber::fmt()
        .with_ansi(false)
        .without_time()
        .with_target(false)
        .try_init()
        .map_err(|_| ())
}

fn verify_mount_namespace(helper: &Path, host: &Path) -> Result<(), ServiceError> {
    // Follow the procfs links: their targets identify the namespaces, not the links.
    let helper = fs::metadata(helper).map_err(|_| ServiceError::MountNamespaceInspection)?;
    let host = fs::metadata(host).map_err(|_| ServiceError::MountNamespaceInspection)?;
    if (helper.dev(), helper.ino()) != (host.dev(), host.ino()) {
        return Err(ServiceError::MountNamespaceMismatch);
    }
    Ok(())
}

async fn serve() -> Result<(), ServiceError> {
    // Home mutations and verification must run in the native systemd host's domain.
    // Check before exposing any privileged operation or acquiring the service name.
    if env::var("LISTEN_PID")
        .ok()
        .and_then(|pid| pid.parse::<u32>().ok())
        != Some(process::id())
        || env::var("LISTEN_FDS").as_deref() != Ok("1")
        || env::var("LISTEN_FDNAMES").as_deref() != Ok("host-mount-namespace")
    {
        return Err(ServiceError::MountNamespaceDescriptor);
    }
    // OpenFile supplies this sole descriptor at fd 3 before capabilities are reduced.
    // Inspecting PID 1's procfs link directly would require CAP_SYS_PTRACE.
    verify_mount_namespace(Path::new("/proc/self/ns/mnt"), Path::new("/proc/self/fd/3"))?;
    let builder = zbus::connection::Builder::system().map_err(|_| ServiceError::Bus)?;
    let builder = builder
        .name(PRIVILEGED1_SERVICE)
        .map_err(|_| ServiceError::Bus)?;
    let builder = builder
        .serve_at(PRIVILEGED1_PATH, PrivilegedService::production())
        .map_err(|_| ServiceError::Bus)?;
    let _connection = timeout(
        SYSTEM_BUS_TIMEOUT,
        builder.method_timeout(SYSTEM_BUS_TIMEOUT).build(),
    )
    .await
    .map_err(|_| ServiceError::Bus)?
    .map_err(|_| ServiceError::Bus)?;
    tracing::info!(service = PRIVILEGED1_SERVICE, "privileged helper ready");
    future::pending::<()>().await;
    Ok(())
}

#[tokio::main]
async fn main() -> ExitCode {
    if initialize_logging().is_err() {
        let _write_result = writeln!(io::stderr().lock(), "{LOGGING_FAILURE_ID}");
        return ExitCode::FAILURE;
    }
    match serve().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(error = %error, "privileged helper stopped");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use super::*;

    #[test]
    fn accepts_links_to_same_mount_namespace() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let namespace = directory.path().join("namespace");
        let helper = directory.path().join("helper");
        let host = directory.path().join("host");
        fs::write(&namespace, b"")?;
        symlink(&namespace, &helper)?;
        symlink(&namespace, &host)?;

        verify_mount_namespace(&helper, &host)?;
        Ok(())
    }

    #[test]
    fn rejects_different_mount_namespaces() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let helper = directory.path().join("helper");
        let host = directory.path().join("host");
        fs::write(&helper, b"")?;
        fs::write(&host, b"")?;

        assert!(matches!(
            verify_mount_namespace(&helper, &host),
            Err(ServiceError::MountNamespaceMismatch)
        ));
        Ok(())
    }

    #[test]
    fn rejects_missing_mount_namespace() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let present = directory.path().join("present");
        let missing = directory.path().join("missing");
        fs::write(&present, b"")?;

        for (helper, host) in [(&missing, &present), (&present, &missing)] {
            assert!(matches!(
                verify_mount_namespace(helper, host),
                Err(ServiceError::MountNamespaceInspection)
            ));
        }
        Ok(())
    }
}
