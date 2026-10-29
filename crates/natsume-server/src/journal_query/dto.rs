//! Typed results for journal queries, reconstruction and runtime status.

use serde::Serialize;
use uuid::Uuid;

use crate::{
    component::device::DeviceId,
    journal::{
        event::types::{DisconnectReason, PauseReason},
        read::JournalRecord,
    },
};

/// Closed writer availability classification exposed to operators.
#[derive(Debug, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Availability {
    Ready,
    Paused,
    Degraded,
}

/// Writer health, retention boundaries and the injected startup offset.
#[derive(Debug, Serialize, specta::Type)]
pub(crate) struct JournalHealth {
    /// Writer availability sampled from process state, independently of the read transaction.
    pub(crate) status: Availability,
    /// True precisely while the writer has a pause reason.
    pub(crate) paused: bool,
    /// Boot-total lost events across every category.
    #[specta(type = specta_typescript::Number)]
    pub(crate) dropped_total: u64,
    /// Earliest known loss bound in Unix milliseconds; `None` before any drop this boot.
    #[specta(type = Option<specta_typescript::Number>)]
    pub(crate) dropped_since: Option<i64>,
    /// Oldest currently retained cursor, absent for an empty journal.
    #[specta(type = Option<specta_typescript::Number>)]
    pub(crate) oldest_retained_id: Option<i64>,
    /// Occurrence time associated with the oldest retained cursor, in Unix milliseconds.
    #[specta(type = Option<specta_typescript::Number>)]
    pub(crate) oldest_retained_at: Option<i64>,
    /// Server-local UTC offset captured at startup, in minutes; UTC fallback on startup failure.
    pub(crate) server_utc_offset_minutes: i16,
}

/// A durable actor ID paired with its optional current account name.
#[derive(Debug, Serialize, specta::Type)]
pub(crate) struct OperatorDisplayName {
    /// Durable operator identity, retained even when the account has been deleted.
    pub(crate) operator_id: Uuid,
    /// Current account display name; `None` on missing account or name lookup failure.
    pub(crate) display_name: Option<String>,
}

/// A descending event page with continuation, names and health metadata.
#[derive(Debug, Serialize, specta::Type)]
pub(crate) struct EventPage {
    /// Validated journal records ordered by descending ID.
    pub(crate) events: Vec<JournalRecord>,
    /// True when more records match this cursor range.
    pub(crate) has_more: bool,
    /// Continue with this before cursor and the same filters/after lower bound.
    #[specta(type = Option<specta_typescript::Number>)]
    pub(crate) next_before: Option<i64>,
    /// Current actor names resolved in one bounded business read per page.
    pub(crate) operators: Vec<OperatorDisplayName>,
    /// Writer health and current retention boundary.
    pub(crate) journal: JournalHealth,
}

/// Boot counters for one journal category, sampled independently.
#[derive(Debug, Serialize, specta::Type)]
pub(crate) struct CategoryStatus {
    /// Boot-total lost events in this category.
    #[specta(type = specta_typescript::Number)]
    pub(crate) dropped: u64,
    /// Boot-total serialization/write failures in this category.
    #[specta(type = specta_typescript::Number)]
    pub(crate) write_failed: u64,
    /// Events still awaiting a final writer outcome, including an in-flight batch.
    #[specta(type = specta_typescript::Number)]
    pub(crate) queued: u64,
    /// Earliest known category loss in Unix milliseconds.
    #[specta(type = Option<specta_typescript::Number>)]
    pub(crate) dropped_since: Option<i64>,
}

/// Named category counters without numeric indexing or enum encodings.
#[derive(Debug, Serialize, specta::Type)]
pub(crate) struct CategoriesStatus {
    /// Device-category producer/writer counters.
    pub(crate) device: CategoryStatus,
    /// Operator request counters.
    pub(crate) audit: CategoryStatus,
    /// Authentication decision counters.
    pub(crate) auth: CategoryStatus,
    /// Journal lifecycle and gap counters.
    pub(crate) journal: CategoryStatus,
}

/// Admin-only writer counters and optional filesystem measurements.
#[derive(Debug, Serialize, specta::Type)]
pub(crate) struct JournalStatus {
    /// Availability and retention metadata.
    pub(crate) journal: JournalHealth,
    /// Estimated channel occupancy, excluding the in-flight writer batch.
    #[specta(type = specta_typescript::Number)]
    pub(crate) queue_depth: usize,
    /// Explicit named category counters; writes fail at batch scope, counted below.
    pub(crate) categories: CategoriesStatus,
    /// Boot-total serialization/write failures across all categories.
    #[specta(type = specta_typescript::Number)]
    pub(crate) write_failed: u64,
    /// Current writer pause reason, absent while writable.
    pub(crate) pause_reason: Option<PauseReason>,
    /// Last observed failure classification, absent before any failure or after successful reopen.
    pub(crate) last_error: Option<crate::journal::ErrorClass>,
    /// Boot-total producer/flush deadline expirations.
    #[specta(type = specta_typescript::Number)]
    pub(crate) shutdown_timeouts: u64,
    /// Main `SQLite` file size in bytes; unknown if sampling fails.
    #[specta(type = Option<specta_typescript::Number>)]
    pub(crate) db_bytes: Option<u64>,
    /// WAL sidecar size in bytes; zero when absent, unknown if sampling fails.
    #[specta(type = Option<specta_typescript::Number>)]
    pub(crate) wal_bytes: Option<u64>,
    /// Shared-memory sidecar size in bytes; zero when absent, unknown if sampling fails.
    #[specta(type = Option<specta_typescript::Number>)]
    pub(crate) shm_bytes: Option<u64>,
    /// Available filesystem bytes; unknown if sampling fails.
    #[specta(type = Option<specta_typescript::Number>)]
    pub(crate) free_bytes: Option<u64>,
}

/// Connection facts and windowed terminal counts from retained device events.
#[derive(Debug, Serialize, specta::Type)]
pub(crate) struct DeviceSummary {
    /// Device identity obtained solely from device journal facts.
    pub(crate) device_id: DeviceId,
    /// Latest connection occurrence time in Unix milliseconds.
    #[specta(type = Option<specta_typescript::Number>)]
    pub(crate) last_connected_at: Option<i64>,
    /// Latest terminal occurrence time in Unix milliseconds.
    #[specta(type = Option<specta_typescript::Number>)]
    pub(crate) last_disconnected_at: Option<i64>,
    /// Closed reason for the latest terminal; `None` when unknown or replaced.
    pub(crate) last_disconnect_reason: Option<DisconnectReason>,
    /// Known terminal count within the requested wall-clock window.
    #[specta(type = specta_typescript::Number)]
    pub(crate) disconnects: u64,
}

/// Retained per-device summaries accompanied by journal health.
#[derive(Debug, Serialize, specta::Type)]
pub(crate) struct DeviceSummaries {
    /// One summary per device with retained journal facts.
    pub(crate) devices: Vec<DeviceSummary>,
    /// Health and retention metadata explaining incomplete history.
    pub(crate) journal: JournalHealth,
}
