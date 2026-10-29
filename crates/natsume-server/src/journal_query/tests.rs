use super::*;
use crate::{
    component::{
        device::DeviceId,
        operator::{OperatorRole, tests::PasswordVerificationTestGuard},
    },
    config::JournalConfig,
    db::{Database, DatabaseConfig},
    journal::{
        JournalComponent, JournalContext,
        event::{payload::*, types::*},
        tests::{SeedEvent, corrupt_latest, prune_through, ready, seed_events},
    },
    test_error::TestError,
};
use uuid::Uuid;

pub(crate) struct Fixture {
    /// Owns the isolated business and journal files for the full scenario.
    pub(crate) directory: tempfile::TempDir,
    /// Shared production composition with a real journal writer and read side.
    pub(crate) state: Arc<crate::server_state::ServerState>,
    /// Known admin session for endpoint permission checks.
    pub(crate) admin_cookie: String,
    /// Known viewer session for endpoint permission checks.
    pub(crate) viewer_cookie: String,
    /// Actor identity used for display-name and correlation tests.
    pub(crate) admin_id: Uuid,
    /// Device identity used by fixture journal facts.
    pub(crate) device_id: DeviceId,
}
impl Fixture {
    pub(crate) async fn new() -> Result<Self, TestError> {
        let directory = tempfile::tempdir()?;
        let database = Database::connect_and_migrate(&DatabaseConfig::new(
            directory.path().join("business.db"),
            true,
        ))
        .await?;
        let admin_id = crate::http::tests::seed_operator(
            &database,
            "journal-admin",
            "journal-password-secret-canary1!",
        )
        .await?;
        let viewer_id = crate::http::tests::seed_operator(
            &database,
            "journal-viewer",
            "journal-password-secret-canary1!",
        )
        .await?;
        let journal = JournalComponent::start(
            directory.path().join("journal.db"),
            JournalConfig {
                min_free_bytes: 1,
                ..JournalConfig::default()
            },
        );
        ready(&journal).await?;
        let state = Arc::new(crate::server_state::tests::for_test_with_journal(
            database,
            directory.path().join("documents"),
            directory.path().join("logos"),
            journal,
        )?);
        state
            .operator()
            .change_role(viewer_id, OperatorRole::Viewer)
            .await?;
        let admin = state
            .operator()
            .sign_in(
                "journal-admin",
                "journal-password-secret-canary1!".into(),
                JournalContext::default(),
            )
            .await?;
        let viewer = state
            .operator()
            .sign_in(
                "journal-viewer",
                "journal-password-secret-canary1!".into(),
                JournalContext::default(),
            )
            .await?;
        ready(state.journal()).await?;
        Ok(Self {
            directory,
            state,
            admin_cookie: format!(
                "__Secure-natsume_session={}",
                admin.wire_credential().expose()
            ),
            viewer_cookie: format!(
                "__Secure-natsume_session={}",
                viewer.wire_credential().expose()
            ),
            admin_id,
            device_id: DeviceId::parse(&Uuid::now_v7().to_string())
                .ok_or(TestError::missing("device id"))?,
        })
    }
    pub(crate) fn row(&self, at: i64, event: Event) -> SeedEvent {
        SeedEvent {
            boot_id: self.state.journal().boot_id(),
            context: if event.category() == Category::Device {
                JournalContext::default().with_device(self.device_id)
            } else {
                JournalContext::default()
            },
            event,
            at,
        }
    }
    pub(crate) async fn seed(&self, rows: Vec<SeedEvent>) -> Result<(), TestError> {
        seed_events(self.state.journal(), rows).await
    }
    fn filter(&self) -> Filter {
        Filter {
            device_id: Some(self.device_id),
            ..Filter::default()
        }
    }
    fn session() -> SessionId {
        SessionId::from_lease(Uuid::now_v7())
    }
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "keeps bounded cursor continuation and conjunctive filters in one persisted sequence"
)]
async fn cursors_filters_page_caps_names_and_restricted_queries() -> Result<(), TestError> {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = Fixture::new().await?;
    let operation = Uuid::now_v7();
    let request_id = Uuid::now_v7();
    let mut rows = Vec::new();
    for at in 0..1102 {
        let mut row = f.row(
            at,
            Event::LeaseAttached(LeaseAttached {
                session_id: Fixture::session(),
            }),
        );
        row.context = row
            .context
            .with_actor(OperatorId::from_authenticated_operator(f.admin_id))
            .with_operation(OperationId::from_operation(operation))
            .with_request(RequestId::from_request(request_id))
            .with_seat(SeatCode::from_committed_binding("A_%1")?);
        rows.push(row);
    }
    f.seed(rows).await?;
    let q = f.state.journal_query();
    let page = q
        .events(
            ReadRequest {
                filter: f.filter(),
                limit: u16::MAX,
                ..ReadRequest::default()
            },
            Visibility::All,
        )
        .await?;
    assert_eq!(page.events.len(), 1000);
    assert!(page.has_more);
    assert_eq!(
        page.operators[0].display_name.as_deref(),
        Some("journal-admin")
    );
    let cursor = page.next_before;
    let older = q
        .events(
            ReadRequest {
                filter: f.filter(),
                before: cursor,
                limit: 1000,
                ..ReadRequest::default()
            },
            Visibility::All,
        )
        .await?;
    assert_eq!(older.events.len(), 102);
    assert!(!older.has_more);
    let after = older
        .events
        .last()
        .ok_or(TestError::missing("oldest"))?
        .id();
    let increment = q
        .events(
            ReadRequest {
                filter: f.filter(),
                after: Some(after),
                limit: DEFAULT_PAGE,
                ..ReadRequest::default()
            },
            Visibility::DeviceOnly,
        )
        .await?;
    assert_eq!(increment.events.len(), 500);
    assert!(increment.has_more);
    let continuation = q
        .events(
            ReadRequest {
                filter: f.filter(),
                after: Some(after),
                before: increment.next_before,
                limit: 1000,
                ..ReadRequest::default()
            },
            Visibility::DeviceOnly,
        )
        .await?;
    assert_eq!(continuation.events.len(), 601);
    let filtered = q
        .events(
            ReadRequest {
                filter: Filter {
                    from: Some(10),
                    to: Some(20),
                    actor_operator_id: Some(f.admin_id),
                    operation_id: Some(operation),
                    request_id: Some(request_id),
                    seat_prefix: Some("A_%".into()),
                    severities: vec![Severity::Info],
                    kinds: vec!["lease_attached".into()],
                    ..f.filter()
                },
                limit: 1000,
                ..ReadRequest::default()
            },
            Visibility::All,
        )
        .await?;
    assert_eq!(filtered.events.len(), 11);
    for filter in [
        Filter {
            categories: vec![Category::Auth],
            ..Filter::default()
        },
        Filter {
            kinds: vec!["operator_request".into()],
            ..Filter::default()
        },
    ] {
        assert!(matches!(
            q.events(
                ReadRequest {
                    filter,
                    limit: 10,
                    ..ReadRequest::default()
                },
                Visibility::DeviceOnly
            )
            .await,
            Err(QueryError::Forbidden)
        ));
    }
    f.state.journal().shutdown().await;
    Ok(())
}

