use std::sync::{Arc, OnceLock};

use opentelemetry::{
    Context,
    global::{BoxedSpan, BoxedTracer},
    trace::{SpanBuilder, Tracer, TracerProvider as _, noop::NoopTracer},
};
use opentelemetry_sdk::{Resource, trace::SdkTracerProvider};
use snafu::Snafu;
use tracing::level_filters::LevelFilter;
use tracing_subscriber::{
    Layer as _, filter::Targets, fmt, layer::SubscriberExt as _, registry::Registry, reload,
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

fn targets(level: LogLevel) -> Targets {
    Targets::new()
        .with_default(LevelFilter::OFF)
        .with_target("natsume_server", level_filter(level))
}

fn log_layer<W, F>(filter: F, writer: W) -> impl tracing_subscriber::Layer<Registry>
where
    W: for<'writer> fmt::MakeWriter<'writer> + Send + Sync + 'static,
    F: tracing_subscriber::layer::Filter<Registry> + Send + Sync + 'static,
{
    fmt::layer()
        .with_writer(writer)
        .with_ansi(false)
        .without_time()
        .with_target(false)
        .with_filter(filter)
}

/// Handles for configuring the process subscriber installed before startup.
pub struct Logging {
    level: reload::Handle<Targets, Registry>,
    telemetry: reload::Handle<Targets, Registry>,
    tracer: Arc<OnceLock<BoxedTracer>>,
}

struct StartupTracer {
    configured: Arc<OnceLock<BoxedTracer>>,
    noop: BoxedTracer,
}

impl Tracer for StartupTracer {
    type Span = BoxedSpan;

    fn build_with_context(&self, builder: SpanBuilder, parent: &Context) -> Self::Span {
        self.configured
            .get()
            .unwrap_or(&self.noop)
            .build_with_context(builder, parent)
    }
}

fn subscriber<W>(writer: W) -> (impl tracing::Subscriber + Send + Sync, Logging)
where
    W: for<'writer> fmt::MakeWriter<'writer> + Send + Sync + 'static,
{
    let (level, level_handle) = reload::Layer::new(targets(LogLevel::Info));
    let (telemetry_filter, telemetry_handle) =
        reload::Layer::new(Targets::new().with_default(LevelFilter::OFF));
    let tracer = Arc::new(OnceLock::new());
    let telemetry = tracing_opentelemetry::layer()
        .with_tracer(StartupTracer {
            configured: Arc::clone(&tracer),
            noop: BoxedTracer::new(Box::new(NoopTracer::new())),
        })
        .with_filter(telemetry_filter);
    (
        Registry::default().with(vec![log_layer(level, writer).boxed(), telemetry.boxed()]),
        Logging {
            level: level_handle,
            telemetry: telemetry_handle,
            tracer,
        },
    )
}

pub(crate) fn bootstrap() -> Result<Logging, LoggingError> {
    let (subscriber, logging) = subscriber(std::io::stderr);
    subscriber
        .try_init()
        .map_err(|source| LoggingError::SubscriberInitialization { source })?;
    install_panic_hook();
    Ok(logging)
}

fn install_panic_hook() {
    std::panic::set_hook(Box::new(|information| {
        let thread = std::thread::current();
        let file = information
            .location()
            .map_or("unknown", std::panic::Location::file);
        let line = information.location().map_or(0, std::panic::Location::line);
        tracing::error!(
            panic_file = file,
            panic_line = line,
            thread_name = thread.name().unwrap_or("unnamed"),
            "thread panicked"
        );
    }));
}

pub(crate) fn initialize(
    logging: Logging,
    level: LogLevel,
) -> Result<TelemetryGuard, LoggingError> {
    let tracer_provider = if telemetry_enabled_from_environment() {
        Some(build_tracer_provider()?)
    } else {
        None
    };
    configure(logging, level, tracer_provider)
}

fn configure(
    logging: Logging,
    level: LogLevel,
    tracer_provider: Option<SdkTracerProvider>,
) -> Result<TelemetryGuard, LoggingError> {
    let Logging {
        level: level_handle,
        telemetry,
        tracer,
    } = logging;
    level_handle
        .reload(targets(level))
        .map_err(|source| LoggingError::SubscriberReload { source })?;
    if let Some(provider) = &tracer_provider {
        tracer.get_or_init(|| BoxedTracer::new(Box::new(provider.tracer("natsume-server"))));
        telemetry
            .reload(targets(LogLevel::Trace))
            .map_err(|source| LoggingError::SubscriberReload { source })?;
    }
    Ok(TelemetryGuard { tracer_provider })
}

fn build_tracer_provider() -> Result<SdkTracerProvider, LoggingError> {
    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .build()
        // OTLP builder errors can contain configured endpoints or headers; never retain them.
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
        // Exporter shutdown errors can retain endpoint or header secrets; report only the safe classification.
        if let Some(provider) = self.tracer_provider
            && provider.shutdown().is_err()
        {
            tracing::error!(error = %snafu::Report::from_error(LoggingError::ExporterShutdown), "OpenTelemetry trace shutdown failed");
        }
    }
}

