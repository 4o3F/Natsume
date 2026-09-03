use std::{fs, path::PathBuf};

use diesel::{ExpressionMethods, QueryDsl, RunQueryDsl};
use uuid::Uuid;

use crate::{
    component::device::DeviceId,
    db::{Database, DatabaseConfig, PersistenceError},
    diesel_schema::{device_session_targets, devices},
};

use super::{LockState, SessionControlComponent, SessionControlError, SessionControlTarget};

#[tokio::test]
async fn materialize_creates_the_default_target_for_an_existing_device() {
    let fixture = Fixture::new().await;
    let device_id = fixture.insert_device().await;

    assert_eq!(fixture.component.read_current(device_id).await, Ok(None));
    assert_eq!(fixture.target_count(device_id).await, 0);

    assert_eq!(
        fixture
            .component
            .materialize(device_id)
            .await
            .unwrap_or_else(|error| panic!("default target failed: {error}")),
        SessionControlTarget {
            lock_state: LockState::Unlocked,
            terminate_epoch: None,
        }
    );
    assert_eq!(fixture.target_count(device_id).await, 1);
    assert_eq!(
        fixture.component.read_current(device_id).await,
        Ok(Some(SessionControlTarget {
            lock_state: LockState::Unlocked,
            terminate_epoch: None,
        }))
    );
}

#[tokio::test]
async fn set_lock_is_idempotent_and_preserves_the_terminate_epoch() {
    let fixture = Fixture::new().await;
    let device_id = fixture.insert_device().await;
    fixture
        .component
        .terminate(device_id)
        .await
        .unwrap_or_else(|error| panic!("terminate setup failed: {error}"));

    let locked = fixture
        .component
        .set_lock(device_id, LockState::Locked)
        .await
        .unwrap_or_else(|error| panic!("lock failed: {error}"));
    let replay = fixture
        .component
        .set_lock(device_id, LockState::Locked)
        .await
        .unwrap_or_else(|error| panic!("lock replay failed: {error}"));

    assert_eq!(locked, replay);
    assert_eq!(locked.lock_state, LockState::Locked);
    assert_eq!(locked.terminate_epoch, Some(1));
    assert_eq!(fixture.target_count(device_id).await, 1);
}

#[tokio::test]
async fn terminate_target_remains_durable_across_component_rebuild() {
    let fixture = Fixture::new().await;
    let device_id = fixture.insert_device().await;
    let terminated = fixture
        .component
        .terminate(device_id)
        .await
        .unwrap_or_else(|error| panic!("terminate failed: {error}"));
    assert_eq!(terminated.terminate_epoch, Some(1));

    let rebuilt = SessionControlComponent::new(fixture.database.clone());
    assert_eq!(
        rebuilt
            .materialize(device_id)
            .await
            .unwrap_or_else(|error| panic!("rebuilt materialization failed: {error}")),
        terminated
    );
}

#[tokio::test]
async fn concurrent_terminate_requests_advance_once_each() {
    let fixture = Fixture::new().await;
    let device_id = fixture.insert_device().await;

    let (first, second) = tokio::join!(
        fixture.component.terminate(device_id),
        fixture.component.terminate(device_id)
    );
    let mut epochs = [
        first
            .unwrap_or_else(|error| panic!("first terminate failed: {error}"))
            .terminate_epoch,
        second
            .unwrap_or_else(|error| panic!("second terminate failed: {error}"))
            .terminate_epoch,
    ];
    epochs.sort_unstable();
    assert_eq!(epochs, [Some(1), Some(2)]);
    assert_eq!(
        fixture
            .component
            .materialize(device_id)
            .await
            .unwrap_or_else(|error| panic!("final materialization failed: {error}"))
            .terminate_epoch,
        Some(2)
    );
}

