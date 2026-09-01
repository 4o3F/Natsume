use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::{Resource, trace::SdkTracerProvider};
use snafu::Snafu;
use tracing::level_filters::LevelFilter;
use tracing_subscriber::{
    Layer as _, filter::Targets, fmt, layer::SubscriberExt as _, registry::Registry,
    util::SubscriberInitExt as _,
};

use crate::config::LogLevel;

const OTEL_EXPORTER_OTLP_ENDPOINT: &str = "OTEL_EXPORTER_OTLP_ENDPOINT";
const OTEL_EXPORTER_OTLP_TRACES_ENDPOINT: &str = "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT";
const OTEL_SDK_DISABLED: &str = "OTEL_SDK_DISABLED";

fn level_filter(level: LogLevel) -> LevelFilter {
    match level {
        LogLevel::Error => LevelFilter::ERROR,
        LogLevel::Warn => LevelFilter::WARN,
        LogLevel::Info => LevelFilter::INFO,
        LogLevel::Debug => LevelFilter::DEBUG,
        LogLevel::Trace => LevelFilter::TRACE,
    }
}

fn log_layer<W>(level: LogLevel, writer: W) -> impl tracing_subscriber::Layer<Registry>
where
    W: for<'writer> fmt::MakeWriter<'writer> + Send + Sync + 'static,
{
    let targets = Targets::new()
        .with_default(LevelFilter::OFF)
        .with_target("natsume_server", level_filter(level));
    fmt::layer()
        .with_writer(writer)
        .with_ansi(false)
        .without_time()
        .with_target(false)
        .with_filter(targets)
}

#[cfg(test)]
fn subscriber<W>(level: LogLevel, writer: W) -> impl tracing::Subscriber + Send + Sync + 'static
where
    W: for<'writer> fmt::MakeWriter<'writer> + Send + Sync + 'static,
{
    Registry::default().with(log_layer(level, writer))
}

pub(crate) fn initialize(level: LogLevel) -> Result<TelemetryGuard, LoggingError> {
    let tracer_provider = if telemetry_enabled_from_environment() {
        Some(build_tracer_provider()?)
    } else {
        None
    };
    let telemetry_layer = tracer_provider.as_ref().map(|provider| {
        let tracer = provider.tracer("natsume-server");
        let targets = Targets::new()
            .with_default(LevelFilter::OFF)
            .with_target("natsume_server", LevelFilter::TRACE);
        tracing_opentelemetry::layer()
            .with_tracer(tracer)
            .with_filter(targets)
    });
    Registry::default()
        .with(log_layer(level, std::io::stderr))
        .with(telemetry_layer)
        .try_init()
        .map_err(|_| LoggingError::SubscriberInitialization)?;
    Ok(TelemetryGuard { tracer_provider })
}

fn build_tracer_provider() -> Result<SdkTracerProvider, LoggingError> {
    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .build()
        .map_err(|_| LoggingError::ExporterInitialization)?;
    let resource = Resource::builder()
        .with_service_name("natsume-server")
        .build();
    Ok(SdkTracerProvider::builder()
        .with_resource(resource)
        .with_batch_exporter(exporter)
        .build())
}

fn telemetry_enabled_from_environment() -> bool {
    let disabled = std::env::var(OTEL_SDK_DISABLED).ok();
    let endpoint = std::env::var(OTEL_EXPORTER_OTLP_ENDPOINT).ok();
    let traces_endpoint = std::env::var(OTEL_EXPORTER_OTLP_TRACES_ENDPOINT).ok();
    telemetry_enabled(
        disabled.as_deref(),
        endpoint.as_deref(),
        traces_endpoint.as_deref(),
    )
}

fn telemetry_enabled(
    disabled: Option<&str>,
    endpoint: Option<&str>,
    traces_endpoint: Option<&str>,
) -> bool {
    !disabled.is_some_and(|value| value.eq_ignore_ascii_case("true"))
        && [endpoint, traces_endpoint]
            .into_iter()
            .flatten()
            .any(|value| !value.trim().is_empty())
}

pub(crate) struct TelemetryGuard {
    tracer_provider: Option<SdkTracerProvider>,
}

impl TelemetryGuard {
    pub(crate) fn shutdown(self) {
        if let Some(provider) = self.tracer_provider
            && provider.shutdown().is_err()
        {
            eprintln!("OpenTelemetry trace shutdown failed");
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Snafu)]
pub(crate) enum LoggingError {
    #[snafu(display("OpenTelemetry OTLP trace exporter initialization failed"))]
    ExporterInitialization,
    #[snafu(display("structured logging initialization failed"))]
    SubscriberInitialization,
}

#[cfg(test)]
pub(crate) mod tests {
    use std::{
        io::{self, Write},
        sync::{
            Arc, Mutex, MutexGuard, PoisonError,
            atomic::{AtomicBool, Ordering},
        },
        time::Duration,
    };

    use opentelemetry_sdk::{
        error::{OTelSdkError, OTelSdkResult},
        trace::{SdkTracerProvider, SpanData, SpanExporter},
    };
    use snafu::Snafu;
    use tracing_subscriber::fmt::MakeWriter;

    use crate::config::LogLevel;

    static SUBSCRIBER_TEST_LOCK: Mutex<()> = Mutex::new(());

    pub(crate) struct SubscriberTestGuard {
        _guard: MutexGuard<'static, ()>,
    }

    impl SubscriberTestGuard {
        /// Serialises tests that install a scoped subscriber because `tracing`
        /// caches callsite interest process-globally.
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

    #[test]
    fn otlp_export_requires_a_nonempty_endpoint_and_honours_sdk_disabled() {
        assert!(!super::telemetry_enabled(None, None, None));
        assert!(!super::telemetry_enabled(None, Some("  "), None));
        assert!(super::telemetry_enabled(
            None,
            Some("http://collector:4317"),
            None
        ));
        assert!(super::telemetry_enabled(
            None,
            None,
            Some("http://collector:4317")
        ));
        assert!(!super::telemetry_enabled(
            Some("TRUE"),
            Some("http://collector:4317"),
            None
        ));
    }

    #[test]
    fn provider_shutdown_failure_is_best_effort() {
        let shutdown_called = Arc::new(AtomicBool::new(false));
        let provider = SdkTracerProvider::builder()
            .with_simple_exporter(ShutdownFailingExporter {
                shutdown_called: Arc::clone(&shutdown_called),
            })
            .build();

        super::TelemetryGuard {
            tracer_provider: Some(provider),
        }
        .shutdown();

        assert!(shutdown_called.load(Ordering::Relaxed));
    }

    #[derive(Debug)]
    struct ShutdownFailingExporter {
        shutdown_called: Arc<AtomicBool>,
    }

    impl SpanExporter for ShutdownFailingExporter {
        fn export(
            &self,
            _batch: Vec<SpanData>,
        ) -> impl std::future::Future<Output = OTelSdkResult> + Send {
            std::future::ready(Ok(()))
        }

        fn shutdown_with_timeout(&self, _timeout: Duration) -> OTelSdkResult {
            self.shutdown_called.store(true, Ordering::Relaxed);
            Err(OTelSdkError::InternalFailure(
                "exporter detail must not be logged".to_owned(),
            ))
        }
    }

    #[derive(Debug, Snafu)]
    enum TestFailure {
        #[snafu(display("captured logging output could not be read"))]
        CaptureFailed,
        #[snafu(display("the configured logging level filter changed"))]
        LevelFilterChanged,
    }
}