#[derive(Debug, Snafu)]
pub(crate) enum LoggingError {
    #[snafu(display("OpenTelemetry OTLP trace exporter initialization failed"))]
    ExporterInitialization,
    #[snafu(display("OpenTelemetry trace shutdown failed"))]
    ExporterShutdown,
    #[snafu(display("structured logging initialization failed"))]
    SubscriberInitialization {
        source: tracing_subscriber::util::TryInitError,
    },
    #[snafu(display("structured logging configuration failed"))]
    SubscriberReload { source: reload::Error },
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
    use tracing_subscriber::{fmt::MakeWriter, layer::SubscriberExt as _, registry::Registry};

    use crate::config::LogLevel;

    static SUBSCRIBER_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn subscriber<W>(level: LogLevel, writer: W) -> impl tracing::Subscriber + Send + Sync + 'static
    where
        W: for<'writer> MakeWriter<'writer> + Send + Sync + 'static,
    {
        Registry::default().with(super::log_layer(super::targets(level), writer))
    }

    pub(crate) struct SubscriberTestGuard {
        _unscoped_dispatch: tracing::Dispatch,
        _guard: MutexGuard<'static, ()>,
    }

    impl SubscriberTestGuard {
        /// Serialises tests that install a scoped subscriber because `tracing`
        /// caches callsite interest process-globally.
        pub(crate) fn acquire() -> Self {
            let guard = SUBSCRIBER_TEST_LOCK
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            Self {
                // Keep a second dispatcher registered without installing it.
                // tracing's single-dispatcher fast path otherwise caches only
                // the unscoped thread's interest on a concurrent first use.
                _unscoped_dispatch: tracing::Dispatch::new(
                    tracing::subscriber::NoSubscriber::default(),
                ),
                _guard: guard,
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
            subscriber(level, self.clone())
        }

        pub(crate) fn text(&self) -> Result<String, ()> {
            // The rejected log buffer or poisoned guard can contain secrets; do not retain it.
            let buffer = self.buffer.lock().map_err(|_| ())?;
            // The rejected log buffer or poisoned guard can contain secrets; do not retain it.
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
                // The rejected log buffer or poisoned guard can contain secrets; do not retain it.
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
    fn scoped_subscriber_observes_callsites_first_used_by_an_unscoped_thread()
    -> Result<(), crate::test_error::TestError> {
        const CHILD: &str = "NATSUME_TRACING_CALLSITE_TEST_CHILD";

        fn emit() {
            tracing::debug!("first-unscoped-callsite-canary");
        }

        if std::env::var_os(CHILD).is_none() {
            // A fresh process guarantees that no other test has registered a
            // dispatcher or this callsite before the unscoped worker uses it.
            let output = std::process::Command::new(std::env::current_exe()?)
                .args([
                    "--exact",
                    "logging::tests::scoped_subscriber_observes_callsites_first_used_by_an_unscoped_thread",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env(CHILD, "1")
                .output()?;
            assert!(
                output.status.success(),
                "isolated callsite test failed:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return Ok(());
        }

        let _subscriber_guard = SubscriberTestGuard::acquire();
        let captured = CapturedLogs::default();
        let dispatcher = tracing::Dispatch::new(captured.subscriber(LogLevel::Debug));
        std::thread::spawn(emit)
            .join()
            .unwrap_or_else(|_| panic!("unscoped worker panicked"));
        tracing::dispatcher::with_default(&dispatcher, emit);
        let output = captured.text().map_err(|()| TestFailure::CaptureFailed)?;
        assert_eq!(output.matches("first-unscoped-callsite-canary").count(), 1);
        Ok(())
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
    fn startup_errors_are_redacted_and_the_configured_level_is_reloaded()
    -> Result<(), crate::test_error::TestError> {
        let _subscriber_guard = SubscriberTestGuard::acquire();
        let captured = CapturedLogs::default();
        let (subscriber, logging) = super::subscriber(captured.clone());
        let fixture = tempfile::tempdir()?;
        let path = fixture.path().join("config.toml");
        std::fs::write(&path, "rejected-secret-canary = [")?;
        let error = crate::config::ServerConfig::load_from(&path)
            .err()
            .unwrap_or_else(|| panic!("invalid configuration was accepted"));
        tracing::subscriber::with_default(subscriber, || {
            fn debug_event() {
                tracing::debug!("reloaded-debug-canary");
            }
            debug_event();
            tracing::error!(config_path = crate::config::CONFIG_PATH, error = %snafu::Report::from_error(error), "server configuration failed");
            let telemetry = super::configure(logging, LogLevel::Debug, None)
                .unwrap_or_else(|error| panic!("{}", snafu::Report::from_error(error)));
            debug_event();
            tracing::info!(target: "dependency", "dependency-event-canary");
            telemetry.shutdown();
        });
        let output = captured
            .text()
            .unwrap_or_else(|()| panic!("log capture failed"));
        assert!(output.starts_with("ERROR server configuration failed config_path=\"/etc/natsume-server/config.toml\" error=invalid TOML syntax or field type near line 1, column "));
        assert!(!output.contains("rejected-secret-canary"));
        assert!(!output.contains("dependency-event-canary"));
        assert!(!output.contains('\u{1b}'));
        assert_eq!(output.matches("reloaded-debug-canary").count(), 1);
        Ok(())
    }

    #[test]
    fn reloaded_telemetry_preserves_remote_parent_and_independent_stderr_filter() {
        use opentelemetry::{propagation::TextMapPropagator as _, trace::TraceContextExt as _};
        use tracing_opentelemetry::OpenTelemetrySpanExt as _;

        let _subscriber_guard = SubscriberTestGuard::acquire();
        let captured = CapturedLogs::default();
        let exporter = CapturedExporter::default();
        let provider = SdkTracerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        let (subscriber, logging) = super::subscriber(captured.clone());
        tracing::subscriber::with_default(subscriber, || {
            let before = tracing::debug_span!("before-telemetry-canary");
            assert!(before.is_disabled());
            let telemetry = super::configure(logging, LogLevel::Info, Some(provider))
                .unwrap_or_else(|error| panic!("{}", snafu::Report::from_error(error)));
            let headers = std::collections::HashMap::from([(
                "traceparent".to_owned(),
                "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01".to_owned(),
            )]);
            let parent =
                opentelemetry_sdk::propagation::TraceContextPropagator::new().extract(&headers);
            let span = tracing::debug_span!("configured-telemetry-canary");
            assert!(span.set_parent(parent).is_ok());
            assert_eq!(
                span.context().span().span_context().trace_id().to_string(),
                "0af7651916cd43dd8448eb211c80319c"
            );
            span.in_scope(|| tracing::debug!("stderr-debug-canary"));
            drop(span);
            telemetry.shutdown();
        });
        let spans = exporter
            .spans
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].name, "configured-telemetry-canary");
        assert_eq!(spans[0].parent_span_id.to_string(), "b7ad6b7169203331");
        assert!(spans[0].parent_span_is_remote);
        let output = captured
            .text()
            .unwrap_or_else(|()| panic!("log capture failed"));
        assert!(!output.contains("stderr-debug-canary"));
    }

    #[derive(Debug, Clone, Default)]
    struct CapturedExporter {
        spans: Arc<Mutex<Vec<SpanData>>>,
    }

    impl SpanExporter for CapturedExporter {
        fn export(
            &self,
            batch: Vec<SpanData>,
        ) -> impl std::future::Future<Output = OTelSdkResult> + Send {
            self.spans
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .extend(batch);
            std::future::ready(Ok(()))
        }
    }

    #[test]
    fn bootstrap_panic_hook_reports_location_and_thread_without_payload()
    -> Result<(), crate::test_error::TestError> {
        const CHILD: &str = "NATSUME_SAFE_PANIC_HOOK_CHILD";
        if std::env::var_os(CHILD).is_some() {
            let _logging = super::bootstrap()
                .unwrap_or_else(|error| panic!("{}", snafu::Report::from_error(error)));
            let worker = std::thread::Builder::new()
                .name("panic-hook-worker".to_owned())
                .spawn(|| panic!("panic-hook-secret-canary"))?;
            assert!(worker.join().is_err());
            return Ok(());
        }
        let output = std::process::Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "logging::tests::bootstrap_panic_hook_reports_location_and_thread_without_payload",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD, "1")
            .output()?;
        assert!(output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("ERROR thread panicked"));
        assert!(stderr.contains("logging.rs"));
        assert!(stderr.contains("panic_line="));
        assert!(stderr.contains("panic-hook-worker"));
        assert!(!stderr.contains("panic-hook-secret-canary"));
        assert!(!String::from_utf8_lossy(&output.stdout).contains("panic-hook-secret-canary"));
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
        let _subscriber_guard = SubscriberTestGuard::acquire();
        let captured = CapturedLogs::default();
        let shutdown_called = Arc::new(AtomicBool::new(false));
        let provider = SdkTracerProvider::builder()
            .with_simple_exporter(ShutdownFailingExporter {
                shutdown_called: Arc::clone(&shutdown_called),
            })
            .build();

        tracing::subscriber::with_default(captured.subscriber(LogLevel::Info), || {
            super::TelemetryGuard {
                tracer_provider: Some(provider),
            }
            .shutdown();
        });

        assert!(shutdown_called.load(Ordering::Relaxed));
        let output = captured
            .text()
            .unwrap_or_else(|()| panic!("log capture failed"));
        assert!(output.contains("ERROR OpenTelemetry trace shutdown failed"));
        assert!(!output.contains("exporter detail must not be logged"));
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
    pub(crate) enum TestFailure {
        #[snafu(display("captured logging output could not be read"))]
        CaptureFailed,
        #[snafu(display("the configured logging level filter changed"))]
        LevelFilterChanged,
    }
}
