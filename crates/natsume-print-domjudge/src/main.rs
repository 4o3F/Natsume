use std::{
    path::{Path, PathBuf},
    process::ExitCode,
};

use clap::Parser;
use natsume_print_domjudge::{
    Input, Runtime,
    config::{CONFIG_PATH, Config},
    run, success_message,
};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

#[derive(Parser)]
#[command(about = "Register, typeset and enqueue a DOMjudge UTF-8 source file")]
struct Arguments {
    #[arg(long)]
    file: PathBuf,
    #[arg(long)]
    account: String,
    #[arg(long)]
    original: String,
    #[arg(long)]
    request_id: Option<String>,
}

fn main() -> ExitCode {
    let started = Instant::now();
    let arguments = Arguments::parse();
    let input = Input {
        file: arguments.file,
        account: arguments.account,
        original: arguments.original,
        request_id: arguments
            .request_id
            .unwrap_or_else(|| uuid::Uuid::now_v7().to_string()),
    };
    let config = match Config::load(Path::new(CONFIG_PATH)) {
        Ok(config) => config,
        Err(error) => {
            println!("{error}");
            return ExitCode::FAILURE;
        }
    };
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        println!("打印运行环境不可用");
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
                println!("{}", success_message(&fact));
                ExitCode::SUCCESS
            }
            Err(error) => {
                // Invalid CLI IDs are data too; do not emit raw terminal controls.
                println!(
                    "{error}；请求 ID：{}",
                    serde_json::to_string(&input.request_id).unwrap_or_default()
                );
                ExitCode::FAILURE
            }
        }
    })
}
