//! Typed journal filters, bounded pages and event-free snapshot metadata.

use diesel::{
    EscapeExpressionMethods as _, ExpressionMethods as _, OptionalExtension as _, QueryDsl as _,
    RunQueryDsl as _, TextExpressionMethods as _,
};

use crate::{
    component::device::DeviceId,
    db::{PersistenceError, Transaction, TransactionError},
    journal::{
        event::{Event, types::Category},
        read::{
            ExportSnapshot, JournalRecord, PAGE_CAP, ReadOrder, ReadPage, ReadRequest,
            RetainedBounds, SummaryDevicePage,
        },
    },
};

use super::{JournalDatabase, StoredEvent, events};

fn filtered(request: &ReadRequest) -> events::BoxedQuery<'static, diesel::sqlite::Sqlite> {
    let filter = &request.filter;
    let mut query = events::table.into_boxed();
    if !filter.categories.is_empty() {
        query = query.filter(
            events::category.eq_any(
                filter
                    .categories
                    .iter()
                    .map(|category| category.as_str().to_owned())
                    .collect::<Vec<_>>(),
            ),
        );
    }
    if !filter.kinds.is_empty() {
        query = query.filter(events::kind.eq_any(filter.kinds.clone()));
    }
    if !filter.severities.is_empty() {
        query = query.filter(
            events::severity.eq_any(
                filter
                    .severities
                    .iter()
                    .map(|severity| severity.as_str().to_owned())
                    .collect::<Vec<_>>(),
            ),
        );
    }
    if let Some(id) = filter.device_id {
        query = query.filter(events::device_id.eq(id.as_text()));
    }
    if let Some(prefix) = &filter.seat_prefix {
        let pattern = format!(
            "{}%",
            prefix
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        );
        query = query.filter(events::seat_code.like(pattern).escape('\\'));
    }
    if let Some(id) = filter.actor_operator_id {
        query = query.filter(events::actor_operator_id.eq(id.to_string()));
    }
    if let Some(id) = filter.operation_id {
        query = query.filter(events::operation_id.eq(id.to_string()));
    }
    if let Some(id) = filter.request_id {
        query = query.filter(events::request_id.eq(id.to_string()));
    }
    if let Some(from) = filter.from {
        query = query.filter(events::occurred_at_ms.ge(from));
    }
    if let Some(to) = filter.to {
        query = query.filter(events::occurred_at_ms.le(to));
    }
    if let Some(before) = request.before {
        query = query.filter(events::id.lt(before));
    }
    if let Some(after) = request.after {
        query = query.filter(events::id.gt(after));
    }
    if let Some(high_water) = request.high_water {
        query = query.filter(events::id.le(high_water));
    }
    query
}

/// Independent indexed streams for reconstructing one device with global journal facts.
struct TimelineRequests {
    /// Device equality query, using the partial device/ID index.
    device: ReadRequest,
    /// Closed journal-kind query, using the kind/ID index.
    journal: ReadRequest,
}

fn timeline_requests(request: &ReadRequest, device_id: DeviceId) -> TimelineRequests {
    let mut device = request.clone();
    device.timeline_device = None;
    device.filter.device_id = Some(device_id);
    let mut journal = request.clone();
    journal.timeline_device = None;
    journal.filter.device_id = None;
    journal.filter.kinds = Event::KINDS
        .iter()
        .filter(|kind| Event::category_for_kind(kind) == Some(Category::Journal))
        .map(|kind| (*kind).to_owned())
        .collect();
    TimelineRequests { device, journal }
}

fn paged(request: &ReadRequest, limit: u16) -> events::BoxedQuery<'static, diesel::sqlite::Sqlite> {
    let query = filtered(request);
    let query = match request.order {
        ReadOrder::NewestFirst => query.order(events::id.desc()),
        ReadOrder::OldestFirst => query.order(events::id.asc()),
    };
    query.limit(i64::from(limit))
}

fn summary_device_query(
    after: Option<String>,
    high_water: i64,
) -> events::BoxedQuery<'static, diesel::sqlite::Sqlite> {
    let mut query = events::table
        .filter(events::device_id.is_not_null())
        .filter(events::category.eq("device"))
        .filter(events::id.le(high_water))
        .into_boxed();
    if let Some(after) = after {
        query = query.filter(events::device_id.gt(after));
    }
    query
        .order(events::device_id.asc())
        .limit(i64::from(PAGE_CAP))
}