#[tokio::test]
async fn invalid_or_exhausted_persisted_targets_fail_closed() {
    let fixture = Fixture::new().await;
    let unknown_lock = fixture.insert_device().await;
    fixture
        .insert_raw_target(unknown_lock, "unknown", None)
        .await;
    assert_eq!(
        fixture.component.materialize(unknown_lock).await,
        Err(SessionControlError::InvalidPersistedFacts)
    );

    for invalid_epoch in [0, -1] {
        let device_id = fixture.insert_device().await;
        fixture
            .insert_raw_target(device_id, "unlocked", Some(invalid_epoch))
            .await;
        assert_eq!(
            fixture.component.materialize(device_id).await,
            Err(SessionControlError::InvalidPersistedFacts)
        );
    }

    let exhausted = fixture.insert_device().await;
    fixture
        .insert_raw_target(exhausted, "locked", Some(i64::MAX))
        .await;
    assert_eq!(
        fixture.component.terminate(exhausted).await,
        Err(SessionControlError::TerminateEpochOverflow)
    );
    assert_eq!(
        fixture
            .component
            .materialize(exhausted)
            .await
            .unwrap_or_else(|error| panic!("exhausted target became unreadable: {error}"))
            .terminate_epoch,
        Some(i64::MAX.cast_unsigned())
    );
}

#[tokio::test]
async fn missing_device_returns_the_typed_error_without_creating_a_target() {
    let fixture = Fixture::new().await;
    let missing = DeviceId::parse("01900000-0000-7000-8000-000000000099")
        .unwrap_or_else(|| panic!("missing Device fixture ID was invalid"));

    assert_eq!(
        fixture.component.materialize(missing).await,
        Err(SessionControlError::DeviceNotFound)
    );
    assert_eq!(
        fixture.component.read_current(missing).await,
        Err(SessionControlError::DeviceNotFound)
    );
    assert_eq!(fixture.target_count(missing).await, 0);
}

struct Fixture {
    root: PathBuf,
    database: Database,
    component: SessionControlComponent,
}

impl Fixture {
    async fn new() -> Self {
        let root = std::env::temp_dir().join(format!("natsume-session-{}", Uuid::now_v7()));
        fs::create_dir(&root)
            .unwrap_or_else(|_| panic!("Session Control fixture directory could not be created"));
        let database =
            Database::connect_and_migrate(&DatabaseConfig::new(root.join("server.sqlite3"), true))
                .await
                .unwrap_or_else(|error| panic!("Session Control fixture failed: {error:?}"));
        let component = SessionControlComponent::new(database.clone());
        Self {
            root,
            database,
            component,
        }
    }

    async fn insert_device(&self) -> DeviceId {
        let device_id = Uuid::now_v7();
        let machine_hardware_id = Uuid::new_v5(&Uuid::NAMESPACE_OID, device_id.as_bytes());
        let device_id_text = device_id.hyphenated().to_string();
        let typed_device_id = DeviceId::parse(&device_id_text)
            .unwrap_or_else(|| panic!("Session Control fixture generated an invalid Device ID"));
        self.database
            .write(move |transaction| {
                diesel::insert_into(devices::table)
                    .values((
                        devices::device_id.eq(device_id_text),
                        devices::machine_hardware_id
                            .eq(machine_hardware_id.hyphenated().to_string()),
                        devices::evidence_quality.eq("strong"),
                        devices::state.eq("enabled"),
                        devices::created_at_unix_ms.eq(1_i64),
                    ))
                    .execute(transaction.connection())
                    .map_err(|_| PersistenceError::OperationFailed)?;
                Ok::<(), PersistenceError>(())
            })
            .await
            .unwrap_or_else(|error| panic!("Session Control Device fixture failed: {error:?}"));
        typed_device_id
    }

    async fn insert_raw_target(
        &self,
        device_id: DeviceId,
        lock_state: &'static str,
        terminate_epoch: Option<i64>,
    ) {
        let device_id = device_id.as_text();
        self.database
            .write(move |transaction| {
                diesel::insert_into(device_session_targets::table)
                    .values((
                        device_session_targets::device_id.eq(device_id),
                        device_session_targets::lock_state.eq(lock_state),
                        device_session_targets::terminate_epoch.eq(terminate_epoch),
                    ))
                    .execute(transaction.connection())
                    .map_err(|_| PersistenceError::OperationFailed)?;
                Ok::<(), PersistenceError>(())
            })
            .await
            .unwrap_or_else(|error| panic!("raw Session Control target failed: {error:?}"));
    }

    async fn target_count(&self, device_id: DeviceId) -> i64 {
        let device_id = device_id.as_text();
        self.database
            .read(move |transaction| {
                device_session_targets::table
                    .filter(device_session_targets::device_id.eq(device_id))
                    .count()
                    .get_result::<i64>(transaction.connection())
                    .map_err(|_| PersistenceError::OperationFailed)
            })
            .await
            .unwrap_or_else(|error| panic!("Session Control target count failed: {error:?}"))
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