#[tokio::test]
async fn export_fixes_high_water_and_describes_rows_pruned_between_pages() -> Result<(), TestError>
{
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = Fixture::new().await?;
    f.seed(
        (0..1500)
            .map(|at| {
                f.row(
                    at,
                    Event::LeaseAttached(LeaseAttached {
                        session_id: Fixture::session(),
                    }),
                )
            })
            .collect(),
    )
    .await?;
    let q = f.state.journal_query();
    let high = q
        .events(
            ReadRequest {
                filter: f.filter(),
                limit: 1,
                ..ReadRequest::default()
            },
            Visibility::All,
        )
        .await?
        .events[0]
        .id();
    let mut export = q
        .export(ReadRequest {
            filter: f.filter(),
            ..ReadRequest::default()
        })
        .await?;
    f.seed(vec![f.row(
        2000,
        Event::LeaseAttached(LeaseAttached {
            session_id: Fixture::session(),
        }),
    )])
    .await?;
    prune_through(f.state.journal(), high - 1000).await?;
    let mut rows = 0;
    let mut metadata = None;
    while let Some(line) = export.next_line().await? {
        let line: serde_json::Value = serde_json::from_slice(&line)?;
        if line["type"] == "event" {
            rows += 1;
            assert!(
                line["event"]["header"]["id"]
                    .as_i64()
                    .is_some_and(|id| id <= high)
            );
        } else {
            metadata = Some(line);
        }
    }
    assert_eq!(rows, 1000);
    let metadata = metadata.ok_or(TestError::missing("export trailer"))?;
    assert_eq!(metadata["pruned_rows"], 500);
    assert_eq!(metadata["high_water_id"], high);
    f.state.journal().shutdown().await;
    Ok(())
}

