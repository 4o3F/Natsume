use diesel::{ExpressionMethods, QueryDsl, RunQueryDsl, connection::SimpleConnection};
use tempfile::TempDir;

use super::*;
use crate::{
    db::DatabaseConfig,
    diesel_schema::{
        device_home_targets, device_session_targets, devices, target_submission_receipts,
    },
};

struct Fixture {
    database: Database,
    component: TargetSubmissionComponent,
    state: crate::server_state::ServerState,
    ids: Vec<DeviceId>,
    owner: Uuid,
    _root: TempDir,
}

impl Fixture {
    async fn new(count: usize) -> Self {
        let root = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let database = Database::connect_and_migrate(&DatabaseConfig::new(
            root.path().join("server.db"),
            true,
        ))
        .await
        .unwrap_or_else(|error| panic!("database: {error}"));
        let ids: Vec<_> = (0..count)
            .map(|_| DeviceId::parse(&Uuid::now_v7().to_string()).unwrap_or_else(|| panic!("ID")))
            .collect();
        let rows = ids
            .iter()
            .map(|id| {
                let id = id.as_text();
                let machine = Uuid::new_v5(&Uuid::NAMESPACE_OID, id.as_bytes()).to_string();
                (
                    devices::device_id.eq(id),
                    devices::machine_hardware_id.eq(machine),
                    devices::evidence_quality.eq("strong"),
                    devices::state.eq("enabled"),
                    devices::created_at_unix_ms.eq(1_i64),
                )
            })
            .collect::<Vec<_>>();
        database
            .write(move |tx| {
                diesel::insert_into(devices::table)
                    .values(rows)
                    .execute(tx.connection())
                    .map_err(PersistenceError::from)?;
                Ok::<_, PersistenceError>(())
            })
            .await
            .unwrap_or_else(|error| panic!("fixtures: {error:?}"));
        let state =
            crate::server_state::tests::for_test(database.clone(), root.path().join("documents"))
                .unwrap_or_else(|error| panic!("state: {error}"));
        Self {
            state,
            component: TargetSubmissionComponent::new(database.clone()),
            database,
            ids,
            owner: Uuid::now_v7(),
            _root: root,
        }
    }

    async fn submit(
        &self,
        owner: Uuid,
        request: TargetSubmissionRequest,
    ) -> Result<TargetSubmissionResult, TargetSubmissionError> {
        self.submit_with(&self.component, owner, request).await
    }

    async fn submit_with(
        &self,
        component: &TargetSubmissionComponent,
        owner: Uuid,
        request: TargetSubmissionRequest,
    ) -> Result<TargetSubmissionResult, TargetSubmissionError> {
        let (result, change) = component.submit(owner, request).await?;
        self.state.device_control().publish(change).await;
        Ok(result)
    }

    fn request(action: TargetAction) -> TargetSubmissionRequest {
        TargetSubmissionRequest {
            operation_id: Uuid::now_v7(),
            scope: TargetScope::AllEnabled,
            action,
        }
    }

    async fn sql(&self, sql: String) {
        self.database
            .write(move |tx| {
                tx.connection()
                    .batch_execute(&sql)
                    .map_err(PersistenceError::from)
            })
            .await
            .unwrap_or_else(|error| panic!("SQL fixture: {error:?}"));
    }

    async fn home_epochs(&self) -> Vec<Option<i64>> {
        self.database
            .read(|tx| {
                device_home_targets::table
                    .order(device_home_targets::device_id)
                    .select(device_home_targets::reset_epoch)
                    .load(tx.connection())
                    .map_err(PersistenceError::from)
            })
            .await
            .unwrap_or_else(|error| panic!("epochs: {error:?}"))
    }

    async fn receipts(&self) -> i64 {
        self.database
            .read(|tx| {
                target_submission_receipts::table
                    .count()
                    .get_result(tx.connection())
                    .map_err(PersistenceError::from)
            })
            .await
            .unwrap_or_else(|error| panic!("receipts: {error:?}"))
    }
}

