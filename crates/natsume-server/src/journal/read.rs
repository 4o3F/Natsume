//! Bounded, validated journal reads without writer ownership or business queries.

use std::{path::PathBuf, sync::Arc};

use serde::Serialize;
use snafu::Snafu;
use tokio::sync::watch;
use uuid::Uuid;

use super::{
    JournalStats, JournalStatsSnapshot,
    db::{JournalDatabase, StoredEvent},
    event::{
        Event, ReadEvent,
        types::{Category, Severity},
    },
};
use crate::component::device::DeviceId;

/// Hard bound on rows materialized by one journal read transaction.
pub(crate) const PAGE_CAP: u16 = 1000;

/// Typed predicates applied before journal ordering and page limits.
#[derive(Clone, Default)]
pub(crate) struct Filter {
    /// Allowed categories; empty means all categories.
    pub(crate) categories: Vec<Category>,
    /// Known event kinds; empty means all kinds, including unreadable rows.
    pub(crate) kinds: Vec<String>,
    /// Allowed severities; empty means all severities.
    pub(crate) severities: Vec<Severity>,
    /// Exact canonical device identity, if requested.
    pub(crate) device_id: Option<DeviceId>,
    /// Literal committed seat prefix, never a SQL pattern.
    pub(crate) seat_prefix: Option<String>,
    /// Exact authenticated operator identity, if requested.
    pub(crate) actor_operator_id: Option<Uuid>,
    /// Exact operation correlation, if requested.
    pub(crate) operation_id: Option<Uuid>,
    /// Exact request correlation, if requested.
    pub(crate) request_id: Option<Uuid>,
    /// Inclusive lower occurrence time in Unix milliseconds.
    pub(crate) from: Option<i64>,
    /// Inclusive upper occurrence time in Unix milliseconds.
    pub(crate) to: Option<i64>,
}

/// ID direction shared by event pages and causal reconstruction scans.
#[derive(Clone, Copy, Default)]
pub(crate) enum ReadOrder {
    #[default]
    NewestFirst,
    OldestFirst,
}

/// Indexed device identities without event payloads, for summary lookup pagination.
pub(crate) struct SummaryDevicePage {
    /// Validated device identities appearing in the retained device category.
    pub(crate) devices: Vec<DeviceId>,
    /// Opaque persisted index cursor; None after the final bounded identity page.
    pub(crate) next_cursor: Option<String>,
}

/// A bounded journal page with explicit cursors and snapshot ceiling.
#[derive(Clone, Default)]
pub(crate) struct ReadRequest {
    /// SQL filters applied before ordering and limiting.
    pub(crate) filter: Filter,
    /// Exclusive upper ID cursor, for descending pages.
    pub(crate) before: Option<i64>,
    /// Exclusive lower ID cursor, for incremental pages.
    pub(crate) after: Option<i64>,
    /// Inclusive export snapshot ceiling, unaffected by subsequent inserts.
    pub(crate) high_water: Option<i64>,
    /// Requested rows, always capped at `PAGE_CAP` by the database adapter.
    pub(crate) limit: u16,
    /// Optional device/global stream used solely for timeline reconstruction.
    pub(crate) timeline_device: Option<DeviceId>,
    /// Causal scans use ascending IDs; list/export pages remain descending.
    pub(crate) order: ReadOrder,
}

/// Validated event data or bounded metadata for an unreadable stored row.
#[derive(Debug, Serialize, specta::Type)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum JournalRecord {
    Readable {
        /// Validated common event facts, separate from persisted SQL row types.
        header: Box<RecordHeader>,
        /// Versioned details rebuilt from the sealed event model, never raw stored JSON.
        details: EventDetails,
    },
    Unreadable {
        /// Database cursor identity, safe even when the remainder cannot be decoded.
        #[specta(type = specta_typescript::Number)]
        id: i64,
        /// Known closed kind or `unknown`; never arbitrary persisted text.
        kind: String,
    },
}

/// Versioned output rebuilt exclusively through the sealed event writer.
#[derive(Debug, Serialize, specta::Type)]
pub(crate) struct EventDetails {
    /// Supported event format version, currently one.
    pub(crate) v: u16,
    /// Sealed payload serialized through `DetailsWriter`'s whitelist.
    #[serde(flatten)]
    pub(crate) event: Event,
}

