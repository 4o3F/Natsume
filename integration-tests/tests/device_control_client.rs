use std::{fs, sync::Arc, time::Duration};

use natsume_device_daemon::control::{CONTROL_RECONNECT_MAX_SECONDS, ControlClient, ControlError};
use natsume_integration_tests::harness::{ClientFixture, TestServer, require_ok};
use tokio::{task::JoinHandle, time::timeout};
use uuid::Uuid;

const TEST_NAMESPACE: Uuid = Uuid::from_u128(0x3234_5678_1234_5678_9234_5678_1234_5678);
const EVENT_TIMEOUT: Duration = Duration::from_secs(5);
const OPERATOR_LOGIN: &str = "wp5a-admin";
const OPERATOR_PASSWORD: &str = "wp5a-operator-password";

type ControlTask = JoinHandle<Result<(), ControlError>>;

struct EnrolledScenario {
    server: TestServer,
    client: ClientFixture,
    device_id: String,
    control: Arc<ControlClient>,
}

impl EnrolledScenario {
    async fn start(label: &str) -> Self {
        let server = TestServer::start(
            env!("CARGO_BIN_EXE_server-bootstrap-driver"),
            OPERATOR_LOGIN,
            OPERATOR_PASSWORD,
        )
        .await;
        server.open_window().await;
        let machine_hardware_id = machine_id(label.as_bytes());
        let client = server.client(label, machine_hardware_id);
        client.enroll().await;
        let device_id = server.device_id_for_hardware(machine_hardware_id);
        let control = Arc::new(client.control());
        Self {
            server,
            client,
            device_id,
            control,
        }
    }
}

trait DeviceControlServerFixture {
    async fn put_lock_command(&self, command_id: Uuid, device_id: &str);
}

impl DeviceControlServerFixture for TestServer {
    async fn put_lock_command(&self, command_id: Uuid, device_id: &str) {
        self.put_command(
            command_id,
            device_id,
            "lock_session",
            serde_json::json!({
                "target": {
                    "session_instance_id": "wp5a-session",
                    "session_epoch": 7,
                },
                "requested_lock_epoch": 11,
            }),
        )
        .await;
    }
}

#[tokio::test]
async fn end_to_end_receipt_is_durable_and_matches_server_redelivery() {
    let scenario = EnrolledScenario::start("receipt").await;
    let command_id = Uuid::now_v7();
    let task = spawn_control(Arc::clone(&scenario.control));
    wait_for_hello(&scenario.control, 1).await;

    scenario
        .server
        .put_lock_command(command_id, &scenario.device_id)
        .await;
    scenario
        .server
        .wait_for_command_state(command_id, "received")
        .await;
    let journal_bytes = require_ok(
        fs::read(scenario.client.journal_frame(command_id)),
        "durable journal frame must exist",
    );
    stop_control(task).await;

    let expected_hello = scenario.control.successful_hello_count().saturating_add(1);
    let expected_delivery = scenario.control.journaled_command_count().saturating_add(1);
    let second_task = spawn_control(Arc::clone(&scenario.control));
    wait_for_hello(&scenario.control, expected_hello).await;
    wait_for_journaled_commands(&scenario.control, expected_delivery).await;
    let redelivered_journal = require_ok(
        fs::read(scenario.client.journal_frame(command_id)),
        "redelivered journal frame must remain readable",
    );
    assert_eq!(journal_bytes, redelivered_journal);
    assert_eq!(scenario.server.command_state(command_id), "received");
    stop_control(second_task).await;
    scenario.server.shutdown().await;
}

#[tokio::test]
async fn offline_command_converges_after_client_starts() {
    let scenario = EnrolledScenario::start("offline").await;
    let command_id = Uuid::now_v7();
    scenario
        .server
        .put_lock_command(command_id, &scenario.device_id)
        .await;
    assert_eq!(scenario.server.command_state(command_id), "created");

    let task = spawn_control(Arc::clone(&scenario.control));
    scenario
        .server
        .wait_for_command_state(command_id, "received")
        .await;
    assert!(scenario.client.journal_frame(command_id).is_file());
    stop_control(task).await;
    scenario.server.shutdown().await;
}

