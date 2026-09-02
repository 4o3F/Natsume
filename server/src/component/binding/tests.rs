use std::{fs, os::unix::fs::PermissionsExt as _, path::PathBuf, sync::Arc};

use diesel::{ExpressionMethods, QueryDsl, RunQueryDsl};
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::{
    component::device::DeviceId,
    db::{Database, DatabaseConfig, PersistenceError},
    diesel_schema::{
        account_mappings, accounts, binding_negotiations, device_bindings, devices, seats,
        server_vault_records,
    },
    vault::{self, VaultSession},
};

use super::{
    BindingComponent, BindingError, BindingEvaluationCode, BindingInput, BindingNegotiationId,
    BindingPassword, BindingSubmissionEpoch, MaterializedBinding,
};

#[tokio::test]
async fn negotiation_fences_replay_conflict_and_stale_epochs() {
    let fixture = Fixture::new().await;
    let device_id = fixture.insert_device("enabled").await;
    let negotiation_id = fixture.negotiation_id(device_id).await;

    fixture
        .ingest(device_id, negotiation_id, 2, "UNKNOWN")
        .await
        .unwrap_or_else(|error| panic!("first rejection failed: {error}"));
    fixture
        .assert_evaluation(
            device_id,
            negotiation_id,
            2,
            BindingEvaluationCode::NotFound,
        )
        .await;

    fixture
        .ingest(device_id, negotiation_id, 2, "UNKNOWN")
        .await
        .unwrap_or_else(|error| panic!("exact replay failed: {error}"));
    assert_eq!(
        fixture
            .ingest(device_id, negotiation_id, 2, "DIFFERENT")
            .await,
        Err(BindingError::ConflictingSubmission)
    );
    fixture
        .ingest(device_id, negotiation_id, 1, "STALE")
        .await
        .unwrap_or_else(|error| panic!("stale epoch was not ignored: {error}"));
    fixture
        .assert_evaluation(
            device_id,
            negotiation_id,
            2,
            BindingEvaluationCode::NotFound,
        )
        .await;

    fixture
        .ingest(device_id, BindingNegotiationId::new(), 3, "STALE-ID")
        .await
        .unwrap_or_else(|error| panic!("stale negotiation was not ignored: {error}"));
    fixture
        .assert_evaluation(
            device_id,
            negotiation_id,
            2,
            BindingEvaluationCode::NotFound,
        )
        .await;
    assert_eq!(fixture.negotiation_count(device_id).await, 1);
}

#[tokio::test]
async fn accepted_binding_materializes_one_redacted_current_credential() {
    let fixture = Fixture::new().await;
    let device_id = fixture.insert_device("enabled").await;
    let (account_id, _seat_id) = fixture
        .insert_mapped_seat("A-01", "team-alpha", b"password-canary")
        .await;
    let negotiation_id = fixture.negotiation_id(device_id).await;

    fixture
        .ingest(device_id, negotiation_id, 1, "A-01")
        .await
        .unwrap_or_else(|error| panic!("valid binding failed: {error}"));
    let bound = fixture.bound(device_id).await;
    assert_eq!(bound.context().account_id(), account_id);
    assert_eq!(bound.context().seat_code(), "A-01");
    assert_eq!(bound.context().domjudge_username(), "team-alpha");
    assert_eq!(bound.context().credential_revision(), 1);
    assert_eq!(bound.password().as_bytes(), b"password-canary");
    let debug = format!("{bound:?}");
    assert!(debug.contains("BindingPassword([REDACTED])"));
    assert!(!debug.contains("password-canary"));
    assert_eq!(fixture.binding_count(device_id).await, 1);
    assert_eq!(fixture.negotiation_count(device_id).await, 0);
}

#[tokio::test]
async fn rejection_vocabulary_distinguishes_unmapped_and_occupied_seats() {
    let fixture = Fixture::new().await;
    let first_device = fixture.insert_device("enabled").await;
    let second_device = fixture.insert_device("enabled").await;
    fixture.insert_unmapped_seat("U-01").await;
    fixture
        .insert_mapped_seat("A-01", "team-alpha", b"password")
        .await;

    let first_negotiation = fixture.negotiation_id(first_device).await;
    fixture
        .ingest(first_device, first_negotiation, 1, "U-01")
        .await
        .unwrap_or_else(|error| panic!("unmapped rejection failed: {error}"));
    fixture
        .assert_evaluation(
            first_device,
            first_negotiation,
            1,
            BindingEvaluationCode::Unmapped,
        )
        .await;

    fixture
        .ingest(first_device, first_negotiation, 2, "A-01")
        .await
        .unwrap_or_else(|error| panic!("first occupancy failed: {error}"));
    let second_negotiation = fixture.negotiation_id(second_device).await;
    fixture
        .ingest(second_device, second_negotiation, 1, "A-01")
        .await
        .unwrap_or_else(|error| panic!("occupied rejection failed: {error}"));
    fixture
        .assert_evaluation(
            second_device,
            second_negotiation,
            1,
            BindingEvaluationCode::Occupied,
        )
        .await;
}