/// Validated common facts for a readable journal event.
#[derive(Debug, Serialize, specta::Type)]
pub(crate) struct RecordHeader {
    /// Monotonic `SQLite` row ID used for cursors.
    #[specta(type = specta_typescript::Number)]
    pub(crate) id: i64,
    /// Wall-clock occurrence time in Unix milliseconds, possibly non-monotonic.
    #[specta(type = specta_typescript::Number)]
    pub(crate) occurred_at_ms: i64,
    /// Process identity stamped by the writer.
    pub(crate) boot_id: Uuid,
    /// Category derived from the decoded event, never trusted from stored text.
    pub(crate) category: Category,
    /// Severity derived from the decoded event.
    pub(crate) severity: Severity,
    /// Canonical device identity, absent for non-device work.
    pub(crate) device_id: Option<DeviceId>,
    /// Canonical lease identity, absent for non-lease work.
    pub(crate) session_id: Option<Uuid>,
    /// Authenticated actor identity, absent when unknown.
    pub(crate) actor_operator_id: Option<Uuid>,
    /// Request correlation supplied at the HTTP boundary.
    pub(crate) request_id: Option<Uuid>,
    /// Application operation identity, when applicable.
    pub(crate) operation_id: Option<Uuid>,
    /// Valid 32-digit lowercase OpenTelemetry trace ID, when captured.
    pub(crate) trace_id: Option<String>,
    /// Committed seat snapshot, absent without a binding fact.
    pub(crate) seat_code: Option<super::event::types::SeatCode>,
}

impl JournalRecord {
    /// Returns the cursor ID even when the stored event is unreadable.
    pub(crate) fn id(&self) -> i64 {
        match self {
            Self::Readable { header, .. } => header.id,
            Self::Unreadable { id, .. } => *id,
        }
    }

    /// Validates persisted facts and suppresses raw text from unreadable records.
    pub(super) fn from_row(id: i64, row: StoredEvent) -> Self {
        let kind = Event::KINDS
            .iter()
            .find(|kind| **kind == row.kind)
            .copied()
            .unwrap_or("unknown");
        let unreadable = || Self::Unreadable {
            id,
            kind: kind.into(),
        };
        let ReadEvent::Known(event) = ReadEvent::from_persisted(id, &row.kind, &row.details) else {
            return unreadable();
        };
        if row.category != event.category().as_str() || row.severity != event.severity().as_str() {
            return Self::Unreadable {
                id,
                kind: "unknown".into(),
            };
        }
        let parsed = (|| {
            let uuid = |raw: Option<String>| {
                raw.map(|raw| Uuid::parse_str(&raw).ok())
                    .map_or(Some(None), |id| id.map(Some))
            };
            let device_id = match row.device_id {
                Some(raw) => Some(Some(DeviceId::parse(&raw)?)),
                None => Some(None),
            }?;
            let trace_id = match row.trace_id {
                Some(raw)
                    if raw.len() == 32
                        && raw
                            .bytes()
                            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                        && raw != "00000000000000000000000000000000" =>
                {
                    Some(raw)
                }
                Some(_) => return None,
                None => None,
            };
            let seat_code = row
                .seat_code
                .map(|value| serde_json::from_value(serde_json::Value::String(value)))
                .transpose()
                .ok()?;
            Some(RecordHeader {
                id,
                occurred_at_ms: row.occurred_at_ms,
                boot_id: Uuid::parse_str(&row.boot_id).ok()?,
                category: event.category(),
                severity: event.severity(),
                device_id,
                session_id: uuid(row.session_id)?,
                actor_operator_id: uuid(row.actor_operator_id)?,
                request_id: uuid(row.request_id)?,
                operation_id: uuid(row.operation_id)?,
                trace_id,
                seat_code,
            })
        })();
        match parsed {
            Some(header) => Self::Readable {
                header: Box::new(header),
                details: EventDetails { v: 1, event },
            },
            None => unreadable(),
        }
    }
}

/// Retention endpoints sampled without loading event payloads.
#[derive(Clone, Debug, Default)]
pub(crate) struct RetainedBounds {
    /// Oldest retained ID; `None` for an empty journal.
    pub(crate) oldest_id: Option<i64>,
    /// Occurrence time of the oldest retained row, in Unix milliseconds.
    pub(crate) oldest_at: Option<i64>,
    /// Largest currently retained ID; `None` for an empty journal.
    pub(crate) high_water: Option<i64>,
}

