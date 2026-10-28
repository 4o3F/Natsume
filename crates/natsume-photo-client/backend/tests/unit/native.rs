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
        tokio::time::timeout(std::time::Duration::from_secs(5), receive.recv())
            .await
            .context(TimeoutSnafu)?
            .ok_or(Error::Closed)?;
        monitor.refresh().await.context(NativeSnafu)?;
    }
    Ok(())
}
