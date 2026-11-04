use super::*;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tracing::{
    Event, Subscriber,
    field::{Field, Visit},
};
use tracing_subscriber::{Layer, layer::Context, prelude::*};

static SUBSCRIBER_LOCK: Mutex<()> = Mutex::new(());

pub(crate) struct SubscriberGuard {
    _guard: std::sync::MutexGuard<'static, ()>,
    _dispatch: tracing::Dispatch,
}

pub(crate) fn subscriber_guard() -> SubscriberGuard {
    SubscriberGuard {
        _guard: SUBSCRIBER_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        _dispatch: tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default()),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Value {
    Text(String),
    Unsigned(u64),
    Boolean(bool),
}
#[derive(Clone, Debug)]
pub(crate) struct Record {
    pub(crate) message: String,
    pub(crate) fields: BTreeMap<String, Value>,
    pub(crate) level: tracing::Level,
}
#[derive(Clone, Default)]
pub(crate) struct Capture(Arc<Mutex<Vec<Record>>>);
impl Visit for Record {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.record_str(field, &format!("{value:?}"));
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_owned();
        } else {
            self.fields
                .insert(field.name().to_owned(), Value::Text(value.to_owned()));
        }
    }
    fn record_u64(&mut self, field: &Field, value: u64) {
        self.fields
            .insert(field.name().to_owned(), Value::Unsigned(value));
    }
    fn record_bool(&mut self, field: &Field, value: bool) {
        self.fields
            .insert(field.name().to_owned(), Value::Boolean(value));
    }
}
impl<S: Subscriber> Layer<S> for Capture {
    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        let mut record = Record {
            message: String::new(),
            fields: BTreeMap::new(),
            level: *event.metadata().level(),
        };
        event.record(&mut record);
        self.0
            .lock()
            .unwrap_or_else(|e| panic!("capture: {e}"))
            .push(record);
    }
}
impl Capture {
    pub(crate) fn subscriber(&self) -> impl Subscriber + Send + Sync + use<> {
        tracing_subscriber::registry().with(self.clone())
    }
    pub(crate) fn records(&self, message: &str) -> Vec<Record> {
        self.0
            .lock()
            .unwrap_or_else(|e| panic!("capture: {e}"))
            .iter()
            .filter(|record| record.message == message)
            .cloned()
            .collect()
    }
}
pub(crate) fn context() -> PlanContext {
    PlanContext {
        plan: 1,
        target: TargetMetadata::new(
            &super::super::tests::snapshot()
                .target
                .unwrap_or_else(|| panic!("target")),
        ),
    }
}

#[test]
fn stages_partition_success_and_early_exit_without_residual() {
    let start = Instant::now();
    let mut timings = StageTimings::new(start);
    for (i, next) in [
        Stage::Configuration,
        Stage::Maintenance,
        Stage::Access,
        Stage::Power,
        Stage::Snapshot,
    ]
    .into_iter()
    .enumerate()
    {
        timings.advance(next, start + Duration::from_micros((i as u64 + 1) * 11));
    }
    let total = timings.finish(start + Duration::from_micros(66));
    assert_eq!(timings.inspect_guard, Some(Duration::from_micros(11)));
    assert_eq!(timings.configuration, Some(Duration::from_micros(11)));
    assert_eq!(timings.maintenance, Some(Duration::from_micros(11)));
    assert_eq!(timings.access, Some(Duration::from_micros(11)));
    assert_eq!(timings.power, Some(Duration::from_micros(11)));
    assert_eq!(timings.snapshot, Some(Duration::from_micros(11)));
    assert_eq!(total, Duration::from_micros(66));
    let mut early = StageTimings::new(start);
    assert_eq!(
        early.finish(start + Duration::from_micros(13)),
        Duration::from_micros(13)
    );
    assert_eq!(early.inspect_guard, Some(Duration::from_micros(13)));
    assert!(early.configuration.is_none());
    assert!(early.maintenance.is_none());
    assert!(early.access.is_none());
    assert!(early.power.is_none());
    assert!(early.snapshot.is_none());
}