#[tokio::test]
async fn replay_preserves_scope_results_and_epochs_after_restart_and_lifecycle_changes() {
    let fixture = Fixture::new(3).await;
    fixture
        .sql(format!(
            "UPDATE devices SET state='disabled' WHERE device_id='{}'",
            fixture.ids[2].as_text()
        ))
        .await;
    let request = Fixture::request(TargetAction::ResetHome);
    let result = fixture
        .submit(fixture.owner, request.clone())
        .await
        .unwrap_or_else(|error| panic!("submit: {error}"));
    assert_eq!(result.results.len(), 2);
    assert!(result.results.iter().all(|row| row.rejection.is_none()));
    fixture
        .sql(
            "UPDATE devices SET state=CASE WHEN state='disabled' THEN 'enabled' ELSE 'revoked' END"
                .into(),
        )
        .await;
    let rebuilt = TargetSubmissionComponent::new(fixture.database.clone());
    assert!(
        matches!(fixture.submit_with(&rebuilt, fixture.owner, request.clone()).await, Ok(actual) if actual == result)
    );
    assert_eq!(fixture.home_epochs().await, [Some(1), Some(1)]);
    assert_eq!(fixture.receipts().await, 1);
    let mut changed = request.clone();
    changed.action = TargetAction::TerminateSession;
    assert!(matches!(
        fixture.submit_with(&rebuilt, fixture.owner, changed).await,
        Err(TargetSubmissionError::OperationIdConflict)
    ));
    assert!(matches!(
        fixture.submit_with(&rebuilt, Uuid::now_v7(), request).await,
        Err(TargetSubmissionError::OperationIdConflict)
    ));
}

#[tokio::test]
async fn domain_rejections_do_not_change_failed_targets_or_block_other_devices() {
    let fixture = Fixture::new(5).await;
    fixture.sql(format!("UPDATE devices SET state='disabled' WHERE device_id='{}'; UPDATE devices SET state='revoked' WHERE device_id='{}'; INSERT INTO device_home_targets VALUES ('{}', {}), ('{}', 0);",
        fixture.ids[0].as_text(), fixture.ids[1].as_text(), fixture.ids[2].as_text(), i64::MAX, fixture.ids[3].as_text())).await;
    let mut request = Fixture::request(TargetAction::ResetHome);
    request.scope = TargetScope::Devices(fixture.ids.clone());
    let result = fixture
        .submit(fixture.owner, request.clone())
        .await
        .unwrap_or_else(|error| panic!("submit: {error}"));
    let rejected: Vec<_> = result.results.iter().map(|row| row.rejection).collect();
    assert_eq!(
        rejected,
        [
            Some(TargetRejection::DeviceNotEnabled),
            Some(TargetRejection::DeviceNotEnabled),
            Some(TargetRejection::EpochExhausted),
            Some(TargetRejection::InvalidTarget),
            None
        ]
    );
    assert_eq!(
        fixture.home_epochs().await,
        [Some(i64::MAX), Some(0), Some(1)]
    );
    assert!(matches!(fixture.submit(fixture.owner, request).await, Ok(actual) if actual == result));
}

#[tokio::test]
async fn concurrent_same_id_advances_each_epoch_once_and_id_sets_are_canonical() {
    let fixture = Fixture::new(4).await;
    let mut request = Fixture::request(TargetAction::ResetHome);
    request.scope = TargetScope::Devices(fixture.ids.clone());
    let mut replay = request.clone();
    let mut ids = fixture.ids.clone();
    ids.reverse();
    ids.push(fixture.ids[0]);
    replay.scope = TargetScope::Devices(ids);
    let (a, b) = tokio::join!(
        fixture.submit(fixture.owner, request),
        fixture.submit(fixture.owner, replay)
    );
    assert_eq!(
        a.as_ref()
            .unwrap_or_else(|error| panic!("{}", snafu::Report::from_error(error))),
        b.as_ref()
            .unwrap_or_else(|error| panic!("{}", snafu::Report::from_error(error)))
    );
    assert!(a.is_ok());
    assert_eq!(fixture.home_epochs().await, vec![Some(1); 4]);
    assert_eq!(fixture.receipts().await, 1);
}

