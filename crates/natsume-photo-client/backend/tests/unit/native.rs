use super::*;
use snafu::{ResultExt as _, Snafu};

#[derive(Debug, Snafu)]
enum Error {
    Io { source: std::io::Error },
    Native { source: windows::core::Error },
    Timeout { source: tokio::time::error::Elapsed },
    Closed,
}

#[tokio::test]
async fn safe_winrt_subscription_observes_repeated_changes() -> Result<(), Error> {
    let directory = tempfile::tempdir().context(IoSnafu)?;
    let (send, mut receive) = mpsc::channel(1);
    let monitor = NativeMonitor::open(directory.path(), send)
        .await
        .context(NativeSnafu)?;
    for index in 0..2 {
        std::fs::write(directory.path().join(format!("photo-{index}.jpg")), b"test")
            .context(IoSnafu)?;
        // Notifications can coalesce or miss the first immediate change. Keep
        // producing bounded changes, but still require a real wake each round.
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let mut changes = tokio::time::interval(std::time::Duration::from_millis(250));
            let mut revision = 0;
            loop {
                tokio::select! {
                    event = receive.recv() => return event.ok_or(Error::Closed),
                    _ = changes.tick() => {
                        revision += 1;
                        std::fs::write(directory.path().join(format!("photo-{index}-{revision}.jpg")), b"test")
                            .context(IoSnafu)?;
                    }
                }
            }
        })
            .await
            .context(TimeoutSnafu)??;
        monitor.refresh().await.context(NativeSnafu)?;
    }
    Ok(())
}