#[tokio::test]
async fn unreadable_details_and_unknown_kind_never_return_raw_canaries() -> Result<(), TestError> {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = Fixture::new().await?;
    f.seed(vec![f.row(
        1,
        Event::LeaseAttached(LeaseAttached {
            session_id: Fixture::session(),
        }),
    )])
    .await?;
    corrupt_latest(f.state.journal(), "password_token_login_secret_canary").await?;
    let page = f
        .state
        .journal_query()
        .events(
            ReadRequest {
                filter: f.filter(),
                limit: 10,
                ..ReadRequest::default()
            },
            Visibility::All,
        )
        .await?;
    let json = serde_json::to_string(&page)?;
    assert!(!json.contains("password_token_login_secret_canary"));
    assert!(matches!(&page.events[0], JournalRecord::Unreadable { kind, .. } if kind == "unknown"));
    f.state.journal().shutdown().await;
    Ok(())
}

#[tokio::test]
async fn timeline_looks_back_without_reviving_pre_restart_sessions_and_handles_clock_steps()
-> Result<(), TestError> {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = Fixture::new().await?;
    let id = Fixture::session();
    f.seed(vec![
        f.row(10, Event::LeaseAttached(LeaseAttached { session_id: id })),
        f.row(
            20,
            Event::ServerStarted(ServerStarted {
                boot_id: Uuid::now_v7(),
            }),
        ),
    ])
    .await?;
    let result = f
        .state
        .journal_query()
        .timeline(f.device_id, 30, 40)
        .await?;
    assert!(result.sessions.is_empty());
    let next = Fixture::session();
    f.seed(vec![
        f.row(
            100,
            Event::LeaseAttached(LeaseAttached { session_id: next }),
        ),
        f.row(
            50,
            Event::Disconnected(Disconnected {
                session_id: next,
                reason: DisconnectReason::ClientClose,
            }),
        ),
    ])
    .await?;
    let result = f
        .state
        .journal_query()
        .timeline(f.device_id, 90, 150)
        .await?;
    assert_eq!(result.sessions.len(), 1);
    assert!(result.sessions[0].imprecise);
    assert_eq!(result.sessions[0].duration_ms, Some(0));
    f.state.journal().shutdown().await;
    Ok(())
}

#[tokio::test]
async fn summaries_count_terminals_inside_the_window_and_names_fail_without_losing_ids()
-> Result<(), TestError> {
    use diesel::Connection as _;
    use diesel::connection::SimpleConnection as _;
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = Fixture::new().await?;
    let now = i64::try_from(time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000)
        .unwrap_or(i64::MAX);
    let first = Fixture::session();
    let second = Fixture::session();
    let mut row = f.row(
        now - 100,
        Event::LeaseAttached(LeaseAttached { session_id: second }),
    );
    row.context = row
        .context
        .with_actor(OperatorId::from_authenticated_operator(f.admin_id));
    f.seed(vec![
        f.row(
            now - 10_000,
            Event::LeaseAttached(LeaseAttached { session_id: first }),
        ),
        f.row(
            now - 9000,
            Event::Disconnected(Disconnected {
                session_id: first,
                reason: DisconnectReason::ClientClose,
            }),
        ),
        row,
        f.row(
            now - 10,
            Event::Disconnected(Disconnected {
                session_id: second,
                reason: DisconnectReason::Disabled,
            }),
        ),
    ])
    .await?;
    let summaries = f.state.journal_query().summaries(1000).await?;
    assert_eq!(summaries.devices.len(), 1);
    assert_eq!(summaries.devices[0].disconnects, 1);
    assert_eq!(
        summaries.devices[0].last_disconnect_reason,
        Some(DisconnectReason::Disabled)
    );
    let mut connection = diesel::sqlite::SqliteConnection::establish(
        f.directory
            .path()
            .join("business.db")
            .to_str()
            .ok_or(TestError::missing("database path"))?,
    )?;
    connection.batch_execute("PRAGMA foreign_keys = OFF; DROP TABLE operator_accounts")?;
    let page = f
        .state
        .journal_query()
        .events(
            ReadRequest {
                filter: f.filter(),
                limit: 10,
                ..ReadRequest::default()
            },
            Visibility::All,
        )
        .await?;
    assert_eq!(page.operators[0].operator_id, f.admin_id);
    assert!(page.operators[0].display_name.is_none());
    f.state.journal().shutdown().await;
    Ok(())
}

