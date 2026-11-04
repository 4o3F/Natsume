use std::{
    fs::{self, File},
    io::{BufWriter, Write as _},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use flate2::{Compression, write::GzEncoder};
use snafu::ResultExt as _;
use tokio::{sync::mpsc, task::JoinHandle};

use crate::error::{self, Result};

#[derive(Debug)]
pub(crate) struct Event {
    ts_us: u128,
    device: usize,
    kind: &'static str,
    latency_us: i128,
    code: String,
}

#[derive(Clone)]
pub(crate) struct Metrics {
    sender: mpsc::Sender<Event>,
    pub(crate) connected: Arc<AtomicUsize>,
    pub(crate) events: Arc<AtomicU64>,
    pub(crate) dropped: Arc<AtomicU64>,
    pub(crate) convergence: Arc<crate::convergence::Tracker>,
}

pub(crate) struct Connected(Arc<AtomicUsize>);
impl Drop for Connected {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

impl Metrics {
    pub(crate) fn start(directory: &Path) -> Result<(Self, JoinHandle<Result<()>>)> {
        fs::create_dir(directory).context(error::IoSnafu)?;
        let file = File::create(directory.join("events.csv.gz")).context(error::IoSnafu)?;
        let (sender, mut receiver) = mpsc::channel::<Event>(65_536);
        let task = tokio::task::spawn_blocking(move || {
            let mut writer = GzEncoder::new(BufWriter::new(file), Compression::fast());
            writer
                .write_all(b"ts_us,device,event_kind,latency_us,detail_code\n")
                .context(error::IoSnafu)?;
            while let Some(event) = receiver.blocking_recv() {
                writeln!(
                    writer,
                    "{},{},{},{},{}",
                    event.ts_us, event.device, event.kind, event.latency_us, event.code
                )
                .context(error::IoSnafu)?;
            }
            writer
                .finish()
                .context(error::IoSnafu)?
                .flush()
                .context(error::IoSnafu)
        });
        Ok((
            Self {
                sender,
                connected: Arc::default(),
                events: Arc::default(),
                dropped: Arc::default(),
                convergence: Arc::default(),
            },
            task,
        ))
    }

    pub(crate) fn record(
        &self,
        device: usize,
        kind: &'static str,
        latency: Duration,
        code: impl Into<String>,
    ) -> Result<()> {
        self.record_us(
            device,
            kind,
            i128::try_from(latency.as_micros()).unwrap_or(i128::MAX),
            code,
        )
    }

    pub(crate) fn record_us(
        &self,
        device: usize,
        kind: &'static str,
        latency_us: i128,
        code: impl Into<String>,
    ) -> Result<()> {
        let result = self.sender.try_send(Event {
            ts_us: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_micros(),
            device,
            kind,
            latency_us,
            code: code.into(),
        });
        if matches!(result, Err(mpsc::error::TrySendError::Full(_))) {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return Ok(());
        }
        result.context(error::MetricsSnafu)?;
        self.events.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    pub(crate) fn connected(&self) -> Connected {
        self.connected.fetch_add(1, Ordering::Relaxed);
        Connected(Arc::clone(&self.connected))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn saturated_writer_drops_events_without_blocking_and_closed_writer_keeps_source() -> Result<()>
    {
        let (sender, receiver) = mpsc::channel(1);
        let metrics = Metrics {
            sender,
            connected: Arc::default(),
            events: Arc::default(),
            dropped: Arc::default(),
            convergence: Arc::default(),
        };
        metrics.record(0, "logo", Duration::ZERO, "200")?;
        metrics.record(1, "logo", Duration::ZERO, "304")?;
        assert_eq!(metrics.events.load(Ordering::Relaxed), 1);
        assert_eq!(metrics.dropped.load(Ordering::Relaxed), 1);
        drop(receiver);
        let error = metrics
            .record(0, "logo", Duration::ZERO, "304")
            .err()
            .ok_or(crate::error::Error::Invalid {
                code: "TEST_CLOSED_WRITER",
            })?;
        assert!(std::error::Error::source(&error).is_some());
        Ok(())
    }
}
