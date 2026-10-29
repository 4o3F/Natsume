//! Paged JSONL export over a fixed journal snapshot ceiling.

use std::collections::VecDeque;

use serde::Serialize;

use crate::journal::read::{JournalRecord, PAGE_CAP, ReadPage, ReadRequest, RetainedBounds};

use super::{JournalQuery, QueryError};

/// Tagged `JSONL` records for exported events, pruning metadata or terminal failure.
#[derive(Debug, Serialize, specta::Type)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ExportLine {
    Event {
        /// One validated event from the fixed ID snapshot.
        event: JournalRecord,
        /// Current actor display name, absent for unknown/deleted actors or lookup failure.
        operator_display_name: Option<String>,
    },
    Metadata {
        /// Inclusive high-water ID fixed before the first response byte.
        #[specta(type = Option<specta_typescript::Number>)]
        high_water_id: Option<i64>,
        /// Matching snapshot rows lost before their page could be read.
        #[specta(type = specta_typescript::Number)]
        pruned_rows: u64,
        /// Current retention boundary; missing rows below it were pruned during export.
        #[specta(type = Option<specta_typescript::Number>)]
        pruned_before_id: Option<i64>,
        /// Conservative ID range containing missing snapshot rows; `None` without observed loss.
        pruned_range: Option<PrunedRange>,
    },
    Error {
        /// Closed export termination reason; source details remain in tracing only.
        code: ExportFailure,
    },
}

#[derive(Debug, Serialize, specta::Type)]
pub(crate) struct PrunedRange {
    /// Exclusive original lower cursor, absent for an unbounded start.
    #[specta(type = Option<specta_typescript::Number>)]
    after_id: Option<i64>,
    /// Inclusive snapshot upper bound, reduced by the original before cursor if supplied.
    #[specta(type = Option<specta_typescript::Number>)]
    through_id: Option<i64>,
}

#[derive(Debug, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ExportFailure {
    JournalReadFailed,
}

/// Tracks bounded page consumption and the single export trailer.
enum Phase {
    Pages,
    Metadata,
    Done,
}

/// Owns one fixed-ceiling export and at most one buffered page.
pub(crate) struct Export {
    /// Query dependencies retained only while the response body is consumed.
    query: JournalQuery,
    /// Descending cursor with a permanently fixed high-water ceiling.
    request: ReadRequest,
    /// At most `PAGE_CAP` fully serialized records from the current page.
    pending: VecDeque<ExportLine>,
    /// Matching row count in the opening snapshot, used to detect pruning without assuming dense IDs.
    expected: u64,
    /// Number of records delivered to the response adapter.
    delivered: u64,
    /// Retention bounds captured at startup and refreshed before writing the trailer.
    bounds: RetainedBounds,
    /// Explicit pagination/trailer completion phase.
    phase: Phase,
    /// Original before cursor, retained for the conservative pruning range in the trailer.
    initial_before: Option<i64>,
}

impl JournalQuery {
    /// Captures a fixed ID ceiling and prepares a bounded `JSONL` export.
    pub(crate) async fn export(&self, mut request: ReadRequest) -> Result<Export, QueryError> {
        request.limit = PAGE_CAP;
        let snapshot = self.reader.export_snapshot(request.clone()).await?;
        request.high_water = snapshot.bounds.high_water;
        let page = if snapshot.matching > 0 {
            self.reader.page(request.clone()).await?
        } else {
            ReadPage {
                records: Vec::new(),
                has_more: false,
            }
        };
        let mut export = Export {
            initial_before: request.before,
            query: self.clone(),
            request,
            expected: snapshot.matching,
            delivered: 0,
            bounds: snapshot.bounds,
            pending: VecDeque::new(),
            phase: Phase::Pages,
        };
        export.accept(page).await;
        Ok(export)
    }
}

impl Export {
    async fn accept(&mut self, page: ReadPage) {
        let names = self.query.names(&page.records).await;
        let more = page.has_more;
        self.request.before = page.records.last().map(JournalRecord::id);

        self.pending = page
            .records
            .into_iter()
            .map(|event| {
                let operator_display_name = match &event {
                    JournalRecord::Readable { header, .. } => names
                        .iter()
                        .find(|operator| Some(operator.operator_id) == header.actor_operator_id)
                        .and_then(|operator| operator.display_name.clone()),
                    JournalRecord::Unreadable { .. } => None,
                };
                ExportLine::Event {
                    event,
                    operator_display_name,
                }
            })
            .collect();
        if !more {
            self.phase = Phase::Metadata;
        }
    }

    /// Returns the next `JSONL` record, then one trailer and finally `None`.
    pub(crate) async fn next_line(&mut self) -> Result<Option<Vec<u8>>, QueryError> {
        loop {
            if let Some(line) = self.pending.pop_front() {
                self.delivered += 1;
                return encode(&line).map(Some);
            }
            match self.phase {
                Phase::Pages => {
                    let page = self.query.reader.page(self.request.clone()).await?;
                    self.accept(page).await;
                }
                Phase::Metadata => {
                    self.bounds = self.query.reader.bounds().await?;
                    self.phase = Phase::Done;
                    return encode(&ExportLine::Metadata {
                        high_water_id: self.request.high_water,
                        pruned_rows: self.expected.saturating_sub(self.delivered),
                        pruned_before_id: self.bounds.oldest_id,
                        pruned_range: (self.expected > self.delivered).then(|| PrunedRange {
                            after_id: self.request.after,
                            through_id: match (self.request.high_water, self.initial_before) {
                                (Some(high), Some(before)) => {
                                    Some(high.min(before.saturating_sub(1)))
                                }
                                (high, None) => high,
                                (None, Some(_)) => None,
                            },
                        }),
                    })
                    .map(Some);
                }
                Phase::Done => return Ok(None),
            }
        }
    }
}

/// Serializes one typed export record with its newline delimiter.
pub(crate) fn encode(line: &ExportLine) -> Result<Vec<u8>, QueryError> {
    let mut bytes = serde_json::to_vec(line)?;
    bytes.push(b'\n');
    Ok(bytes)
}