#[tokio::test]
async fn receipt_or_target_write_failure_rolls_back_all_targets_and_the_id() {
    for trigger in [
        "CREATE TRIGGER fail_write BEFORE INSERT ON target_submission_receipts BEGIN SELECT RAISE(ABORT,'test receipt failure'); END;".to_owned(),
        "CREATE TRIGGER fail_write BEFORE INSERT ON device_home_targets WHEN (SELECT count(*) FROM device_home_targets) = 1 BEGIN SELECT RAISE(ABORT,'test target failure'); END;".to_owned(),
        "CREATE TRIGGER fail_write BEFORE INSERT ON device_home_targets BEGIN SELECT RAISE(IGNORE); END;".to_owned(),
    ] {
        let fixture = Fixture::new(3).await;
        fixture.sql(trigger).await;
        let request = Fixture::request(TargetAction::ResetHome);
        assert!(matches!(fixture.submit(fixture.owner, request.clone()).await, Err(TargetSubmissionError::PersistenceFailed { .. })));
        assert!(fixture.home_epochs().await.is_empty());
        assert_eq!(fixture.receipts().await, 0);
        fixture.sql("DROP TRIGGER fail_write".into()).await;
        assert!(fixture.submit(fixture.owner, request).await.is_ok());
        assert_eq!(fixture.home_epochs().await, [Some(1); 3]);
    }
}

#[tokio::test]
async fn commit_failure_rolls_back_targets_and_receipt_together() {
    let fixture = Fixture::new(2).await;
    fixture.sql("CREATE TABLE receipt_commit_guard (id INTEGER REFERENCES operator_accounts(operator_id) DEFERRABLE INITIALLY DEFERRED); CREATE TRIGGER fail_commit AFTER INSERT ON target_submission_receipts BEGIN INSERT INTO receipt_commit_guard VALUES (123); END;".into()).await;
    let request = Fixture::request(TargetAction::ResetHome);
    assert!(matches!(
        fixture.submit(fixture.owner, request.clone()).await,
        Err(TargetSubmissionError::PersistenceFailed { .. })
    ));
    assert!(fixture.home_epochs().await.is_empty());
    assert_eq!(fixture.receipts().await, 0);
    fixture.sql("DROP TRIGGER fail_commit".into()).await;
    assert!(fixture.submit(fixture.owner, request).await.is_ok());
}

#[tokio::test]
async fn empty_and_all_rejected_submissions_are_replayable() {
    let fixture = Fixture::new(1).await;
    fixture
        .sql("UPDATE devices SET state='disabled'".into())
        .await;
    let empty = Fixture::request(TargetAction::ResetHome);
    let mut rejected = Fixture::request(TargetAction::ResetHome);
    rejected.scope = TargetScope::Devices(fixture.ids.clone());
    let a = fixture
        .submit(fixture.owner, empty.clone())
        .await
        .unwrap_or_else(|error| panic!("empty: {error}"));
    let b = fixture
        .submit(fixture.owner, rejected.clone())
        .await
        .unwrap_or_else(|error| panic!("rejected: {error}"));
    assert!(a.results.is_empty());
    assert_eq!(
        b.results[0].rejection,
        Some(TargetRejection::DeviceNotEnabled)
    );
    fixture
        .sql("UPDATE devices SET state='enabled'".into())
        .await;
    assert!(matches!(fixture.submit(fixture.owner, empty).await, Ok(actual) if actual == a));
    assert!(matches!(fixture.submit(fixture.owner, rejected).await, Ok(actual) if actual == b));
    assert!(fixture.home_epochs().await.is_empty());
}

#[tokio::test]
async fn foreground_and_terminate_share_the_submission_boundary() {
    let fixture = Fixture::new(2).await;
    fixture
        .sql(format!(
            "INSERT INTO device_session_targets VALUES ('{}','contest',{});",
            fixture.ids[0].as_text(),
            i64::MAX
        ))
        .await;
    let request = Fixture::request(TargetAction::TerminateSession);
    let result = fixture
        .submit(fixture.owner, request.clone())
        .await
        .unwrap_or_else(|error| panic!("terminate: {error}"));
    assert_eq!(
        result.results[0].rejection,
        Some(TargetRejection::EpochExhausted)
    );
    assert_eq!(result.results[1].rejection, None);
    assert!(matches!(fixture.submit(fixture.owner, request).await, Ok(actual) if actual == result));
    let foreground = Fixture::request(TargetAction::SetForeground(ForegroundTarget::Contest));
    assert!(fixture.submit(fixture.owner, foreground).await.is_ok());
    let targets: Vec<(String, Option<i64>)> = fixture
        .database
        .read(|tx| {
            device_session_targets::table
                .order(device_session_targets::device_id)
                .select((
                    device_session_targets::foreground_target,
                    device_session_targets::terminate_epoch,
                ))
                .load(tx.connection())
                .map_err(PersistenceError::from)
        })
        .await
        .unwrap_or_else(|error| panic!("targets: {error:?}"));
    assert_eq!(
        targets,
        [
            ("contest".into(), Some(i64::MAX)),
            ("contest".into(), Some(1))
        ]
    );
}