#[tokio::test]
async fn duplicate_delivery_after_reconnect_is_idempotent() {
    let scenario = EnrolledScenario::start("duplicate").await;
    let command_id = Uuid::now_v7();
    let first_task = spawn_control(Arc::clone(&scenario.control));
    wait_for_hello(&scenario.control, 1).await;
    scenario
        .server
        .put_lock_command(command_id, &scenario.device_id)
        .await;
    scenario
        .server
        .wait_for_command_state(command_id, "received")
        .await;
    wait_for_journaled_commands(&scenario.control, 1).await;
    let journal_path = scenario.client.journal_frame(command_id);
    let original = require_ok(
        fs::read(&journal_path),
        "first journal frame must be readable",
    );
    stop_control(first_task).await;

    let expected_hello = scenario.control.successful_hello_count().saturating_add(1);
    let expected_delivery = scenario.control.journaled_command_count().saturating_add(1);
    let second_task = spawn_control(Arc::clone(&scenario.control));
    wait_for_hello(&scenario.control, expected_hello).await;
    wait_for_journaled_commands(&scenario.control, expected_delivery).await;

    let after = require_ok(
        fs::read(&journal_path),
        "redelivered journal frame must be readable",
    );
    assert_eq!(after, original);
    let journal_entries = require_ok(
        fs::read_dir(scenario.client.journal_directory()),
        "journal directory must be readable",
    )
    .count();
    assert_eq!(
        journal_entries, 1,
        "duplicate delivery must create no side effect"
    );
    assert_eq!(scenario.server.command_state(command_id), "received");
    stop_control(second_task).await;
    scenario.server.shutdown().await;
}

#[tokio::test]
async fn journal_conflict_code_is_accepted_and_the_connection_survives() {
    let scenario = EnrolledScenario::start("conflict").await;
    let conflicting_command_id = Uuid::now_v7();
    require_ok(
        fs::write(
            scenario.client.journal_frame(conflicting_command_id),
            b"different previously journaled frame",
        ),
        "conflicting journal frame must be seeded",
    );
    let task = spawn_control(Arc::clone(&scenario.control));
    wait_for_hello(&scenario.control, 1).await;

    scenario
        .server
        .put_lock_command(conflicting_command_id, &scenario.device_id)
        .await;
    scenario
        .server
        .wait_for_command_state(conflicting_command_id, "failed")
        .await;

    let following_command_id = Uuid::now_v7();
    scenario
        .server
        .put_lock_command(following_command_id, &scenario.device_id)
        .await;
    scenario
        .server
        .wait_for_command_state(following_command_id, "received")
        .await;
    assert_eq!(scenario.control.successful_hello_count(), 1);

    stop_control(task).await;
    scenario.server.shutdown().await;
}

#[tokio::test]
async fn revoked_token_uses_maximum_backoff_and_retains_credentials() {
    let scenario = EnrolledScenario::start("revoked").await;
    let credentials = scenario.client.credential_snapshot();
    let task = spawn_control(Arc::clone(&scenario.control));
    wait_for_hello(&scenario.control, 1).await;
    let attempts_before_revoke = scenario.control.connection_attempt_count();

    scenario.server.revoke_device(&scenario.device_id).await;
    wait_for_attempts(&scenario.control, attempts_before_revoke.saturating_add(1)).await;
    let forbidden_attempt = scenario
        .control
        .connection_attempt_count()
        .saturating_add(1);
    assert!(
        timeout(
            Duration::from_secs(CONTROL_RECONNECT_MAX_SECONDS),
            scenario
                .control
                .wait_for_connection_attempt_count(forbidden_attempt),
        )
        .await
        .is_err(),
        "revoked credential must not produce another attempt inside the bounded window"
    );
    assert_eq!(scenario.control.successful_hello_count(), 1);
    for (path, expected) in credentials {
        let actual = require_ok(
            fs::read(&path),
            "revocation must not delete a local credential",
        );
        assert_eq!(actual, expected);
    }

    stop_control(task).await;
    scenario.server.shutdown().await;
}

fn spawn_control(control: Arc<ControlClient>) -> ControlTask {
    tokio::spawn(async move { control.run().await })
}

async fn stop_control(task: ControlTask) {
    task.abort();
    match timeout(EVENT_TIMEOUT, task).await {
        Ok(Err(error)) if error.is_cancelled() => {}
        Ok(Ok(Ok(()) | Err(_)) | Err(_)) | Err(_) => {
            panic!("Device control task must stop only by test cancellation")
        }
    }
}

async fn wait_for_hello(control: &ControlClient, minimum: u64) {
    require_ok(
        timeout(
            EVENT_TIMEOUT,
            control.wait_for_successful_hello_count(minimum),
        )
        .await,
        "Device control hello must complete within the bound",
    );
}

async fn wait_for_journaled_commands(control: &ControlClient, minimum: u64) {
    require_ok(
        timeout(
            EVENT_TIMEOUT,
            control.wait_for_journaled_command_count(minimum),
        )
        .await,
        "Device command must be journaled within the bound",
    );
}

async fn wait_for_attempts(control: &ControlClient, minimum: u64) {
    require_ok(
        timeout(
            EVENT_TIMEOUT,
            control.wait_for_connection_attempt_count(minimum),
        )
        .await,
        "Device reconnect attempt must occur within the bound",
    );
}

fn machine_id(label: &[u8]) -> Uuid {
    Uuid::new_v5(&TEST_NAMESPACE, label)
}
