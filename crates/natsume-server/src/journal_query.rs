//! Read-only journal coordination, separate from device mutation/runtime coordination.

use std::sync::Arc;

use snafu::Snafu;
use time::UtcOffset;

use crate::{
    component::operator::OperatorComponent,
    device_control::LiveLeaseView,
    journal::{
        CategorySnapshot, JournalStatsSnapshot,
        event::{Event, types::Category},
        read::{
            Filter, JournalReader, JournalRecord, PAGE_CAP, ReadError, ReadOrder, ReadPage,
            ReadRequest, RetainedBounds,
        },
    },
};

pub(crate) mod dto;
mod export;
mod timeline;

use self::dto::{
    Availability, CategoriesStatus, CategoryStatus, EventPage, JournalHealth, JournalStatus,
    OperatorDisplayName,
};

/// Encodes the closed termination record used after an export has sent its HTTP headers.
pub(crate) fn export_error_line() -> Result<Vec<u8>, QueryError> {
    export::encode(&export::ExportLine::Error {
        code: export::ExportFailure::JournalReadFailed,
    })
}

/// Initial/head page size; increment callers continue via `next_before` within the same after bound.
pub(crate) const DEFAULT_PAGE: u16 = 500;

/// Operator permission scope applied to journal event filters.
#[derive(Clone, Copy)]
pub(crate) enum Visibility {
    DeviceOnly,
    All,
}

/// Coordinates bounded journal reads, actor names and a narrow live-lease view.
#[derive(Clone)]
pub(crate) struct JournalQuery {
    /// Bounded journal read side; cannot mutate storage or control the writer.
    reader: JournalReader,
    /// Operator owner performs only batch display-name reads for this coordinator.
    operator: Arc<OperatorComponent>,
    /// Narrow runtime view; neither owns nor depends on `DeviceControl`.
    live: LiveLeaseView,
    /// Server-local offset captured before process threads start; reused by every response.
    utc_offset: UtcOffset,
}

impl JournalQuery {
    /// Constructs read-only coordination with an offset captured before runtime startup.
    pub(crate) const fn new(
        reader: JournalReader,
        operator: Arc<OperatorComponent>,
        live: LiveLeaseView,
        utc_offset: UtcOffset,
    ) -> Self {
        Self {
            reader,
            operator,
            live,
            utc_offset,
        }
    }

    /// Applies the caller's category scope and rejects explicit restricted filters.
    pub(crate) fn authorize(filter: &mut Filter, visibility: Visibility) -> Result<(), QueryError> {
        if matches!(visibility, Visibility::DeviceOnly) {
            if filter
                .categories
                .iter()
                .any(|category| *category != Category::Device)
                || filter
                    .kinds
                    .iter()
                    .any(|kind| Event::category_for_kind(kind) != Some(Category::Device))
            {
                return Err(QueryError::Forbidden);
            }
            filter.categories = vec![Category::Device];
        }
        Ok(())
    }

    /// Returns a descending cursor page with names and retention metadata.
    pub(crate) async fn events(
        &self,
        mut request: ReadRequest,
        visibility: Visibility,
    ) -> Result<EventPage, QueryError> {
        Self::authorize(&mut request.filter, visibility)?;
        let bounds = self.reader.bounds().await?;
        request.high_water = request.high_water.or(bounds.high_water);
        let page = if bounds.high_water.is_some() {
            self.reader.page(request).await?
        } else {
            ReadPage {
                records: Vec::new(),
                has_more: false,
            }
        };
        let has_more = page.has_more;
        let next_before = has_more
            .then(|| page.records.last().map(JournalRecord::id))
            .flatten();
        let operators = self.names(&page.records).await;
        Ok(EventPage {
            events: page.records,
            has_more,
            next_before,
            operators,
            journal: self.health(&bounds),
        })
    }

    async fn names(&self, records: &[JournalRecord]) -> Vec<OperatorDisplayName> {
        let mut ids = records
            .iter()
            .filter_map(|record| match record {
                JournalRecord::Readable { header, .. } => header.actor_operator_id,
                JournalRecord::Unreadable { .. } => None,
            })
            .collect::<Vec<_>>();
        ids.sort_unstable();
        ids.dedup();
        if ids.is_empty() {
            return Vec::new();
        }
        let names = self.operator.display_names(ids.clone()).await.unwrap_or_else(|error| {
            tracing::warn!(error = %snafu::Report::from_error(error), "journal operator names unavailable");
            std::collections::HashMap::new()
        });
        ids.into_iter()
            .map(|id| OperatorDisplayName {
                operator_id: id,
                display_name: names.get(&id).cloned(),
            })
            .collect()
    }

