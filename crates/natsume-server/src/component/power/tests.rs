use diesel::{ExpressionMethods as _, QueryDsl as _, RunQueryDsl as _};
use tempfile::TempDir;
use uuid::Uuid;

use super::{PowerControlComponent, PowerControlTarget, PowerError};
use crate::{
    component::device::DeviceId,
    db::{Database, DatabaseConfig, PersistenceError},
    diesel_schema::{device_power_targets, devices},
};

type TestResult = Result<(), crate::test_error::TestError>;

async fn fixture()
-> Result<(TempDir, Database, DeviceId, PowerControlComponent), crate::test_error::TestError> {
    let root = tempfile::tempdir()?;
    let database = Database::connect_and_migrate(&DatabaseConfig::new(
        root.path().join("server.sqlite3"),
        true,
    ))
    .await?;
    let id = Uuid::now_v7();
    let device_id = DeviceId::parse(&id.to_string())
        .ok_or(crate::test_error::TestError::missing("device ID"))?;
    database
        .write(move |transaction| {
            diesel::insert_into(devices::table)
                .values((
                    devices::device_id.eq(id.to_string()),
                    devices::machine_hardware_id.eq(Uuid::new_v5(
                        &Uuid::NAMESPACE_OID,
                        id.as_bytes(),
                    )
                    .to_string()),
                    devices::evidence_quality.eq("strong"),
                    devices::state.eq("enabled"),
                    devices::created_at_unix_ms.eq(1_i64),
                ))
                .execute(transaction.connection())
                .map(|_| ())
                .map_err(PersistenceError::from)
        })
        .await?;
    let component = PowerControlComponent::new(database.clone());
    Ok((root, database, device_id, component))
}

async fn target_count(database: &Database) -> i64 {
    database
        .read(|transaction| {
            device_power_targets::table
                .count()
                .get_result(transaction.connection())
                .map_err(PersistenceError::from)
        })
        .await
        .map_err(crate::db::TransactionError::into_error)
        .unwrap_or_else(|error| panic!("{}", snafu::Report::from_error(error)))
}

#[tokio::test]
async fn materialize_defaults_and_stored_shutdown_do_not_insert_or_wait_for_a_writer() -> TestResult
{
    let (root, database, device_id, component) = fixture().await?;
    let lock = crate::db::tests::test_lock_database(&root.path().join("server.sqlite3"))?;
    let target = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        component.materialize(device_id),
    )
    .await?
    .unwrap_or_else(|error| panic!("{}", snafu::Report::from_error(error)));
    assert_eq!(
        target,
        PowerControlTarget {
            shutdown_epoch: None,
            expires_at_unix_ms: None
        }
    );
    assert_eq!(target_count(&database).await, 0);
    drop(lock);
    let stored = database
        .write(move |transaction| {
            PowerControlComponent::request_shutdown_in_transaction(transaction, &device_id, 60_000)
        })
        .await
        .map_err(crate::db::TransactionError::into_error)
        .unwrap_or_else(|error| panic!("{}", snafu::Report::from_error(error)));
    assert_eq!(stored.shutdown_epoch(), Some(1));
    let _lock = crate::db::tests::test_lock_database(&root.path().join("server.sqlite3"))?;
    let target = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        component.materialize(device_id),
    )
    .await?
    .unwrap_or_else(|error| panic!("{}", snafu::Report::from_error(error)));
    assert_eq!(target, stored);
    assert_eq!(target.expires_at_unix_ms(), Some(60_000));
    assert_eq!(target_count(&database).await, 1);
    Ok(())
}

#[tokio::test]
async fn missing_devices_and_invalid_persisted_shutdowns_still_fail_closed() -> TestResult {
    let (_root, database, device_id, component) = fixture().await?;
    let missing = DeviceId::parse(&Uuid::now_v7().to_string())
        .ok_or(crate::test_error::TestError::missing("device ID"))?;
    assert!(matches!(
        component.materialize(missing).await,
        Err(PowerError::DeviceNotFound)
    ));
    assert_eq!(target_count(&database).await, 0);
    database
        .write(move |transaction| {
            diesel::insert_into(device_power_targets::table)
                .values((
                    device_power_targets::device_id.eq(device_id.as_text()),
                    device_power_targets::shutdown_epoch.eq(Some(0_i64)),
                    device_power_targets::expires_at_unix_ms.eq(Some(1_i64)),
                ))
                .execute(transaction.connection())
                .map(|_| ())
                .map_err(PersistenceError::from)
        })
        .await?;
    assert!(matches!(
        component.materialize(device_id).await,
        Err(PowerError::InvalidPersistedFacts { .. })
    ));
    Ok(())
}
