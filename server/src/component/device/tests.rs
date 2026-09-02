use std::{fs, path::PathBuf, sync::mpsc, time::Duration};

use diesel::{ExpressionMethods, QueryDsl, RunQueryDsl};
use uuid::Uuid;

use crate::{
    component::provisioning::ProvisioningComponent,
    db::{Database, DatabaseConfig, PersistenceError, TransactionError},
    diesel_schema::{device_control_keys, devices},
};

use super::{
    ActivationError, ControlPublicKey, DeviceComponent, DeviceError, DeviceId, DeviceState,
    EnrollmentApprovalError, EnrollmentStartError, EnrollmentStartOutcome, EvidenceQuality,
    LifecycleOutcome, MachineHardwareId, ValidatedEnrollmentEvidence, db,
};

const FIRST_MACHINE: &str = "a9aa9d04-3ece-5567-8260-910930ff5e03";
const SECOND_MACHINE: &str = "bbbbbbbb-bbbb-5bbb-8bbb-bbbbbbbbbbbb";

#[tokio::test]
async fn first_activation_is_atomic_and_exact_replay_is_a_no_op() {
    let fixture = Fixture::new().await;
    let machine = machine(FIRST_MACHINE);
    let candidate = key(0x11);
    let activated = activate(&fixture, machine, candidate, EvidenceQuality::Strong)
        .await
        .unwrap_or_else(|error| panic!("first activation failed: {error}"));
    let device_id = activated.device_id();

    let current = current_authority(&fixture, machine).await;
    assert_eq!(current.device_id(), device_id);
    assert_eq!(current.control_public_key(), candidate);
    assert_eq!(current.device_state(), DeviceState::Enabled);
    let replayed = activate(&fixture, machine, candidate, EvidenceQuality::Medium)
        .await
        .unwrap_or_else(|error| panic!("exact authority replay failed: {error}"));
    assert_eq!(replayed, activated);

    let devices = fixture.device_rows().await;
    let keys = fixture.control_key_rows().await;
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].2, "strong");
    assert_eq!(devices[0].3, "enabled");
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].0, candidate.as_bytes());
    assert_eq!(keys[0].2, "current");
    assert!(keys[0].4.is_none());
}

#[tokio::test]
async fn replacement_keeps_the_device_and_retires_the_old_key() {
    let fixture = Fixture::new().await;
    let machine = machine(FIRST_MACHINE);
    let old_key = key(0x21);
    let new_key = key(0x22);
    let activated = activate(&fixture, machine, old_key, EvidenceQuality::Strong)
        .await
        .unwrap_or_else(|error| panic!("first activation failed: {error}"));
    let device_id = activated.device_id();

    let replaced = activate(&fixture, machine, new_key, EvidenceQuality::Medium)
        .await
        .unwrap_or_else(|error| panic!("replacement activation failed: {error}"));
    assert_eq!(replaced.device_id(), device_id);
    assert_eq!(replaced.control_public_key(), new_key);
    assert_eq!(
        current_authority(&fixture, machine)
            .await
            .control_public_key(),
        new_key
    );
    let devices = fixture.device_rows().await;
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].2, "medium");
    let keys = fixture.control_key_rows().await;
    assert_eq!(keys.len(), 2);
    let old = keys
        .iter()
        .find(|row| row.0.as_slice() == old_key.as_bytes())
        .unwrap_or_else(|| panic!("retired control key was not preserved"));
    let new = keys
        .iter()
        .find(|row| row.0.as_slice() == new_key.as_bytes())
        .unwrap_or_else(|| panic!("new current control key was not persisted"));
    assert_eq!(old.2, "retired");
    assert!(old.4.is_some());
    assert_eq!(new.2, "current");
    assert!(new.4.is_none());
}

