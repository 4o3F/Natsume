use natsume_local_control_api::{SessionTarget, UnlockSessionRequest};

#[test]
fn unlock_payload_freezes_the_exact_session_epoch() {
    let request = UnlockSessionRequest {
        command_id: "018f0e2e-8c1d-7c5e-8b12-3456789abcde".to_owned(),
        target: SessionTarget {
            session_instance_id: "session-a".to_owned(),
            session_epoch: 42,
        },
        expected_lock_epoch: 7,
        expected_lock_command_id: "018f0e2e-8c1d-7c5e-9c23-456789abcdef".to_owned(),
        deadline_unix_ms: 1_800_000_000_000,
    };

    assert_eq!(request.target.session_epoch, 42);
    assert_eq!(request.expected_lock_epoch, 7);
    assert_eq!(
        request.expected_lock_command_id,
        "018f0e2e-8c1d-7c5e-9c23-456789abcdef"
    );
}
