use natsume_machine_identity::{
    IdentityRecordState, LocalIdentityPreflightDecision, MachineIdentityDecision,
    StartupIdentityDecision, evaluate_local_identity_preflight, evaluate_startup_identity,
};
use uuid::Uuid;

fn derived_identity(id: u128) -> MachineIdentityDecision {
    MachineIdentityDecision::Derived {
        machine_hardware_id: Uuid::from_u128(id),
        present_slot_count: 3,
    }
}

#[test]
fn copied_configured_state_on_different_hardware_uses_standard_reset_path() {
    assert_eq!(
        evaluate_startup_identity(Some(Uuid::from_u128(1)), &derived_identity(2)),
        StartupIdentityDecision::ResetRequired {
            stored: Uuid::from_u128(1),
            recomputed_machine_hardware_id: Uuid::from_u128(2),
        },
    );
}

#[test]
fn temporary_hardware_collection_failure_never_deletes_state() {
    let decision = MachineIdentityDecision::InsufficientSources {
        present_slot_count: 1,
    };
    assert_eq!(
        evaluate_startup_identity(Some(Uuid::from_u128(1)), &decision),
        StartupIdentityDecision::Indeterminate,
    );
}

#[test]
fn missing_identity_record_with_existing_vault_fails_closed() {
    assert_eq!(
        evaluate_local_identity_preflight(Uuid::from_u128(10), IdentityRecordState::Absent, true,),
        LocalIdentityPreflightDecision::IdentityRecordMissingWithState,
    );
}

#[test]
fn deployment_namespace_change_is_not_a_new_device_signal() {
    assert_eq!(
        evaluate_local_identity_preflight(
            Uuid::from_u128(10),
            IdentityRecordState::Valid {
                fleet_namespace_uuid: Uuid::from_u128(11),
                machine_hardware_id: Uuid::from_u128(1),
            },
            true,
        ),
        LocalIdentityPreflightDecision::SiteNamespaceMismatch {
            configured_namespace: Uuid::from_u128(10),
            stored_namespace: Uuid::from_u128(11),
        },
    );
}