#[tokio::test]
async fn a_key_already_owned_by_another_device_cannot_replace_authority() {
    let fixture = Fixture::new().await;
    let first_machine = machine(FIRST_MACHINE);
    let second_machine = machine(SECOND_MACHINE);
    let first_key = key(0x31);
    let second_key = key(0x32);
    activate(&fixture, first_machine, first_key, EvidenceQuality::Strong)
        .await
        .unwrap_or_else(|error| panic!("first Device activation failed: {error}"));
    activate(
        &fixture,
        second_machine,
        second_key,
        EvidenceQuality::Strong,
    )
    .await
    .unwrap_or_else(|error| panic!("second Device activation failed: {error}"));

    assert_eq!(
        activate(&fixture, first_machine, second_key, EvidenceQuality::Medium).await,
        Err(ActivationError::CandidateKeyRejected)
    );
    assert_eq!(
        current_authority(&fixture, first_machine)
            .await
            .control_public_key(),
        first_key
    );
    assert_eq!(
        current_authority(&fixture, second_machine)
            .await
            .control_public_key(),
        second_key
    );
}

#[tokio::test]
async fn disabled_authority_is_retained_and_revoke_is_terminal() {
    let fixture = Fixture::new().await;
    let machine = machine(FIRST_MACHINE);
    let old_key = key(0x41);
    let activated = activate(&fixture, machine, old_key, EvidenceQuality::Strong)
        .await
        .unwrap_or_else(|error| panic!("first activation failed: {error}"));
    let device_id = activated.device_id();

    assert_eq!(
        fixture.component.disable(device_id).await,
        Ok(LifecycleOutcome::Changed)
    );
    let disabled = current_authority(&fixture, machine).await;
    assert_eq!(disabled.control_public_key(), old_key);
    assert_eq!(disabled.device_state(), DeviceState::Disabled);
    assert_eq!(
        fixture.component.disable(device_id).await,
        Ok(LifecycleOutcome::Unchanged)
    );
    assert_eq!(
        fixture.component.enable(device_id).await,
        Ok(LifecycleOutcome::Changed)
    );

    assert_eq!(
        fixture.component.revoke(device_id).await,
        Ok(LifecycleOutcome::Changed)
    );
    assert_eq!(
        fixture.component.find_current_authority(machine).await,
        Ok(None)
    );
    assert_eq!(
        fixture.component.enable(device_id).await,
        Ok(LifecycleOutcome::RejectedTerminal)
    );
    assert_eq!(
        fixture.component.revoke(device_id).await,
        Ok(LifecycleOutcome::Unchanged)
    );
    assert_eq!(
        activate(&fixture, machine, old_key, EvidenceQuality::Strong).await,
        Err(ActivationError::CandidateKeyRejected)
    );

    let new_key = key(0x42);
    let new_authority = activate(&fixture, machine, new_key, EvidenceQuality::Medium)
        .await
        .unwrap_or_else(|error| panic!("replacement activation failed: {error}"));
    let new_device_id = new_authority.device_id();
    assert_ne!(new_device_id, device_id);
    assert_eq!(
        current_authority(&fixture, machine)
            .await
            .control_public_key(),
        new_key
    );
    let devices = fixture.device_rows().await;
    assert_eq!(devices.len(), 2);
    assert_eq!(devices.iter().filter(|row| row.3 == "revoked").count(), 1);
    assert_eq!(devices.iter().filter(|row| row.3 == "enabled").count(), 1);
}