#[tokio::test]
async fn timeline_restores_binding_and_pause_snapshots_before_the_window_anchor()
-> Result<(), TestError> {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = Fixture::new().await?;
    let id = Fixture::session();
    f.seed(vec![
        f.row(
            1,
            Event::BindingBound(BindingBound {
                binding_id: Uuid::now_v7(),
                seat_code: SeatCode::from_committed_binding("A7")?,
            }),
        ),
        f.row(
            5,
            Event::JournalPaused(JournalPaused {
                reason: PauseReason::DiskBudget,
            }),
        ),
        f.row(10, Event::LeaseAttached(LeaseAttached { session_id: id })),
        f.row(20, Event::JournalResumed(JournalResumed {})),
    ])
    .await?;
    let result = f
        .state
        .journal_query()
        .timeline(f.device_id, 15, 30)
        .await?;
    assert_eq!(
        result.sessions[0].seat_code.as_ref().map(SeatCode::as_str),
        Some("A7")
    );
    assert!(result.sessions[0].truncated_start);
    assert!(
        result
            .gaps
            .iter()
            .any(|g| matches!(g.kind, super::timeline::GapKind::Paused)
                && g.from == 15
                && g.to == Some(20))
    );
    f.state.journal().shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn injected_startup_offset_is_used_inside_the_multithreaded_runtime() -> Result<(), TestError>
{
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let fixture = Fixture::new().await?;
    let existing = fixture.state.journal_query();
    let offset = time::UtcOffset::from_hms(5, 45, 0)
        .unwrap_or_else(|error| panic!("fixture offset: {error}"));
    let query = JournalQuery::new(
        existing.reader.clone(),
        Arc::clone(&existing.operator),
        existing.live.clone(),
        offset,
    );
    let page = query
        .events(
            ReadRequest {
                limit: 10,
                ..ReadRequest::default()
            },
            Visibility::All,
        )
        .await?;
    assert_eq!(page.journal.server_utc_offset_minutes, 345);
    assert_eq!(query.status().await?.journal.server_utc_offset_minutes, 345);
    fixture.state.journal().shutdown().await;
    Ok(())
}

#[tokio::test]
async fn maximum_pages_distinguish_an_exact_boundary_from_one_more_row() -> Result<(), TestError> {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let fixture = Fixture::new().await?;
    fixture
        .seed(
            (0..1001)
                .map(|at| {
                    fixture.row(
                        at,
                        Event::LeaseAttached(LeaseAttached {
                            session_id: Fixture::session(),
                        }),
                    )
                })
                .collect(),
        )
        .await?;
    let query = fixture.state.journal_query();
    let exact = query
        .events(
            ReadRequest {
                filter: Filter {
                    to: Some(999),
                    ..fixture.filter()
                },
                limit: 1000,
                ..ReadRequest::default()
            },
            Visibility::All,
        )
        .await?;
    assert_eq!(exact.events.len(), 1000);
    assert!(!exact.has_more);
    let more = query
        .events(
            ReadRequest {
                filter: fixture.filter(),
                limit: 1000,
                ..ReadRequest::default()
            },
            Visibility::All,
        )
        .await?;
    assert_eq!(more.events.len(), 1000);
    assert!(more.has_more);
    let last = query
        .events(
            ReadRequest {
                filter: fixture.filter(),
                before: more.next_before,
                limit: 1000,
                ..ReadRequest::default()
            },
            Visibility::All,
        )
        .await?;
    assert_eq!(last.events.len(), 1);
    assert!(!last.has_more);
    fixture.state.journal().shutdown().await;
    Ok(())
}

#[tokio::test]
async fn indexed_timeline_streams_merge_both_orders_without_duplicates_or_new_rows()
-> Result<(), TestError> {
    use crate::journal::read::ReadOrder;
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let fixture = Fixture::new().await?;
    let mut rows = Vec::new();
    for at in 0..1200 {
        rows.push(fixture.row(
            at,
            if at % 2 == 0 {
                Event::LeaseAttached(LeaseAttached {
                    session_id: Fixture::session(),
                })
            } else {
                Event::JournalResumed(JournalResumed {})
            },
        ));
    }
    let mut overlapping = fixture.row(1200, Event::JournalResumed(JournalResumed {}));
    overlapping.context = JournalContext::default().with_device(fixture.device_id);
    rows.push(overlapping);
    fixture.seed(rows).await?;
    let reader = fixture.state.journal().reader();
    let high_water = reader.bounds().await.map_err(QueryError::from)?.high_water;
    fixture
        .seed(vec![
            fixture.row(1201, Event::JournalResumed(JournalResumed {})),
        ])
        .await?;
    for order in [ReadOrder::OldestFirst, ReadOrder::NewestFirst] {
        let mut request = ReadRequest {
            timeline_device: Some(fixture.device_id),
            high_water,
            limit: 137,
            order,
            ..ReadRequest::default()
        };
        let mut ids = Vec::new();
        loop {
            let page = reader
                .page(request.clone())
                .await
                .map_err(QueryError::from)?;
            let last = page.records.last().map(JournalRecord::id);
            ids.extend(page.records.iter().map(JournalRecord::id));
            if !page.has_more {
                break;
            }
            match order {
                ReadOrder::OldestFirst => request.after = last,
                ReadOrder::NewestFirst => request.before = last,
            }
        }
        assert_eq!(ids.len(), 1202);
        assert!(ids.windows(2).all(|pair| match order {
            ReadOrder::OldestFirst => pair[0] < pair[1],
            ReadOrder::NewestFirst => pair[0] > pair[1],
        }));
        assert!(ids.iter().all(|id| Some(*id) <= high_water));
    }
    fixture.state.journal().shutdown().await;
    Ok(())
}

#[tokio::test]
async fn timeline_regression_keeps_a_session_covering_the_window_before_a_clock_step()
-> Result<(), TestError> {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let fixture = Fixture::new().await?;
    let first = Fixture::session();
    let second = Fixture::session();
    fixture
        .seed(vec![
            fixture.row(
                10,
                Event::LeaseAttached(LeaseAttached { session_id: first }),
            ),
            fixture.row(
                50,
                Event::Disconnected(Disconnected {
                    session_id: first,
                    reason: DisconnectReason::ClientClose,
                }),
            ),
            fixture.row(
                20,
                Event::LeaseAttached(LeaseAttached { session_id: second }),
            ),
            fixture.row(
                30,
                Event::Disconnected(Disconnected {
                    session_id: second,
                    reason: DisconnectReason::ClientClose,
                }),
            ),
        ])
        .await?;
    let timeline = fixture
        .state
        .journal_query()
        .timeline(fixture.device_id, 35, 45)
        .await?;
    assert_eq!(timeline.sessions.len(), 1);
    assert_eq!(timeline.sessions[0].session_id, first.value());
    assert_eq!(timeline.sessions[0].duration_ms, Some(10));
    fixture.state.journal().shutdown().await;
    Ok(())
}

#[tokio::test]
async fn summaries_bound_payload_reads_to_connection_kinds_and_the_count_window()
-> Result<(), TestError> {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let fixture = Fixture::new().await?;
    let now = super::now_ms();
    let session_id = Fixture::session();
    let mut rows = vec![
        fixture.row(
            now - 10000,
            Event::LeaseAttached(LeaseAttached { session_id }),
        ),
        fixture.row(
            now - 9000,
            Event::Disconnected(Disconnected {
                session_id,
                reason: DisconnectReason::ClientClose,
            }),
        ),
    ];
    rows.extend((0..1200).map(|_| {
        fixture.row(
            now - 10,
            Event::LifecycleChanged(LifecycleChanged {
                to: LifecycleState::Enabled,
            }),
        )
    }));
    fixture.seed(rows).await?;
    let queries = crate::journal::tests::capture_queries(fixture.state.journal())?;
    let summaries = fixture.state.journal_query().summaries(1000).await?;
    assert_eq!(summaries.devices.len(), 1);
    assert_eq!(summaries.devices[0].last_connected_at, Some(now - 10000));
    assert_eq!(summaries.devices[0].last_disconnected_at, Some(now - 9000));
    assert_eq!(summaries.devices[0].disconnects, 0);
    {
        let queries = queries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let payload_reads = queries
            .iter()
            .filter(|query| query.contains("SELECT") && query.contains("`details`"))
            .collect::<Vec<_>>();
        assert!(!payload_reads.is_empty());
        assert!(
            payload_reads
                .iter()
                .all(|query| query.contains("`kind` IN")),
            "payload reads must constrain connection kinds: {payload_reads:?}"
        );
        assert!(
            payload_reads
                .iter()
                .any(|query| query.contains("`occurred_at_ms` >=")
                    && query.contains("`occurred_at_ms` <=")),
            "window counts must be time-bounded"
        );
    }
    fixture.state.journal().shutdown().await;
    Ok(())
}
