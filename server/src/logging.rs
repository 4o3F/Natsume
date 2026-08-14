use snafu::Snafu;
use tracing::level_filters::LevelFilter;
use tracing_subscriber::{
    Layer as _, filter::Targets, fmt, layer::SubscriberExt as _, registry::Registry,
    util::SubscriberInitExt as _,
};

use crate::config::LogLevel;

fn level_filter(level: LogLevel) -> LevelFilter {
    match level {
        LogLevel::Error => LevelFilter::ERROR,
        LogLevel::Warn => LevelFilter::WARN,
        LogLevel::Info => LevelFilter::INFO,
        LogLevel::Debug => LevelFilter::DEBUG,
        LogLevel::Trace => LevelFilter::TRACE,
    }
}

fn subscriber<W>(level: LogLevel, writer: W) -> impl tracing::Subscriber + Send + Sync + 'static
where
    W: for<'writer> fmt::MakeWriter<'writer> + Send + Sync + 'static,
{
    let targets = Targets::new()
        .with_default(LevelFilter::OFF)
        .with_target("natsume_server", level_filter(level));
    let formatter = fmt::layer()
        .with_writer(writer)
        .with_ansi(false)
        .without_time()
        .with_target(false)
        .with_filter(targets);
    Registry::default().with(formatter)
}

pub(crate) fn initialize(level: LogLevel) -> Result<(), LoggingError> {
    subscriber(level, std::io::stderr)
        .try_init()
        .map_err(|_| LoggingError::InitializationFailed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Snafu)]
pub(crate) enum LoggingError {
    #[snafu(display("structured logging initialization failed"))]
    InitializationFailed,
}

#[cfg(test)]
pub(crate) mod tests {
    use std::{
        io::{self, Write},
        sync::{Arc, Mutex, MutexGuard, PoisonError},
    };

    use snafu::Snafu;
    use tracing_subscriber::fmt::MakeWriter;

    use crate::config::LogLevel;

    static SUBSCRIBER_TEST_LOCK: Mutex<()> = Mutex::new(());

    pub(crate) struct SubscriberTestGuard {
        _guard: MutexGuard<'static, ()>,
    }

    impl SubscriberTestGuard {
        /// Serialises every test that installs a scoped subscriber or emits a
        /// crate callsite without one. `tracing` caches callsite interest
        /// process-globally, and a `Dispatch::new` rebuild racing a callsite's
        /// first registration can latch that callsite at `never` for the rest of
        /// the process, silently dropping the event from later captures.
        ///
        /// This is a plain `std` mutex because the tests that need it are a mix
        /// of `#[test]` and `#[tokio::test]`; the guard is held across `.await`
        /// on the current-thread runtimes `#[tokio::test]` builds.
        pub(crate) fn acquire() -> Self {
            Self {
                _guard: SUBSCRIBER_TEST_LOCK
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner),
            }
        }
    }

    #[derive(Clone, Default)]
    pub(crate) struct CapturedLogs {
        buffer: Arc<Mutex<Vec<u8>>>,
    }

    impl CapturedLogs {
        pub(crate) fn subscriber(
            &self,
            level: LogLevel,
        ) -> impl tracing::Subscriber + Send + Sync + 'static {
            super::subscriber(level, self.clone())
        }

        pub(crate) fn text(&self) -> Result<String, ()> {
            let buffer = self.buffer.lock().map_err(|_| ())?;
            String::from_utf8(buffer.clone()).map_err(|_| ())
        }
    }

    pub(crate) struct CapturedWriter {
        buffer: Arc<Mutex<Vec<u8>>>,
    }

    impl Write for CapturedWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            let mut buffer = self
                .buffer
                .lock()
                .map_err(|_| io::Error::other("captured log buffer unavailable"))?;
            buffer.extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'writer> MakeWriter<'writer> for CapturedLogs {
        type Writer = CapturedWriter;

        fn make_writer(&'writer self) -> Self::Writer {
            CapturedWriter {
                buffer: Arc::clone(&self.buffer),
            }
        }
    }

    #[test]
    fn configured_level_suppresses_lower_events_and_emits_enabled_events() -> Result<(), TestFailure>
    {
        let _subscriber_guard = SubscriberTestGuard::acquire();
        let captured = CapturedLogs::default();
        let subscriber = captured.subscriber(LogLevel::Info);
        tracing::subscriber::with_default(subscriber, || {
            tracing::debug!("below-level-debug-canary");
            tracing::info!("enabled-info-canary");
            tracing::error!("enabled-error-canary");
        });
        let output = captured.text().map_err(|()| TestFailure::CaptureFailed)?;
        if output.contains("below-level-debug-canary")
            || !output.contains("enabled-info-canary")
            || !output.contains("enabled-error-canary")
            || output.contains('\u{1b}')
        {
            return Err(TestFailure::LevelFilterChanged);
        }
        Ok(())
    }

    #[derive(Debug, Snafu)]
    enum TestFailure {
        #[snafu(display("captured logging output could not be read"))]
        CaptureFailed,
        #[snafu(display("the configured logging level filter changed"))]
        LevelFilterChanged,
    }
}