#[tokio::test]
async fn concurrent_acceptance_preserves_seat_and_device_occupancy() {
    let fixture = Fixture::new().await;
    let first_device = fixture.insert_device("enabled").await;
    let second_device = fixture.insert_device("enabled").await;
    fixture
        .insert_mapped_seat("A-01", "team-alpha", b"password")
        .await;
    let first_negotiation = fixture.negotiation_id(first_device).await;
    let second_negotiation = fixture.negotiation_id(second_device).await;

    let first = fixture
        .component
        .ingest(first_device, Some(input(first_negotiation, 1, "A-01")));
    let second = fixture
        .component
        .ingest(second_device, Some(input(second_negotiation, 1, "A-01")));
    let (first, second) = tokio::join!(first, second);
    assert_eq!(first, Ok(()));
    assert_eq!(second, Ok(()));

    let first = fixture.materialize(first_device).await;
    let second = fixture.materialize(second_device).await;
    let bound_count = usize::from(first.target().bound().is_some())
        + usize::from(second.target().bound().is_some());
    assert_eq!(bound_count, 1);
    let rejected = if first.target().bound().is_none() {
        first
    } else {
        second
    };
    assert_eq!(
        rejected
            .intent()
            .and_then(|intent| intent.evaluation())
            .map(|evaluation| evaluation.error_code()),
        Some(BindingEvaluationCode::Occupied)
    );
    assert_eq!(fixture.total_binding_count().await, 1);

    let third_device = fixture.insert_device("enabled").await;
    fixture
        .insert_mapped_seat("A-02", "team-beta", b"second-password")
        .await;
    fixture
        .insert_mapped_seat("A-03", "team-gamma", b"third-password")
        .await;
    let third_negotiation = fixture.negotiation_id(third_device).await;
    let first_seat = fixture
        .component
        .ingest(third_device, Some(input(third_negotiation, 1, "A-02")));
    let second_seat = fixture
        .component
        .ingest(third_device, Some(input(third_negotiation, 1, "A-03")));
    let (first_seat, second_seat) = tokio::join!(first_seat, second_seat);
    assert_eq!(first_seat, Ok(()));
    assert_eq!(second_seat, Ok(()));
    assert_eq!(fixture.binding_count(third_device).await, 1);
    assert_eq!(fixture.negotiation_count(third_device).await, 0);
    assert_eq!(fixture.total_binding_count().await, 2);
}

#[tokio::test]
async fn explicit_unbind_releases_occupancy_and_creates_one_new_negotiation() {
    let fixture = Fixture::new().await;
    let device_id = fixture.insert_device("enabled").await;
    fixture
        .insert_mapped_seat("A-01", "team-alpha", b"password")
        .await;
    let accepted_negotiation = fixture.negotiation_id(device_id).await;
    fixture
        .ingest(device_id, accepted_negotiation, 1, "A-01")
        .await
        .unwrap_or_else(|error| panic!("binding setup failed: {error}"));

    fixture
        .component
        .unbind(device_id)
        .await
        .unwrap_or_else(|error| panic!("unbind failed: {error}"));
    let replacement = fixture.negotiation_id(device_id).await;
    assert_ne!(replacement, accepted_negotiation);
    assert_eq!(fixture.binding_count(device_id).await, 0);

    fixture
        .component
        .unbind(device_id)
        .await
        .unwrap_or_else(|error| panic!("idempotent unbind failed: {error}"));
    assert_eq!(fixture.negotiation_id(device_id).await, replacement);
    assert_eq!(fixture.negotiation_count(device_id).await, 1);
}