#[tokio::test]
#[ignore = "exploratory timing of actual 600-device writes; not a production capacity assertion"]
async fn measure_six_hundred_device_submission_cost() {
    let fixture = Fixture::new(600).await;
    for kind in ["devices", "all_enabled", "all_online_enabled"] {
        let mut elapsed = Vec::new();
        for _ in 0..10 {
            let expires = i64::try_from(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_else(|error| panic!("clock: {error}"))
                    .as_millis(),
            )
            .unwrap_or_default()
                + 60_000;
            let mut request = Fixture::request(TargetAction::ResetHome);
            request.scope = match kind {
                "devices" => TargetScope::Devices(fixture.ids.clone()),
                "all_enabled" => TargetScope::AllEnabled,
                _ => {
                    request.action = TargetAction::PowerOff {
                        expires_at_unix_ms: expires,
                    };
                    TargetScope::AllOnlineEnabled(fixture.ids.clone())
                }
            };
            let started = std::time::Instant::now();
            let result = fixture
                .submit(fixture.owner, request.clone())
                .await
                .unwrap_or_else(|error| panic!("submission: {error}"));
            elapsed.push(started.elapsed());
            assert_eq!(result.results.len(), 600);
            assert!(result.results.iter().all(|row| row.rejection.is_none()));
            let replay = fixture
                .submit(fixture.owner, request)
                .await
                .unwrap_or_else(|error| panic!("replay: {error}"));
            assert_eq!(replay, result);
        }
        elapsed.sort_unstable();
        tracing::info!(
            "{kind}: 600 actual device writes, 10 samples, p50={:?}, p95={:?}",
            elapsed[4],
            elapsed[9]
        );
    }
    assert_eq!(fixture.home_epochs().await, vec![Some(20); 600]);
    assert_eq!(fixture.receipts().await, 30);
}

#[tokio::test]
async fn six_hundred_devices_commit_without_any_client_connection() {
    let fixture = Fixture::new(600).await;
    let result = fixture
        .submit(fixture.owner, Fixture::request(TargetAction::ResetHome))
        .await
        .unwrap_or_else(|error| panic!("fleet: {error}"));
    assert_eq!(result.results.len(), 600);
    assert!(result.results.iter().all(|row| row.rejection.is_none()));
    assert_eq!(fixture.home_epochs().await, vec![Some(1); 600]);
}

#[tokio::test]
async fn ignored_mutations_report_row_counts_and_roll_back_instead_of_domain_rejection() {
    for action in [
        TargetAction::ResetHome,
        TargetAction::TerminateSession,
        TargetAction::PowerOff {
            expires_at_unix_ms: 60_000,
        },
    ] {
        let fixture = Fixture::new(1).await;
        let table = match action {
            TargetAction::ResetHome => "device_home_targets",
            TargetAction::TerminateSession => "device_session_targets",
            TargetAction::PowerOff { .. } => "device_power_targets",
            TargetAction::SetForeground(_) => unreachable!(),
        };
        fixture.sql(format!("CREATE TRIGGER ignore_target BEFORE INSERT ON {table} BEGIN SELECT RAISE(IGNORE); END;")).await;
        let error = fixture
            .submit(fixture.owner, Fixture::request(action))
            .await;
        assert!(matches!(
            error,
            Err(TargetSubmissionError::PersistenceFailed {
                source: PersistenceError::UnexpectedRowCount {
                    expected: 1,
                    actual: 0,
                    ..
                }
            })
        ));
        assert_eq!(fixture.receipts().await, 0);
    }
}