    fn health(&self, bounds: &RetainedBounds) -> JournalHealth {
        let stats = self.reader.stats();
        let categories = &stats.categories;
        JournalHealth {
            status: if stats.degraded {
                Availability::Degraded
            } else if stats.paused {
                Availability::Paused
            } else {
                Availability::Ready
            },
            paused: stats.paused,
            dropped_total: categories
                .device
                .dropped
                .saturating_add(categories.audit.dropped)
                .saturating_add(categories.auth.dropped)
                .saturating_add(categories.journal.dropped),
            dropped_since: categories
                .device
                .first_unreported_drop_ms
                .into_iter()
                .chain(categories.audit.first_unreported_drop_ms)
                .chain(categories.auth.first_unreported_drop_ms)
                .chain(categories.journal.first_unreported_drop_ms)
                .min(),
            oldest_retained_id: bounds.oldest_id,
            oldest_retained_at: bounds.oldest_at,
            server_utc_offset_minutes: self.utc_offset.whole_minutes(),
        }
    }

    /// Samples admin-visible writer health and optional filesystem measurements.
    pub(crate) async fn status(&self) -> Result<JournalStatus, QueryError> {
        let bounds = match self.reader.bounds().await {
            Ok(bounds) => bounds,
            Err(ReadError::Unavailable) => RetainedBounds::default(),
            Err(error) => return Err(error.into()),
        };
        let stats = self.reader.stats();
        let usage = self.reader.disk_usage().await.inspect_err(|error| tracing::warn!(error = %snafu::Report::from_error(error), "journal disk status unavailable")).ok();
        Ok(JournalStatus {
            journal: self.health(&bounds),
            queue_depth: stats.queue_depth,
            categories: categories_status(&stats),
            write_failed: stats.write_failed,
            pause_reason: stats.pause_reason,
            last_error: stats.last_error,
            shutdown_timeouts: stats.shutdown_timeouts,
            db_bytes: usage.map(|usage| usage.db_bytes),
            wal_bytes: usage.map(|usage| usage.wal_bytes),
            shm_bytes: usage.map(|usage| usage.shm_bytes),
            free_bytes: usage.map(|usage| usage.free_bytes),
        })
    }

    async fn scan_page(&self, request: &mut ReadRequest) -> Result<ReadPage, QueryError> {
        if request.high_water.is_none() {
            return Ok(ReadPage {
                records: Vec::new(),
                has_more: false,
            });
        }
        request.limit = PAGE_CAP;
        request.order = ReadOrder::OldestFirst;
        let page = self.reader.page(request.clone()).await?;

        request.after = page.records.last().map(JournalRecord::id).or(request.after);
        Ok(page)
    }
}

fn now_ms() -> i64 {
    i64::try_from(time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000)
        .unwrap_or(i64::MAX)
}

fn categories_status(stats: &JournalStatsSnapshot) -> CategoriesStatus {
    fn category(snapshot: &CategorySnapshot) -> CategoryStatus {
        CategoryStatus {
            dropped: snapshot.dropped,
            write_failed: snapshot.write_failed,
            queued: snapshot.queued,
            dropped_since: snapshot.first_unreported_drop_ms,
        }
    }
    CategoriesStatus {
        device: category(&stats.categories.device),
        audit: category(&stats.categories.audit),
        auth: category(&stats.categories.auth),
        journal: category(&stats.categories.journal),
    }
}

#[derive(Debug, Snafu)]
/// Typed query failures mapped to redacted HTTP or `JSONL` outcomes.
pub(crate) enum QueryError {
    #[snafu(context(false), display("journal query read failed"))]
    Read {
        /// Original typed source retained for tracing and error classification.
        source: ReadError,
    },
    #[snafu(display("journal filter is not permitted"))]
    Forbidden,
    #[snafu(display("journal query number is invalid"))]
    Number {
        /// Original typed source retained for tracing and error classification.
        source: std::num::ParseIntError,
    },
    #[snafu(display("journal query ID is invalid"))]
    Id {
        /// Original typed source retained for tracing and error classification.
        source: uuid::Error,
    },
    #[snafu(display("journal query is invalid"))]
    Invalid,
    #[snafu(context(false), display("journal export serialization failed"))]
    Serialize {
        /// Original typed source retained for tracing and error classification.
        source: serde_json::Error,
    },
}

#[cfg(test)]
pub(crate) mod tests;