#[tokio::test]
async fn ineligible_devices_and_invalid_persisted_evaluations_fail_closed() {
    let fixture = Fixture::new().await;
    let disabled = fixture.insert_device("disabled").await;
    assert!(matches!(
        fixture.component.materialize(disabled).await,
        Err(BindingError::DeviceNotEligible)
    ));
    assert_eq!(fixture.negotiation_count(disabled).await, 0);

    let enabled = fixture.insert_device("enabled").await;
    let negotiation_id = Uuid::now_v7();
    let device_id = enabled.as_text();
    fixture
        .database
        .write(move |transaction| {
            diesel::insert_into(binding_negotiations::table)
                .values((
                    binding_negotiations::device_id.eq(device_id),
                    binding_negotiations::negotiation_id
                        .eq(negotiation_id.hyphenated().to_string()),
                    binding_negotiations::submission_epoch.eq(Some(1_i64)),
                    binding_negotiations::seat_code.eq(Some("A-01")),
                    binding_negotiations::evaluation_error_code.eq(Some("SEAT_UNKNOWN")),
                ))
                .execute(transaction.connection())
                .map_err(|_| PersistenceError::OperationFailed)?;
            Ok::<(), PersistenceError>(())
        })
        .await
        .unwrap_or_else(|error| panic!("invalid evaluation fixture failed: {error:?}"));
    assert!(matches!(
        fixture.component.materialize(enabled).await,
        Err(BindingError::InvalidPersistedFacts)
    ));

    for invalid_password in [
        Vec::new(),
        vec![0xff],
        b"control\ncharacter".to_vec(),
        vec![b'x'; 513],
    ] {
        assert!(BindingPassword::new(Zeroizing::new(invalid_password)).is_err());
    }
}

fn input(negotiation_id: BindingNegotiationId, epoch: u64, seat_code: &str) -> BindingInput {
    BindingInput::new(
        negotiation_id,
        BindingSubmissionEpoch::new(epoch)
            .unwrap_or_else(|| panic!("test epoch is outside the persisted range")),
        seat_code.to_owned(),
    )
}

struct Fixture {
    root: PathBuf,
    database: Database,
    vault: Arc<VaultSession>,
    component: BindingComponent,
}

impl Fixture {
    async fn new() -> Self {
        let root = std::env::temp_dir().join(format!("natsume-binding-{}", Uuid::now_v7()));
        fs::create_dir(&root)
            .unwrap_or_else(|_| panic!("Binding fixture directory could not be created"));
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
            .unwrap_or_else(|_| panic!("Binding fixture permissions could not be set"));
        let key_path = root.join("master.key");
        vault::ensure_master_key(&key_path)
            .unwrap_or_else(|_| panic!("Binding fixture vault key could not be created"));
        let vault = Arc::new(
            vault::load(&key_path)
                .unwrap_or_else(|_| panic!("Binding fixture vault could not be loaded")),
        );
        let database =
            Database::connect_and_migrate(&DatabaseConfig::new(root.join("server.sqlite3"), true))
                .await
                .unwrap_or_else(|error| panic!("Binding fixture database failed: {error:?}"));
        let component = BindingComponent::new(database.clone(), Arc::clone(&vault));
        Self {
            root,
            database,
            vault,
            component,
        }
    }

    async fn insert_device(&self, state: &'static str) -> DeviceId {
        let device_id = Uuid::now_v7();
        let machine_hardware_id = Uuid::new_v5(&Uuid::NAMESPACE_OID, device_id.as_bytes());
        let device_id_text = device_id.hyphenated().to_string();
        let typed_device_id = DeviceId::parse(&device_id_text)
            .unwrap_or_else(|| panic!("Binding fixture generated an invalid Device ID"));
        self.database
            .write(move |transaction| {
                diesel::insert_into(devices::table)
                    .values((
                        devices::device_id.eq(&device_id_text),
                        devices::machine_hardware_id
                            .eq(machine_hardware_id.hyphenated().to_string()),
                        devices::evidence_quality.eq("strong"),
                        devices::state.eq(state),
                        devices::created_at_unix_ms.eq(1_i64),
                    ))
                    .execute(transaction.connection())
                    .map_err(|_| PersistenceError::OperationFailed)?;
                Ok::<(), PersistenceError>(())
            })
            .await
            .unwrap_or_else(|error| panic!("Binding Device fixture failed: {error:?}"));
        typed_device_id
    }

    async fn insert_unmapped_seat(&self, seat_code: &'static str) -> Uuid {
        let seat_id = Uuid::now_v7();
        self.insert_seat(seat_id, seat_code).await;
        seat_id
    }

