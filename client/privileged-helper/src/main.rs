use std::{
    future,
    io::{self, Write as _},
    process::ExitCode,
};

use natsume_local_control_api::{PRIVILEGED1_PATH, PRIVILEGED1_SERVICE};
use natsume_privileged_helper::PrivilegedService;
use snafu::Snafu;
use tokio::time::{Duration, timeout};

const LOGGING_FAILURE_ID: &str = "NATSUME_PRIVILEGED_HELPER_LOGGING_INIT_FAILED";
const SYSTEM_BUS_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Snafu)]
enum ServiceError {
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

async fn serve() -> Result<(), ServiceError> {
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
