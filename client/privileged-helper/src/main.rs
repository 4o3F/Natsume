use std::{
    io::{self, Write as _},
    process::ExitCode,
};

const LOGGING_FAILURE_ID: &str = "NATSUME_PRIVILEGED_HELPER_LOGGING_INIT_FAILED";

fn initialize_logging() -> Result<(), ()> {
    tracing_subscriber::fmt()
        .with_ansi(false)
        .without_time()
        .with_target(false)
        .try_init()
        .map_err(|_| ())
}

fn main() -> ExitCode {
    if initialize_logging().is_err() {
        let _write_result = writeln!(io::stderr().lock(), "{LOGGING_FAILURE_ID}");
        return ExitCode::FAILURE;
    }
    tracing::info!("natsume-privileged-helper architecture blueprint");
    ExitCode::SUCCESS
}