fn read_bounds(transaction: &mut Transaction<'_>) -> Result<RetainedBounds, PersistenceError> {
    let oldest = events::table
        .select((events::id, events::occurred_at_ms))
        .order(events::id)
        .first::<(i64, i64)>(transaction.connection())
        .optional()?;
    let high_water = events::table
        .select(diesel::dsl::max(events::id))
        .first::<Option<i64>>(transaction.connection())?;
    Ok(RetainedBounds {
        oldest_id: oldest.map(|row| row.0),
        oldest_at: oldest.map(|row| row.1),
        high_water,
    })
}

impl JournalDatabase {
    /// Pages distinct identity metadata via the device index, independently of event payloads.
    pub(in crate::journal) async fn summary_devices(
        &self,
        after: Option<String>,
        high_water: i64,
    ) -> Result<SummaryDevicePage, PersistenceError> {
        self.0
            .read(move |transaction| {
                let rows = summary_device_query(after, high_water)
                    .select(events::device_id)
                    .distinct()
                    .load::<Option<String>>(transaction.connection())?;
                let next_cursor = (rows.len() == usize::from(PAGE_CAP))
                    .then(|| rows.last().cloned().flatten())
                    .flatten();
                Ok(SummaryDevicePage {
                    devices: rows
                        .iter()
                        .filter_map(|id| id.as_deref().and_then(DeviceId::parse))
                        .collect(),
                    next_cursor,
                })
            })
            .await
            .map_err(TransactionError::into_error)
    }

    /// Reads retention endpoints without materializing any event rows.
    pub(in crate::journal) async fn bounds(&self) -> Result<RetainedBounds, PersistenceError> {
        self.0
            .read(read_bounds)
            .await
            .map_err(TransactionError::into_error)
    }

    /// Captures export bounds and its only full matching count in one transaction.
    pub(in crate::journal) async fn export_snapshot(
        &self,
        mut request: ReadRequest,
    ) -> Result<ExportSnapshot, PersistenceError> {
        self.0
            .read(move |transaction| {
                let bounds = read_bounds(transaction)?;
                request.high_water = bounds.high_water;
                let matching = filtered(&request)
                    .select(diesel::dsl::count_star())
                    .first::<i64>(transaction.connection())?;
                Ok(ExportSnapshot {
                    bounds,
                    matching: matching.unsigned_abs(),
                })
            })
            .await
            .map_err(TransactionError::into_error)
    }

    /// Reads bounded pages or merges independently indexed timeline streams by ID.
    pub(in crate::journal) async fn page(
        &self,
        request: ReadRequest,
    ) -> Result<ReadPage, PersistenceError> {
        if let Some(device_id) = request.timeline_device {
            let streams = timeline_requests(&request, device_id);
            let device = self.page_single(streams.device).await?;
            let journal = self.page_single(streams.journal).await?;
            let mut has_more = device.has_more || journal.has_more;
            let mut records = device.records;
            records.extend(journal.records);
            records.sort_unstable_by_key(JournalRecord::id);
            records.dedup_by_key(|record| record.id());
            if matches!(request.order, ReadOrder::NewestFirst) {
                records.reverse();
            }
            has_more |= records.len() > usize::from(request.limit.min(PAGE_CAP));
            records.truncate(usize::from(request.limit.min(PAGE_CAP)));
            return Ok(ReadPage { records, has_more });
        }
        self.page_single(request).await
    }