    async fn insert_mapped_seat(
        &self,
        seat_code: &'static str,
        domjudge_username: &'static str,
        password: &[u8],
    ) -> (Uuid, Uuid) {
        let seat_id = Uuid::now_v7();
        let account_id = Uuid::now_v7();
        let (nonce, ciphertext) = self
            .vault
            .seal(password)
            .unwrap_or_else(|_| panic!("Binding fixture credential could not be sealed"));
        let seat_id_text = seat_id.hyphenated().to_string();
        let account_id_text = account_id.hyphenated().to_string();
        self.database
            .write(move |transaction| {
                diesel::insert_into(seats::table)
                    .values((
                        seats::seat_id.eq(&seat_id_text),
                        seats::seat_code.eq(seat_code),
                    ))
                    .execute(transaction.connection())
                    .map_err(|_| PersistenceError::OperationFailed)?;
                diesel::insert_into(accounts::table)
                    .values((
                        accounts::account_id.eq(&account_id_text),
                        accounts::domjudge_username.eq(domjudge_username),
                        accounts::credential_revision.eq(1_i64),
                    ))
                    .execute(transaction.connection())
                    .map_err(|_| PersistenceError::OperationFailed)?;
                diesel::insert_into(server_vault_records::table)
                    .values((
                        server_vault_records::account_id.eq(&account_id_text),
                        server_vault_records::nonce.eq(nonce.as_slice()),
                        server_vault_records::ciphertext.eq(ciphertext),
                    ))
                    .execute(transaction.connection())
                    .map_err(|_| PersistenceError::OperationFailed)?;
                diesel::insert_into(account_mappings::table)
                    .values((
                        account_mappings::seat_id.eq(seat_id_text),
                        account_mappings::account_id.eq(account_id_text),
                    ))
                    .execute(transaction.connection())
                    .map_err(|_| PersistenceError::OperationFailed)?;
                Ok::<(), PersistenceError>(())
            })
            .await
            .unwrap_or_else(|error| panic!("Binding Seat fixture failed: {error:?}"));
        (account_id, seat_id)
    }

    async fn insert_seat(&self, seat_id: Uuid, seat_code: &'static str) {
        self.database
            .write(move |transaction| {
                diesel::insert_into(seats::table)
                    .values((
                        seats::seat_id.eq(seat_id.hyphenated().to_string()),
                        seats::seat_code.eq(seat_code),
                    ))
                    .execute(transaction.connection())
                    .map_err(|_| PersistenceError::OperationFailed)?;
                Ok::<(), PersistenceError>(())
            })
            .await
            .unwrap_or_else(|error| panic!("unmapped Seat fixture failed: {error:?}"));
    }

    async fn ingest(
        &self,
        device_id: DeviceId,
        negotiation_id: BindingNegotiationId,
        epoch: u64,
        seat_code: &str,
    ) -> Result<(), BindingError> {
        self.component
            .ingest(device_id, Some(input(negotiation_id, epoch, seat_code)))
            .await
    }

    async fn materialize(&self, device_id: DeviceId) -> MaterializedBinding {
        self.component
            .materialize(device_id)
            .await
            .unwrap_or_else(|error| panic!("Binding materialization failed: {error}"))
    }

    async fn negotiation_id(&self, device_id: DeviceId) -> BindingNegotiationId {
        self.materialize(device_id)
            .await
            .intent()
            .unwrap_or_else(|| panic!("unbound Device has no Binding intent"))
            .negotiation_id()
    }

    async fn bound(&self, device_id: DeviceId) -> super::BoundTarget {
        let materialized = self.materialize(device_id).await;
        materialized
            .target
            .bound
            .unwrap_or_else(|| panic!("Device did not materialize a bound target"))
    }

    async fn assert_evaluation(
        &self,
        device_id: DeviceId,
        negotiation_id: BindingNegotiationId,
        epoch: u64,
        code: BindingEvaluationCode,
    ) {
        let materialized = self.materialize(device_id).await;
        let intent = materialized
            .intent()
            .unwrap_or_else(|| panic!("rejected Device lost its Binding intent"));
        let evaluation = intent
            .evaluation()
            .unwrap_or_else(|| panic!("rejected submission has no evaluation"));
        assert_eq!(intent.negotiation_id(), negotiation_id);
        assert_eq!(evaluation.submission_epoch().as_u64(), epoch);
        assert_eq!(evaluation.error_code(), code);
    }

    async fn negotiation_count(&self, device_id: DeviceId) -> i64 {
        let device_id = device_id.as_text();
        self.database
            .read(move |transaction| {
                binding_negotiations::table
                    .filter(binding_negotiations::device_id.eq(device_id))
                    .count()
                    .get_result::<i64>(transaction.connection())
                    .map_err(|_| PersistenceError::OperationFailed)
            })
            .await
            .unwrap_or_else(|error| panic!("negotiation count failed: {error:?}"))
    }

    async fn binding_count(&self, device_id: DeviceId) -> i64 {
        let device_id = device_id.as_text();
        self.database
            .read(move |transaction| {
                device_bindings::table
                    .filter(device_bindings::device_id.eq(device_id))
                    .count()
                    .get_result::<i64>(transaction.connection())
                    .map_err(|_| PersistenceError::OperationFailed)
            })
            .await
            .unwrap_or_else(|error| panic!("Device binding count failed: {error:?}"))
    }

    async fn total_binding_count(&self) -> i64 {
        self.database
            .read(move |transaction| {
                device_bindings::table
                    .count()
                    .get_result::<i64>(transaction.connection())
                    .map_err(|_| PersistenceError::OperationFailed)
            })
            .await
            .unwrap_or_else(|error| panic!("binding count failed: {error:?}"))
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