#[tokio::test]
async fn old_authority_remains_visible_until_replacement_commit() {
    let fixture = Fixture::new().await;
    let machine = machine(FIRST_MACHINE);
    let old_key = key(0x51);
    let new_key = key(0x52);
    activate(&fixture, machine, old_key, EvidenceQuality::Strong)
        .await
        .unwrap_or_else(|error| panic!("first activation failed: {error}"));

    let (retired_sender, retired_receiver) = tokio::sync::oneshot::channel();
    let (continue_sender, continue_receiver) = mpsc::channel();
    let writer_database = fixture.database.clone();
    let writer = tokio::spawn(async move {
        writer_database
            .write(move |transaction| -> Result<(), PersistenceError> {
                let device = db::find_non_revoked_by_machine(transaction, &machine)?
                    .ok_or(PersistenceError::InvalidPersistedData)?;
                let current = db::find_current_for_device(transaction, &device.device_id())?
                    .ok_or(PersistenceError::InvalidPersistedData)?;
                let now = db::current_unix_ms(transaction)?;
                if db::retire_current(transaction, &current, &device.device_id(), now)? != 1 {
                    return Err(PersistenceError::InvalidPersistedData);
                }
                retired_sender
                    .send(())
                    .map_err(|()| PersistenceError::OperationFailed)?;
                continue_receiver
                    .recv_timeout(Duration::from_secs(5))
                    .map_err(|_| PersistenceError::OperationFailed)?;
                if db::insert_current(transaction, &new_key, &device.device_id(), now)? != 1 {
                    return Err(PersistenceError::InvalidPersistedData);
                }
                Ok(())
            })
            .await
            .map_err(TransactionError::into_error)
    });

    retired_receiver
        .await
        .unwrap_or_else(|_| panic!("replacement transaction did not reach the commit barrier"));
    let during = fixture.component.find_current_authority(machine).await;
    continue_sender
        .send(())
        .unwrap_or_else(|_| panic!("replacement transaction stopped before commit"));
    writer
        .await
        .unwrap_or_else(|error| panic!("replacement task failed: {error}"))
        .unwrap_or_else(|error| panic!("replacement transaction failed: {error}"));

    assert_eq!(
        during
            .unwrap_or_else(|error| panic!("pre-commit authority read failed: {error}"))
            .unwrap_or_else(|| panic!("old authority disappeared before commit"))
            .control_public_key(),
        old_key
    );
    assert_eq!(
        current_authority(&fixture, machine)
            .await
            .control_public_key(),
        new_key
    );
}

#[tokio::test]
async fn invalid_persisted_lifecycle_fails_closed() {
    let fixture = Fixture::new().await;
    let machine = machine(FIRST_MACHINE);
    let device_id = "01900000-0000-7000-8000-000000000091".to_owned();
    let machine_text = machine.as_text();
    let public_key = key(0x61);
    let key_bytes = public_key.as_bytes().to_vec();
    fixture
        .database
        .write(move |transaction| {
            diesel::insert_into(devices::table)
                .values((
                    devices::device_id.eq(device_id),
                    devices::machine_hardware_id.eq(machine_text),
                    devices::evidence_quality.eq("strong"),
                    devices::state.eq("corrupt"),
                    devices::created_at_unix_ms.eq(1_i64),
                ))
                .execute(transaction.connection())
                .map_err(|_| PersistenceError::OperationFailed)?;
            diesel::insert_into(device_control_keys::table)
                .values((
                    device_control_keys::public_key.eq(key_bytes),
                    device_control_keys::device_id.eq("01900000-0000-7000-8000-000000000091"),
                    device_control_keys::status.eq("current"),
                    device_control_keys::activated_at_unix_ms.eq(1_i64),
                    device_control_keys::retired_at_unix_ms.eq(None::<i64>),
                ))
                .execute(transaction.connection())
                .map_err(|_| PersistenceError::OperationFailed)?;
            Ok::<(), PersistenceError>(())
        })
        .await
        .map_err(TransactionError::into_error)
        .unwrap_or_else(|error| panic!("invalid persisted fixture failed: {error}"));

    assert_eq!(
        fixture.component.find_current_authority(machine).await,
        Err(DeviceError::InvalidPersistedFacts)
    );
}

#[tokio::test]
async fn lifecycle_mutation_rejects_an_unknown_device() {
    let fixture = Fixture::new().await;
    let missing = DeviceId::parse("01900000-0000-7000-8000-000000000099")
        .unwrap_or_else(|| panic!("missing Device fixture ID is invalid"));
    assert_eq!(
        fixture.component.disable(missing).await,
        Err(DeviceError::DeviceNotFound)
    );
    assert_eq!(
        fixture.component.revoke(missing).await,
        Err(DeviceError::DeviceNotFound)
    );
}