    async fn page_single(&self, request: ReadRequest) -> Result<ReadPage, PersistenceError> {
        let limit = request.limit.min(PAGE_CAP);
        let load_limit = limit.saturating_add(1).min(PAGE_CAP);
        let query_request = request.clone();
        let mut rows = self
            .0
            .read(move |transaction| {
                paged(&query_request, load_limit)
                    .select((
                        events::id,
                        (
                            events::occurred_at_ms,
                            events::boot_id,
                            events::kind,
                            events::category,
                            events::severity,
                            events::device_id,
                            events::session_id,
                            events::actor_operator_id,
                            events::request_id,
                            events::operation_id,
                            events::trace_id,
                            events::seat_code,
                            events::details,
                        ),
                    ))
                    .load::<(i64, StoredEvent)>(transaction.connection())
                    .map_err(PersistenceError::from)
            })
            .await
            .map_err(TransactionError::into_error)?;
        let mut has_more = rows.len() > usize::from(limit);
        rows.truncate(usize::from(limit));
        if limit == PAGE_CAP && rows.len() == usize::from(PAGE_CAP) {
            // Keep the 1,000-row transaction bound: a full-size page probes its extra ID in a separate read.
            if let Some((last_id, _)) = rows.last() {
                let mut lookahead = request;
                match lookahead.order {
                    ReadOrder::NewestFirst => lookahead.before = Some(*last_id),
                    ReadOrder::OldestFirst => lookahead.after = Some(*last_id),
                }
                has_more = self
                    .0
                    .read(move |transaction| {
                        paged(&lookahead, 1)
                            .select(events::id)
                            .first::<i64>(transaction.connection())
                            .optional()
                            .map(|next_id| next_id.is_some())
                            .map_err(PersistenceError::from)
                    })
                    .await
                    .map_err(TransactionError::into_error)?;
            }
        }
        Ok(ReadPage {
            records: rows
                .into_iter()
                .map(|(id, row)| JournalRecord::from_row(id, row))
                .collect(),
            has_more,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{component::device::DeviceId, journal::read::Filter, test_error::TestError};
    use diesel::{
        QueryableByName, RunQueryDsl,
        query_builder::{AstPass, Query, QueryFragment, QueryId},
        sql_types::{Text, Untyped},
        sqlite::{Sqlite, SqliteConnection},
    };
    use uuid::Uuid;

    /// Planner row used only by the `SQLite` EXPLAIN diagnostic boundary.
    #[derive(QueryableByName)]
    struct QueryPlan {
        /// Planner description used for index assertions.
        #[diesel(sql_type = Text)]
        detail: String,
    }

    /// Adds `SQLite`'s diagnostic prefix to the production query AST, retaining its real binds.
    #[derive(QueryId)]
    struct Explain<QueryType> {
        /// Actual filter/page query, rather than a handwritten SQL replica.
        query: QueryType,
    }

    impl<QueryType> Query for Explain<QueryType> {
        type SqlType = Untyped;
    }

    impl<QueryType: QueryFragment<Sqlite>> QueryFragment<Sqlite> for Explain<QueryType> {
        fn walk_ast<'query>(
            &'query self,
            mut output: AstPass<'_, 'query, Sqlite>,
        ) -> diesel::QueryResult<()> {
            // SQLite planner diagnostics have no typed Diesel equivalent; the underlying query stays typed.
            output.push_sql("EXPLAIN QUERY PLAN ");
            self.query.walk_ast(output.reborrow())
        }
    }

    impl<QueryType> RunQueryDsl<SqliteConnection> for Explain<QueryType> {}

    enum PlanShape {
        Filtered,
        Paged,
    }

    struct Case {
        /// Representative production filters and cursor values.
        request: ReadRequest,
        /// Index required to serve this request shape.
        index: &'static str,
    }

    async fn assert_index(
        database: &JournalDatabase,
        case: Case,
        shape: PlanShape,
    ) -> Result<(), TestError> {
        let query = match shape {
            PlanShape::Filtered => filtered(&case.request),
            PlanShape::Paged => paged(&case.request, case.request.limit.clamp(1, PAGE_CAP)),
        };
        let sql = diesel::debug_query::<Sqlite, _>(&query).to_string();
        let details = database
            .0
            .read(move |transaction| {
                Explain { query }
                    .load::<QueryPlan>(transaction.connection())
                    .map_err(PersistenceError::from)
            })
            .await
            .map_err(TransactionError::into_error)?;
        assert!(
            details.iter().any(|row| row.detail.contains(case.index)),
            "index {} missing: {sql}",
            case.index
        );
        Ok(())
    }

    #[tokio::test]
    async fn read_filters_use_the_existing_journal_indexes() -> Result<(), TestError> {
        let directory = tempfile::tempdir()?;
        let database = JournalDatabase::open(&directory.path().join("journal.db")).await?;
        let device_id = DeviceId::parse(&Uuid::now_v7().to_string())
            .ok_or(TestError::missing("device fixture"))?;
        for case in [
            Case {
                request: ReadRequest {
                    filter: Filter {
                        device_id: Some(device_id),
                        ..Filter::default()
                    },
                    ..ReadRequest::default()
                },
                index: "journal_events_device_id",
            },
            Case {
                request: ReadRequest {
                    filter: Filter {
                        kinds: vec!["lease_attached".into()],
                        ..Filter::default()
                    },
                    ..ReadRequest::default()
                },
                index: "journal_events_kind_id",
            },
            Case {
                request: ReadRequest {
                    filter: Filter {
                        actor_operator_id: Some(Uuid::now_v7()),
                        ..Filter::default()
                    },
                    ..ReadRequest::default()
                },
                index: "journal_events_actor_id",
            },
            Case {
                request: ReadRequest {
                    filter: Filter {
                        operation_id: Some(Uuid::now_v7()),
                        ..Filter::default()
                    },
                    ..ReadRequest::default()
                },
                index: "journal_events_operation_id",
            },
            Case {
                request: ReadRequest {
                    filter: Filter {
                        request_id: Some(Uuid::now_v7()),
                        ..Filter::default()
                    },
                    ..ReadRequest::default()
                },
                index: "journal_events_request_id",
            },
            Case {
                request: ReadRequest {
                    filter: Filter {
                        from: Some(0),
                        to: Some(100),
                        ..Filter::default()
                    },
                    ..ReadRequest::default()
                },
                index: "journal_events_occurred_at",
            },
        ] {
            assert_index(&database, case, PlanShape::Filtered).await?;
        }
        Ok(())
    }

    #[tokio::test]
    async fn timeline_pages_use_device_and_kind_indexes_with_real_cursor_binds()
    -> Result<(), TestError> {
        let directory = tempfile::tempdir()?;
        let database = JournalDatabase::open(&directory.path().join("journal.db")).await?;
        let device_id = DeviceId::parse(&Uuid::now_v7().to_string())
            .ok_or(TestError::missing("device fixture"))?;
        for order in [ReadOrder::NewestFirst, ReadOrder::OldestFirst] {
            let request = ReadRequest {
                after: Some(10),
                before: Some(200),
                high_water: Some(300),
                limit: PAGE_CAP,
                order,
                ..ReadRequest::default()
            };
            let streams = timeline_requests(&request, device_id);
            assert_index(
                &database,
                Case {
                    request: streams.device,
                    index: "journal_events_device_id",
                },
                PlanShape::Paged,
            )
            .await?;
            assert_index(
                &database,
                Case {
                    request: streams.journal,
                    index: "journal_events_kind_id",
                },
                PlanShape::Paged,
            )
            .await?;
        }
        Ok(())
    }

    #[tokio::test]
    async fn summary_queries_use_kind_and_device_indexes_with_actual_bounds()
    -> Result<(), TestError> {
        let directory = tempfile::tempdir()?;
        let database = JournalDatabase::open(&directory.path().join("journal.db")).await?;
        let device_id =
            DeviceId::parse(&Uuid::now_v7().to_string()).ok_or(TestError::missing("device"))?;
        assert_index(
            &database,
            Case {
                request: ReadRequest {
                    filter: Filter {
                        kinds: vec![
                            "disconnected".into(),
                            "lease_replaced".into(),
                            "actor_stopped".into(),
                        ],
                        from: Some(10),
                        to: Some(20),
                        ..Filter::default()
                    },
                    high_water: Some(50),
                    limit: PAGE_CAP,
                    ..ReadRequest::default()
                },
                index: "journal_events_kind_id",
            },
            PlanShape::Paged,
        )
        .await?;
        for kinds in [
            vec!["lease_attached".into(), "lease_replaced".into()],
            vec![
                "disconnected".into(),
                "lease_replaced".into(),
                "actor_stopped".into(),
            ],
        ] {
            assert_index(
                &database,
                Case {
                    request: ReadRequest {
                        filter: Filter {
                            device_id: Some(device_id),
                            kinds,
                            ..Filter::default()
                        },
                        high_water: Some(50),
                        limit: 1,
                        ..ReadRequest::default()
                    },
                    index: "journal_events_device_id",
                },
                PlanShape::Paged,
            )
            .await?;
        }
        let query = summary_device_query(Some(device_id.as_text()), 50)
            .select(events::device_id)
            .distinct();
        let details = database
            .0
            .read(move |transaction| {
                Explain { query }
                    .load::<QueryPlan>(transaction.connection())
                    .map_err(PersistenceError::from)
            })
            .await
            .map_err(TransactionError::into_error)?;
        assert!(
            details
                .iter()
                .any(|row| row.detail.contains("journal_events_device_id"))
        );
        Ok(())
    }

    #[tokio::test]
    async fn bounds_do_not_read_event_payload_columns() -> Result<(), TestError> {
        use diesel::connection::SimpleConnection as _;
        let directory = tempfile::tempdir()?;
        let database = JournalDatabase::open(&directory.path().join("journal.db")).await?;
        database
            .0
            .write(|transaction| {
                diesel::insert_into(events::table)
                    .values((
                        events::occurred_at_ms.eq(100),
                        events::boot_id.eq(Uuid::now_v7().to_string()),
                        events::kind.eq("server_started"),
                        events::category.eq("journal"),
                        events::severity.eq("info"),
                        events::details.eq("{}"),
                    ))
                    .execute(transaction.connection())?;
                transaction
                    .connection()
                    .batch_execute("ALTER TABLE journal_events DROP COLUMN details")
                    .map_err(PersistenceError::from)
            })
            .await
            .map_err(TransactionError::into_error)?;
        let bounds = database.bounds().await?;
        assert_eq!(bounds.oldest_id, Some(1));
        assert_eq!(bounds.oldest_at, Some(100));
        assert_eq!(bounds.high_water, Some(1));
        Ok(())
    }
}
