use super::*;
use snafu::Snafu;

#[derive(Debug, Snafu)]
enum TestError {
    Io {
        source: std::io::Error,
    },
    Initialization {
        source: super::super::Error,
    },
    Signal {
        source: tokio::sync::oneshot::error::RecvError,
    },
    Timeout {
        source: tokio::time::error::Elapsed,
    },
}
type TestResult = std::result::Result<(), TestError>;

#[tokio::test]
async fn cancelling_the_caller_keeps_the_lock_until_blocking_io_finishes() -> TestResult {
    let directory = tempfile::tempdir().context(IoSnafu)?;
    let config = Config {
        server_url: "https://localhost".into(),
        workspace: directory.path().join("workspace"),
        ca_certificate: None,
    };
    let store = Store::open(&config).context(InitializationSnafu)?;
    let (started, entered) = tokio::sync::oneshot::channel();
    let (release, waiting) = std::sync::mpsc::channel();
    let task = tokio::spawn(async move {
        store
            .blocking(move || {
                let _ = started.send(());
                waiting
                    .recv()
                    .map_err(|_| super::super::InvalidSnafu.build())?;
                Ok(())
            })
            .await
    });
    entered.await.context(SignalSnafu)?;
    task.abort();
    assert!(task.await.is_err_and(|error| error.is_cancelled()));
    assert!(matches!(
        Store::open(&config),
        Err(super::super::Error::Busy)
    ));
    drop(release);
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            match Store::open(&config) {
                Ok(store) => {
                    drop(store);
                    return Ok(());
                }
                Err(super::super::Error::Busy) => tokio::task::yield_now().await,
                Err(error) => return Err(error),
            }
        }
    })
    .await
    .context(TimeoutSnafu)?
    .context(InitializationSnafu)
}
