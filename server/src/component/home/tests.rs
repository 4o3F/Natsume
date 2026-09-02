use std::{fs, path::PathBuf};

use diesel::{ExpressionMethods, QueryDsl, RunQueryDsl};
use uuid::Uuid;

use crate::{
    component::device::DeviceId,
    db::{Database, DatabaseConfig, PersistenceError},
    diesel_schema::{device_home_targets, devices},
};

use super::{HomeComponent, HomeError};

#[tokio::test]
async fn materialize_creates_default_and_reset_advances() {
    let fixture = Fixture::new().await;
    let device_id = fixture.insert_device().await;

    assert_eq!(fixture.component.materialize(device_id).await, Ok(None));
    assert_eq!(fixture.target_count(device_id).await, 1);
    assert_eq!(fixture.component.reset(device_id).await, Ok(1));
    assert_eq!(fixture.component.reset(device_id).await, Ok(2));
    assert_eq!(fixture.component.materialize(device_id).await, Ok(Some(2)));
}

#[tokio::test]
async fn reset_epoch_survives_component_rebuild() {
    let fixture = Fixture::new().await;
    let device_id = fixture.insert_device().await;

    assert_eq!(fixture.component.reset(device_id).await, Ok(1));
    let rebuilt = HomeComponent::new(fixture.database.clone());
    assert_eq!(rebuilt.materialize(device_id).await, Ok(Some(1)));
    assert_eq!(rebuilt.reset(device_id).await, Ok(2));
}

#[tokio::test]
async fn concurrent_resets_do_not_lose_epoch_advances() {
    let fixture = Fixture::new().await;
    let device_id = fixture.insert_device().await;

    let (first, second, third, fourth) = tokio::join!(
        fixture.component.reset(device_id),
        fixture.component.reset(device_id),
        fixture.component.reset(device_id),
        fixture.component.reset(device_id),
    );
    let mut epochs = [first, second, third, fourth]
        .map(|result| result.unwrap_or_else(|error| panic!("concurrent reset failed: {error}")));
    epochs.sort_unstable();
    assert_eq!(epochs, [1, 2, 3, 4]);
    assert_eq!(fixture.component.materialize(device_id).await, Ok(Some(4)));
}

#[tokio::test]
async fn missing_device_invalid_epoch_and_overflow_fail_closed() {
    let fixture = Fixture::new().await;
    let missing = DeviceId::parse(&Uuid::now_v7().hyphenated().to_string())
        .unwrap_or_else(|| panic!("fixture generated an invalid Device ID"));
    assert_eq!(
        fixture.component.materialize(missing).await,
        Err(HomeError::DeviceNotFound)
    );
    assert_eq!(
        fixture.component.reset(missing).await,
        Err(HomeError::DeviceNotFound)
    );

    let device_id = fixture.insert_device().await;
    assert_eq!(fixture.component.materialize(device_id).await, Ok(None));

    fixture.set_persisted_epoch(device_id, 0).await;
    assert_eq!(
        fixture.component.materialize(device_id).await,
        Err(HomeError::InvalidPersistedFacts)
    );
    fixture.set_persisted_epoch(device_id, -1).await;
    assert_eq!(
        fixture.component.reset(device_id).await,
        Err(HomeError::InvalidPersistedFacts)
    );

    fixture.set_persisted_epoch(device_id, i64::MAX).await;
    assert_eq!(
        fixture.component.materialize(device_id).await,
        Ok(Some(i64::MAX.cast_unsigned()))
    );
    assert_eq!(
        fixture.component.reset(device_id).await,
        Err(HomeError::EpochExhausted)
    );
    assert_eq!(
        fixture.component.materialize(device_id).await,
        Ok(Some(i64::MAX.cast_unsigned()))
    );
}

struct Fixture {
    path: PathBuf,
    database: Database,
    component: HomeComponent,
}

impl Fixture {
    async fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "natsume-home-component-test-{}.sqlite3",
            Uuid::now_v7()
        ));
        let database = Database::connect_and_migrate(&DatabaseConfig::new(&path, true))
            .await
            .unwrap_or_else(|error| panic!("test database creation failed: {error:?}"));
        let component = HomeComponent::new(database.clone());
        Self {
            path,
            database,
            component,
        }
    }

    async fn insert_device(&self) -> DeviceId {
        let device_id = Uuid::now_v7();
        let machine_hardware_id = Uuid::new_v5(&Uuid::NAMESPACE_OID, device_id.as_bytes());
        let device_id_text = device_id.hyphenated().to_string();
        let typed_device_id = DeviceId::parse(&device_id_text)
            .unwrap_or_else(|| panic!("fixture generated an invalid Device ID"));
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
            .unwrap_or_else(|error| panic!("Device fixture insertion failed: {error:?}"));
        typed_device_id
    }

    async fn target_count(&self, device_id: DeviceId) -> i64 {
        let device_id = device_id.as_text();
        self.database
            .read(move |transaction| {
                device_home_targets::table
                    .filter(device_home_targets::device_id.eq(device_id))
                    .count()
                    .get_result::<i64>(transaction.connection())
                    .map_err(|_| PersistenceError::OperationFailed)
            })
            .await
            .unwrap_or_else(|error| panic!("Home target count failed: {error:?}"))
    }

    async fn set_persisted_epoch(&self, device_id: DeviceId, epoch: i64) {
        let device_id = device_id.as_text();
        self.database
            .write(move |transaction| {
                diesel::update(
                    device_home_targets::table.filter(device_home_targets::device_id.eq(device_id)),
                )
                .set(device_home_targets::reset_epoch.eq(Some(epoch)))
                .execute(transaction.connection())
                .map_err(|_| PersistenceError::OperationFailed)?;
                Ok::<(), PersistenceError>(())
            })
            .await
            .unwrap_or_else(|error| panic!("persisted Home epoch setup failed: {error:?}"));
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for path in [
            self.path.clone(),
            PathBuf::from(format!("{}-wal", self.path.display())),
            PathBuf::from(format!("{}-shm", self.path.display())),
        ] {
            let _ = fs::remove_file(path);
        }
    }
}
