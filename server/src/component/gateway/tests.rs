use std::{
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use diesel::{ExpressionMethods, QueryDsl, RunQueryDsl};
use rcgen::{CertificateParams, KeyPair};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::{
    component::device::DeviceId,
    db::{Database, DatabaseConfig, PersistenceError},
    diesel_schema::{devices, gateway_credentials},
};

use super::{
    GatewayActualState, GatewayComponent, GatewayCredentialId, GatewayCredentialInput,
    GatewayError, MaterializedGateway, issuer::GatewayIssuer,
};

const DEVICE_ID: &str = "01900000-0000-7000-8000-000000000081";
const MACHINE_HARDWARE_ID: &str = "a9aa9d04-3ece-5567-8260-910930ff5e03";

type PersistedGatewayRow = (String, Option<Vec<u8>>, Option<Vec<u8>>, Option<Vec<u8>>);

#[tokio::test]
async fn first_materialization_creates_one_waiting_intent() {
    let fixture = Fixture::new().await;

    let materialized = fixture.materialize().await;
    let credential_id = materialized.intent().credential_id();

    assert_eq!(materialized.target().credential_id(), credential_id);
    assert!(materialized.target().certificate().is_none());
    assert_eq!(
        fixture.gateway_row().await,
        (credential_id.as_text(), None, None, None,)
    );
    assert_eq!(fixture.issue_count.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn accepted_csr_is_issued_once_and_replayed_exactly_after_component_rebuild() {
    let fixture = Fixture::new().await;
    let credential_id = fixture.current_credential_id().await;
    let csr_der = valid_csr();

    fixture
        .component
        .ingest(
            fixture.device_id,
            Some(GatewayCredentialInput::new(
                credential_id,
                Some(csr_der.clone()),
            )),
            GatewayActualState::Absent,
        )
        .await
        .unwrap_or_else(|error| panic!("valid CSR ingestion failed: {error}"));
    let signing_component = fixture.rebuilt_component();
    let issued = signing_component
        .materialize(fixture.device_id)
        .await
        .unwrap_or_else(|error| panic!("post-rebuild issuance failed: {error}"));
    let issued_leaf = issued
        .target()
        .certificate()
        .unwrap_or_else(|| panic!("accepted CSR did not produce a certificate"))
        .leaf_der()
        .to_vec();
    assert_eq!(fixture.issue_count.load(Ordering::Relaxed), 1);

    let rebuilt = fixture.rebuilt_component();
    let replayed = rebuilt
        .materialize(fixture.device_id)
        .await
        .unwrap_or_else(|error| panic!("durable grant replay failed: {error}"));
    let replayed_grant = replayed
        .target()
        .certificate()
        .unwrap_or_else(|| panic!("rebuilt component did not replay the grant"));

    assert_eq!(replayed, issued);
    assert_eq!(replayed_grant.leaf_der(), issued_leaf);
    assert!(replayed_grant.issuer_chain_der().is_empty());
    assert_eq!(fixture.issue_count.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn concurrent_materialization_returns_the_single_committed_grant() {
    let fixture = Fixture::new().await;
    let credential_id = fixture.current_credential_id().await;
    fixture
        .component
        .ingest(
            fixture.device_id,
            Some(GatewayCredentialInput::new(
                credential_id,
                Some(valid_csr()),
            )),
            GatewayActualState::Absent,
        )
        .await
        .unwrap_or_else(|error| panic!("valid CSR ingestion failed: {error}"));

    let first = fixture.rebuilt_component();
    let second = fixture.rebuilt_component();
    let (first, second) = tokio::join!(
        first.materialize(fixture.device_id),
        second.materialize(fixture.device_id)
    );

    assert_eq!(first, second);
    assert!(
        first
            .unwrap_or_else(|error| panic!("concurrent materialization failed: {error}"))
            .target()
            .certificate()
            .is_some()
    );
}

#[tokio::test]
async fn same_csr_replays_but_a_different_valid_csr_conflicts() {
    let fixture = Fixture::new().await;
    let credential_id = fixture.current_credential_id().await;
    let accepted_csr = valid_csr();

    for csr_der in [accepted_csr.clone(), accepted_csr.clone()] {
        fixture
            .component
            .ingest(
                fixture.device_id,
                Some(GatewayCredentialInput::new(credential_id, Some(csr_der))),
                GatewayActualState::Absent,
            )
            .await
            .unwrap_or_else(|error| panic!("exact CSR replay failed: {error}"));
    }

    assert_eq!(
        fixture
            .component
            .ingest(
                fixture.device_id,
                Some(GatewayCredentialInput::new(
                    credential_id,
                    Some(valid_csr()),
                )),
                GatewayActualState::Absent,
            )
            .await,
        Err(GatewayError::ConflictingCsr)
    );
    assert_eq!(fixture.gateway_row().await.1, Some(accepted_csr));
    assert_eq!(fixture.issue_count.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn stale_input_and_recovery_actual_do_not_modify_the_current_generation() {
    let fixture = Fixture::new().await;
    let current_id = fixture.current_credential_id().await;
    let stale_id = GatewayCredentialId::new();

    fixture
        .component
        .ingest(
            fixture.device_id,
            Some(GatewayCredentialInput::new(stale_id, Some(valid_csr()))),
            GatewayActualState::RecoveryRequired {
                credential_id: stale_id,
            },
        )
        .await
        .unwrap_or_else(|error| panic!("stale input was not ignored: {error}"));

    assert_eq!(
        fixture.gateway_row().await,
        (current_id.as_text(), None, None, None)
    );
}

#[tokio::test]
async fn missing_current_csr_recovery_and_loaded_leaf_mismatch_each_replace_generation() {
    let fixture = Fixture::new().await;
    let first_id = fixture.current_credential_id().await;

    fixture
        .component
        .ingest(
            fixture.device_id,
            Some(GatewayCredentialInput::new(first_id, None)),
            GatewayActualState::Absent,
        )
        .await
        .unwrap_or_else(|error| panic!("missing current CSR replacement failed: {error}"));
    let second_id = fixture.current_credential_id().await;
    assert_ne!(second_id, first_id);
    fixture.assert_waiting_generation(second_id).await;

    fixture
        .component
        .ingest(
            fixture.device_id,
            None,
            GatewayActualState::RecoveryRequired {
                credential_id: second_id,
            },
        )
        .await
        .unwrap_or_else(|error| panic!("recovery replacement failed: {error}"));
    let third_id = fixture.current_credential_id().await;
    assert_ne!(third_id, second_id);
    fixture.assert_waiting_generation(third_id).await;

    fixture
        .component
        .ingest(
            fixture.device_id,
            Some(GatewayCredentialInput::new(third_id, Some(valid_csr()))),
            GatewayActualState::Absent,
        )
        .await
        .unwrap_or_else(|error| panic!("CSR fixture ingestion failed: {error}"));
    assert!(fixture.materialize().await.target().certificate().is_some());

    fixture
        .component
        .ingest(
            fixture.device_id,
            None,
            GatewayActualState::Loaded {
                credential_id: third_id,
                leaf_sha256: [0_u8; 32],
            },
        )
        .await
        .unwrap_or_else(|error| panic!("loaded leaf mismatch replacement failed: {error}"));
    let fourth_id = fixture.current_credential_id().await;
    assert_ne!(fourth_id, third_id);
    fixture.assert_waiting_generation(fourth_id).await;
}

#[tokio::test]
async fn invalid_persisted_presence_and_der_fail_closed() {
    let invalid_presence = Fixture::new().await;
    invalid_presence
        .insert_gateway_row(None, Some(vec![0x01]), Some(Vec::new()))
        .await;
    assert_eq!(
        invalid_presence
            .component
            .materialize(invalid_presence.device_id)
            .await,
        Err(GatewayError::InvalidPersistedFacts)
    );

    let invalid_csr = Fixture::new().await;
    invalid_csr
        .insert_gateway_row(Some(vec![0x30, 0x00]), None, None)
        .await;
    assert_eq!(
        invalid_csr
            .component
            .materialize(invalid_csr.device_id)
            .await,
        Err(GatewayError::InvalidPersistedFacts)
    );

    let invalid_leaf = Fixture::new().await;
    invalid_leaf
        .insert_gateway_row(Some(valid_csr()), Some(vec![0x30, 0x00]), Some(Vec::new()))
        .await;
    assert_eq!(
        invalid_leaf
            .component
            .materialize(invalid_leaf.device_id)
            .await,
        Err(GatewayError::InvalidPersistedFacts)
    );
}

#[tokio::test]
async fn valid_but_expired_persisted_grant_replaces_the_generation() {
    let fixture = Fixture::new().await;
    let expired_id = fixture
        .insert_gateway_row(
            Some(valid_csr()),
            Some(expired_leaf_der()),
            Some(Vec::new()),
        )
        .await;

    let materialized = fixture.materialize().await;
    let replacement_id = materialized.intent().credential_id();

    assert_ne!(replacement_id, expired_id);
    assert!(materialized.target().certificate().is_none());
    fixture.assert_waiting_generation(replacement_id).await;
}

fn valid_csr() -> Vec<u8> {
    let key = KeyPair::generate().unwrap_or_else(|_| panic!("test CSR key generation failed"));
    CertificateParams::default()
        .serialize_request(&key)
        .map_or_else(
            |_| panic!("test CSR generation failed"),
            |request| request.der().as_ref().to_vec(),
        )
}

fn expired_leaf_der() -> Vec<u8> {
    let key = KeyPair::generate().unwrap_or_else(|_| panic!("expired leaf key generation failed"));
    let mut params = CertificateParams::default();
    params.not_before = OffsetDateTime::UNIX_EPOCH;
    params.not_after = OffsetDateTime::UNIX_EPOCH + Duration::days(1);
    params
        .self_signed(&key)
        .unwrap_or_else(|_| panic!("expired leaf generation failed"))
        .der()
        .as_ref()
        .to_vec()
}

struct Fixture {
    path: PathBuf,
    database: Database,
    issuer: Arc<GatewayIssuer>,
    issue_count: Arc<AtomicUsize>,
    component: GatewayComponent,
    device_id: DeviceId,
}

impl Fixture {
    async fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "natsume-gateway-component-test-{}.sqlite3",
            Uuid::now_v7()
        ));
        let database = Database::connect_and_migrate(&DatabaseConfig::new(&path, true))
            .await
            .unwrap_or_else(|error| panic!("test database creation failed: {error:?}"));
        let device_id = DeviceId::parse(DEVICE_ID)
            .unwrap_or_else(|| panic!("Gateway fixture Device ID is invalid"));
        let device_id_text = device_id.as_text();
        database
            .write(move |transaction| {
                diesel::insert_into(devices::table)
                    .values((
                        devices::device_id.eq(device_id_text),
                        devices::machine_hardware_id.eq(MACHINE_HARDWARE_ID),
                        devices::evidence_quality.eq("strong"),
                        devices::state.eq("enabled"),
                        devices::created_at_unix_ms.eq(1_i64),
                    ))
                    .execute(transaction.connection())
                    .map_err(|_| PersistenceError::OperationFailed)?;
                Ok::<(), PersistenceError>(())
            })
            .await
            .unwrap_or_else(|error| panic!("Gateway fixture Device insertion failed: {error:?}"));
        let (issuer, issue_count) = GatewayIssuer::for_test()
            .unwrap_or_else(|error| panic!("test Gateway issuer creation failed: {error}"));
        let issuer = Arc::new(issuer);
        let component = GatewayComponent::new(database.clone(), Arc::clone(&issuer));
        Self {
            path,
            database,
            issuer,
            issue_count,
            component,
            device_id,
        }
    }

    fn rebuilt_component(&self) -> GatewayComponent {
        GatewayComponent::new(self.database.clone(), Arc::clone(&self.issuer))
    }

    async fn materialize(&self) -> MaterializedGateway {
        self.component
            .materialize(self.device_id)
            .await
            .unwrap_or_else(|error| panic!("Gateway materialization failed: {error}"))
    }

    async fn current_credential_id(&self) -> GatewayCredentialId {
        self.materialize().await.intent().credential_id()
    }

    async fn assert_waiting_generation(&self, credential_id: GatewayCredentialId) {
        assert_eq!(
            self.gateway_row().await,
            (credential_id.as_text(), None, None, None)
        );
    }

    async fn gateway_row(&self) -> PersistedGatewayRow {
        let device_id = self.device_id.as_text();
        self.database
            .read(move |transaction| {
                gateway_credentials::table
                    .select((
                        gateway_credentials::credential_id,
                        gateway_credentials::gateway_csr_der,
                        gateway_credentials::gateway_leaf_der,
                        gateway_credentials::issuer_chain_der,
                    ))
                    .filter(gateway_credentials::device_id.eq(device_id))
                    .first::<PersistedGatewayRow>(transaction.connection())
                    .map_err(|_| PersistenceError::OperationFailed)
            })
            .await
            .unwrap_or_else(|error| panic!("Gateway row read failed: {error:?}"))
    }

    async fn insert_gateway_row(
        &self,
        csr_der: Option<Vec<u8>>,
        leaf_der: Option<Vec<u8>>,
        issuer_chain_der: Option<Vec<u8>>,
    ) -> GatewayCredentialId {
        let device_id = self.device_id.as_text();
        let credential_id = GatewayCredentialId::new();
        let credential_id_text = credential_id.as_text();
        self.database
            .write(move |transaction| {
                diesel::insert_into(gateway_credentials::table)
                    .values((
                        gateway_credentials::device_id.eq(device_id),
                        gateway_credentials::credential_id.eq(credential_id_text),
                        gateway_credentials::gateway_csr_der.eq(csr_der),
                        gateway_credentials::gateway_leaf_der.eq(leaf_der),
                        gateway_credentials::issuer_chain_der.eq(issuer_chain_der),
                    ))
                    .execute(transaction.connection())
                    .map_err(|_| PersistenceError::OperationFailed)?;
                Ok::<(), PersistenceError>(())
            })
            .await
            .unwrap_or_else(|error| panic!("Gateway fixture row insertion failed: {error:?}"));
        credential_id
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        let _ = fs::remove_file(PathBuf::from(format!("{}-wal", self.path.display())));
        let _ = fs::remove_file(PathBuf::from(format!("{}-shm", self.path.display())));
    }
}