pub(crate) fn assert_text(record: &Record, field: &str, expected: &str) {
    assert_eq!(record.level, tracing::Level::INFO);
    assert_eq!(
        record.fields.get(field),
        Some(&Value::Text(expected.to_owned()))
    );
}

pub(crate) fn assert_partition(record: &Record) {
    let Some(Value::Unsigned(elapsed)) = record.fields.get("elapsed_us") else {
        panic!("elapsed");
    };
    let sum = [
        "inspect_guard_us",
        "configuration_us",
        "maintenance_us",
        "access_us",
        "power_us",
        "snapshot_us",
    ]
    .iter()
    .map(|field| match record.fields.get(*field) {
        Some(Value::Unsigned(value)) => *value,
        None => 0,
        _ => panic!("stage type"),
    })
    .sum::<u64>();
    assert!(sum.abs_diff(*elapsed) <= 6.max(elapsed / 20));
}

#[tokio::test]
async fn real_reconcile_records_each_outcome_and_failed_stage_once() {
    use super::super::tests::ready_fixture;
    use tokio_util::sync::CancellationToken;
    use tracing::instrument::WithSubscriber as _;
    let _guard = subscriber_guard();
    let capture = Capture::default();
    async {
        let (fixture, mut target) = ready_fixture()
            .await
            .unwrap_or_else(|e| panic!("fixture: {e}"));
        let context = context();
        let before = capture.records("reconcile completed").len();
        let fence = CancellationToken::new();
        fixture
            .snapshots
            .begin_plan(&fence)
            .unwrap_or_else(|e| panic!("plan: {e}"));
        let result = fixture
            .snapshots
            .reconcile(&target, fence, &context)
            .await
            .unwrap_or_else(|e| panic!("reconcile: {e}"));
        assert!(!result.retry);
        let records = capture.records("reconcile completed");
        assert_eq!(records.len(), before + 1);
        assert_text(&records[before], "outcome", "completed");
        assert_partition(&records[before]);
        target.home_epoch = Some(7);
        fixture
            .helper
            .lock()
            .unwrap_or_else(|e| panic!("helper: {e}"))
            .home_failures = 1;
        let fence = CancellationToken::new();
        fixture
            .snapshots
            .begin_plan(&fence)
            .unwrap_or_else(|e| panic!("plan: {e}"));
        assert!(
            fixture
                .snapshots
                .reconcile(&target, fence, &context)
                .await
                .unwrap_or_else(|e| panic!("retry: {e}"))
                .retry
        );
        let cancelled = CancellationToken::new();
        cancelled.cancel();
        assert!(matches!(
            fixture
                .snapshots
                .reconcile(&target, cancelled, &context)
                .await,
            Err(SnapshotError::Cancelled)
        ));
        std::fs::write(
            fixture.directory.path().join("caddy-fixture"),
            "#!/bin/sh\nexit 1\n",
        )
        .unwrap_or_else(|e| panic!("write: {e}"));
        let fence = CancellationToken::new();
        fixture
            .snapshots
            .begin_plan(&fence)
            .unwrap_or_else(|e| panic!("plan: {e}"));
        assert!(matches!(
            fixture.snapshots.reconcile(&target, fence, &context).await,
            Err(SnapshotError::Caddy)
        ));
        let records = capture.records("reconcile completed");
        assert_eq!(records.len(), before + 4);
        for (record, outcome) in
            records[before..]
                .iter()
                .zip(["completed", "retry", "cancelled", "error"])
        {
            assert_text(record, "outcome", outcome);
            assert_eq!(
                record.fields.get("plan"),
                Some(&Value::Unsigned(context.plan))
            );
            assert_text(record, "target_digest", &context.target.target_digest);
            assert_partition(record);
        }
        assert!(!records[before + 2].fields.contains_key("configuration_us"));
        assert!(!records[before + 3].fields.contains_key("configuration_us"));
    }
    .with_subscriber(capture.subscriber())
    .await;
}
