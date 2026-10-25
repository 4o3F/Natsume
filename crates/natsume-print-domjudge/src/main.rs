#[cfg(target_os = "linux")]
fn main() -> std::process::ExitCode {
    application::run_application()
}

#[cfg(not(target_os = "linux"))]
fn main() -> std::process::ExitCode {
    tracing_subscriber::fmt()
        .with_writer(std::io::stdout)
        .with_ansi(false)
        .with_target(false)
        .init();
    tracing::error!("the DOMjudge print wrapper requires Linux");
    std::process::ExitCode::FAILURE
}

#[cfg(target_os = "linux")]
mod application {
    use std::{path::PathBuf, process::ExitCode};

    use clap::Parser;
    use natsume_print_domjudge::{Input, Runtime, config::Config, run, success_message};
    use tokio::time::Instant;
    use tokio_util::sync::CancellationToken;

    #[derive(Parser)]
    #[command(about = "Register, typeset and enqueue a DOMjudge UTF-8 source file")]
    struct Arguments {
        /// Deployment configuration file.
        #[arg(long, default_value = "/etc/natsume-print-domjudge/config.toml")]
        config: PathBuf,
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        account: String,
        #[arg(long)]
        original: String,
        #[arg(long)]
        request_id: Option<String>,
    }

    pub(super) fn run_application() -> ExitCode {
        let started = Instant::now();
        // DOMjudge reads stdout for both successful and failed invocation results.
        if tracing_subscriber::fmt()
            .with_writer(std::io::stdout)
            .with_ansi(false)
            .without_time()
            .with_target(false)
            .with_max_level(tracing::Level::INFO)
            .try_init()
            .is_err()
        {
            return ExitCode::FAILURE;
        }
        let arguments = match Arguments::try_parse() {
            Ok(arguments) => arguments,
            Err(error) => {
                if error.use_stderr() {
                    tracing::error!("{error}");
                    return ExitCode::from(2);
                }
                tracing::info!("{error}");
                return ExitCode::SUCCESS;
            }
        };
        let input = Input {
            file: arguments.file,
            account: arguments.account,
            original: arguments.original,
            request_id: arguments
                .request_id
                .unwrap_or_else(|| uuid::Uuid::now_v7().to_string()),
        };
        let config = match Config::load(&arguments.config) {
            Ok(config) => config,
            Err(error) => {
                tracing::error!("{error}");
                return ExitCode::FAILURE;
            }
        };
        let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            tracing::error!("打印运行环境不可用");
            return ExitCode::FAILURE;
        };
        runtime.block_on(async {
            let cancellation = CancellationToken::new();
            let signal_token = cancellation.clone();
            let signals = tokio::spawn(async move {
                let interrupt =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt());
                let terminate =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate());
                if let (Ok(mut interrupt), Ok(mut terminate)) = (interrupt, terminate) {
                    tokio::select! { _ = interrupt.recv() => {}, _ = terminate.recv() => {} }
                    signal_token.cancel();
                }
            });
            let result = run(
                &config,
                &Runtime::installed(),
                &input,
                started,
                &cancellation,
            )
            .await;
            signals.abort();
            match result {
                Ok(fact) => {
                    tracing::info!("{}", success_message(&fact));
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    // Invalid CLI IDs are data too; do not emit raw terminal controls.
                    tracing::error!(
                        "{error}；请求 ID：{}",
                        serde_json::to_string(&input.request_id).unwrap_or_default()
                    );
                    ExitCode::FAILURE
                }
            }
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        const REQUIRED: [&str; 4] = [
            "natsume-print-domjudge",
            "--file=/tmp/source.cpp",
            "--account=team01",
            "--original=source.cpp",
        ];

        #[test]
        fn configuration_defaults_to_installed_path() -> Result<(), clap::Error> {
            let arguments = Arguments::try_parse_from(REQUIRED)?;
            assert_eq!(
                arguments.config,
                PathBuf::from("/etc/natsume-print-domjudge/config.toml")
            );
            Ok(())
        }

        #[test]
        fn explicit_configuration_overrides_default() -> Result<(), clap::Error> {
            for option in [
                vec!["--config", "/srv/print config/site.toml"],
                vec!["--config=/srv/print config/site.toml"],
            ] {
                let arguments = Arguments::try_parse_from(REQUIRED.into_iter().chain(option))?;
                assert_eq!(
                    arguments.config,
                    PathBuf::from("/srv/print config/site.toml")
                );
            }
            Ok(())
        }

        #[test]
        fn configuration_option_requires_a_path() {
            assert!(Arguments::try_parse_from(REQUIRED.into_iter().chain(["--config"])).is_err());
        }
    }
}