#[tokio::test]
async fn exact_replay_bypasses_the_closed_gate_but_a_new_candidate_does_not() {
    let fixture = Fixture::new().await;
    let provisioning = ProvisioningComponent::new();
    let machine = machine(FIRST_MACHINE);
    let current_key = key(0x71);
    let current = activate(&fixture, machine, current_key, EvidenceQuality::Strong)
        .await
        .unwrap_or_else(|error| panic!("authority fixture activation failed: {error}"));

    let replay = fixture
        .component
        .start_enrollment(&provisioning, evidence(machine, current_key))
        .await;
    assert_eq!(replay, Ok(EnrollmentStartOutcome::Replay(current)));

    let rejected = fixture
        .component
        .start_enrollment(&provisioning, evidence(machine, key(0x72)))
        .await;
    assert_eq!(rejected, Err(EnrollmentStartError::ProvisioningClosed));
    assert!(
        fixture
            .component
            .pending_enrollment_reviews()
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn approval_rechecks_the_gate_and_claims_the_review_once() {
    let fixture = Fixture::new().await;
    let provisioning = ProvisioningComponent::new();
    provisioning.open_window().await;
    let machine = machine(FIRST_MACHINE);
    let candidate = key(0x73);
    let pending = fixture
        .component
        .start_enrollment(&provisioning, evidence(machine, candidate))
        .await
        .unwrap_or_else(|error| panic!("review creation failed: {error:?}"));
    let EnrollmentStartOutcome::Pending(pending) = pending else {
        panic!("new authority candidate unexpectedly replayed");
    };

    provisioning.close_window().await;
    assert_eq!(
        fixture
            .component
            .approve_enrollment(&provisioning, pending.review_id())
            .await,
        Err(EnrollmentApprovalError::ProvisioningClosed)
    );
    assert_eq!(
        fixture.component.pending_enrollment_reviews().await.len(),
        1
    );
    assert_eq!(
        fixture.component.find_current_authority(machine).await,
        Ok(None)
    );

    provisioning.open_window().await;
    let activated = fixture
        .component
        .approve_enrollment(&provisioning, pending.review_id())
        .await
        .unwrap_or_else(|error| panic!("fenced approval failed: {error:?}"));
    assert_eq!(activated.control_public_key(), candidate);
    assert!(
        fixture
            .component
            .pending_enrollment_reviews()
            .await
            .is_empty()
    );
    assert_eq!(
        fixture
            .component
            .approve_enrollment(&provisioning, pending.review_id())
            .await,
        Err(EnrollmentApprovalError::ReviewNotFound)
    );
}

#[tokio::test]
async fn pre_activation_disconnect_restarts_review_and_post_commit_disconnect_replays() {
    let fixture = Fixture::new().await;
    let provisioning = ProvisioningComponent::new();
    provisioning.open_window().await;
    let machine = machine(FIRST_MACHINE);
    let candidate = key(0x74);
    let first = fixture
        .component
        .start_enrollment(&provisioning, evidence(machine, candidate))
        .await
        .unwrap_or_else(|error| panic!("first review creation failed: {error:?}"));
    let EnrollmentStartOutcome::Pending(first) = first else {
        panic!("new authority candidate unexpectedly replayed");
    };
    assert!(
        fixture
            .component
            .remove_enrollment_review(first.review_id())
            .await
    );
    assert_eq!(
        fixture
            .component
            .approve_enrollment(&provisioning, first.review_id())
            .await,
        Err(EnrollmentApprovalError::ReviewNotFound)
    );
    assert_eq!(
        fixture.component.find_current_authority(machine).await,
        Ok(None)
    );

    let second = fixture
        .component
        .start_enrollment(&provisioning, evidence(machine, candidate))
        .await
        .unwrap_or_else(|error| panic!("second review creation failed: {error:?}"));
    let EnrollmentStartOutcome::Pending(second) = second else {
        panic!("uncommitted authority candidate unexpectedly replayed");
    };
    let activated = fixture
        .component
        .approve_enrollment(&provisioning, second.review_id())
        .await
        .unwrap_or_else(|error| panic!("second review activation failed: {error:?}"));

    provisioning.close_window().await;
    let replay = fixture
        .component
        .start_enrollment(&provisioning, evidence(machine, candidate))
        .await;
    assert_eq!(replay, Ok(EnrollmentStartOutcome::Replay(activated)));
    assert!(
        fixture
            .component
            .pending_enrollment_reviews()
            .await
            .is_empty()
    );
}

async fn current_authority(
    fixture: &Fixture,
    machine_hardware_id: MachineHardwareId,
) -> super::ControlAuthority {
    fixture
        .component
        .find_current_authority(machine_hardware_id)
        .await
        .unwrap_or_else(|error| panic!("current authority lookup failed: {error}"))
        .unwrap_or_else(|| panic!("current authority was unexpectedly absent"))
}

async fn activate(
    fixture: &Fixture,
    machine_hardware_id: MachineHardwareId,
    candidate_public_key: ControlPublicKey,
    evidence_quality: EvidenceQuality,
) -> Result<super::ControlAuthority, ActivationError> {
    super::authority::activate(
        &fixture.database,
        machine_hardware_id,
        candidate_public_key,
        evidence_quality,
    )
    .await
}

fn machine(value: &str) -> MachineHardwareId {
    MachineHardwareId::parse(value)
        .unwrap_or_else(|| panic!("Machine Hardware ID fixture is invalid: {value}"))
}

fn key(seed: u8) -> ControlPublicKey {
    ControlPublicKey::parse(&[seed; 32])
        .unwrap_or_else(|| panic!("the fixture control key is valid"))
}

fn evidence(
    machine_hardware_id: MachineHardwareId,
    candidate_public_key: ControlPublicKey,
) -> ValidatedEnrollmentEvidence {
    ValidatedEnrollmentEvidence::new(
        machine_hardware_id,
        candidate_public_key,
        EvidenceQuality::Strong,
        "2.0.0".to_owned(),
        "2.0.0".to_owned(),
    )
}

type PersistedDeviceRow = (String, String, String, String, i64);
type PersistedControlKeyRow = (Vec<u8>, String, String, i64, Option<i64>);

struct Fixture {
    path: PathBuf,
    database: Database,
    component: DeviceComponent,
}

impl Fixture {
    async fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "natsume-device-authority-test-{}.sqlite3",
            Uuid::now_v7()
        ));
        let database = Database::connect_and_migrate(&DatabaseConfig::new(&path, true))
            .await
            .unwrap_or_else(|error| panic!("test database creation failed: {error:?}"));
        let component = DeviceComponent::new(database.clone());
        Self {
            path,
            database,
            component,
        }
    }

    async fn device_rows(&self) -> Vec<PersistedDeviceRow> {
        self.database
            .read(|transaction| {
                devices::table
                    .select((
                        devices::device_id,
                        devices::machine_hardware_id,
                        devices::evidence_quality,
                        devices::state,
                        devices::created_at_unix_ms,
                    ))
                    .order(devices::device_id)
                    .load::<PersistedDeviceRow>(transaction.connection())
                    .map_err(|_| PersistenceError::OperationFailed)
            })
            .await
            .map_err(TransactionError::into_error)
            .unwrap_or_else(|error| panic!("persisted Device rows could not be read: {error}"))
    }

    async fn control_key_rows(&self) -> Vec<PersistedControlKeyRow> {
        self.database
            .read(|transaction| {
                device_control_keys::table
                    .select((
                        device_control_keys::public_key,
                        device_control_keys::device_id,
                        device_control_keys::status,
                        device_control_keys::activated_at_unix_ms,
                        device_control_keys::retired_at_unix_ms,
                    ))
                    .order(device_control_keys::public_key)
                    .load::<PersistedControlKeyRow>(transaction.connection())
                    .map_err(|_| PersistenceError::OperationFailed)
            })
            .await
            .map_err(TransactionError::into_error)
            .unwrap_or_else(|error| panic!("persisted control-key rows could not be read: {error}"))
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