/// Bounded validated records and lookahead availability for the same cursor range.
pub(crate) struct ReadPage {
    /// At most `PAGE_CAP` validated records in the requested ID order.
    pub(crate) records: Vec<JournalRecord>,
    /// True when a bounded lookahead found another matching event.
    pub(crate) has_more: bool,
}

/// Export ceiling and expected row count captured in one event-free read transaction.
pub(crate) struct ExportSnapshot {
    /// Retention bounds at export start, including its immutable high-water ID.
    pub(crate) bounds: RetainedBounds,
    /// Matching snapshot rows before paging, counted once to detect export pruning.
    pub(crate) matching: u64,
}

/// Cloneable read side sharing writer publication without writer ownership.
#[derive(Clone)]
pub(crate) struct JournalReader {
    /// Writer-published pool; no read path opens, migrates or writes a database.
    pub(super) database: watch::Receiver<Option<JournalDatabase>>,
    /// Configured filesystem location; absent for an inert channel with no writer.
    pub(super) path: Option<PathBuf>,
    /// Shared producer/writer counters sampled without holding a database transaction.
    pub(super) stats: Arc<JournalStats>,
}

impl JournalReader {
    /// Loads a bounded page, using indexed stream merging for timelines.
    pub(crate) async fn page(&self, request: ReadRequest) -> Result<ReadPage, ReadError> {
        let database = self
            .database
            .borrow()
            .clone()
            .ok_or(ReadError::Unavailable)?;
        database
            .page(request)
            .await
            .map_err(|source| ReadError::Database { source })
    }

    /// Reads one indexed page of summary identities without materializing event payloads.
    pub(crate) async fn summary_devices(
        &self,
        after: Option<String>,
        high_water: i64,
    ) -> Result<SummaryDevicePage, ReadError> {
        let database = self
            .database
            .borrow()
            .clone()
            .ok_or(ReadError::Unavailable)?;
        database
            .summary_devices(after, high_water)
            .await
            .map_err(|source| ReadError::Database { source })
    }

    /// Reads retention bounds without loading any event payloads.
    pub(crate) async fn bounds(&self) -> Result<RetainedBounds, ReadError> {
        let database = self
            .database
            .borrow()
            .clone()
            .ok_or(ReadError::Unavailable)?;
        database
            .bounds()
            .await
            .map_err(|source| ReadError::Database { source })
    }

    /// Fixes export bounds and counts its filtered snapshot once, without loading payloads.
    pub(crate) async fn export_snapshot(
        &self,
        request: ReadRequest,
    ) -> Result<ExportSnapshot, ReadError> {
        let database = self
            .database
            .borrow()
            .clone()
            .ok_or(ReadError::Unavailable)?;
        database
            .export_snapshot(request)
            .await
            .map_err(|source| ReadError::Database { source })
    }

    /// Samples the shared boot counters without opening a transaction.
    pub(crate) fn stats(&self) -> JournalStatsSnapshot {
        super::snapshot(&self.stats)
    }

    /// Samples file lengths and available space outside the async executor.
    pub(crate) async fn disk_usage(&self) -> Result<super::budget::DiskUsage, ReadError> {
        let path = self.path.clone().ok_or(ReadError::Unavailable)?;
        tokio::task::spawn_blocking(move || super::writer::disk_sizes(&path))
            .await
            .map_err(|source| ReadError::Task {
                source: source.into(),
            })?
            .map_err(|source| ReadError::Filesystem { source })
    }
}

/// Storage and filesystem failures preserved at the journal read boundary.
#[derive(Debug, Snafu)]
pub(crate) enum ReadError {
    #[snafu(display("journal is unavailable"))]
    Unavailable,
    #[snafu(display("journal read failed"))]
    Database {
        /// Original typed source retained for tracing and error classification.
        source: crate::db::PersistenceError,
    },
    #[snafu(display("journal filesystem sample failed"))]
    Filesystem {
        /// Original typed source retained for tracing and error classification.
        source: super::writer::DiskError,
    },
    #[snafu(display("journal filesystem task failed"))]
    Task {
        /// Original typed source retained for tracing and error classification.
        source: crate::task::TaskFailure,
    },
}
