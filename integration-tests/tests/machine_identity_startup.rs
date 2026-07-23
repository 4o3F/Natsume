use natsume_machine_identity::{
    CollectionCompleteness, EvidenceQuality, HardwareCandidate, HardwareClaim, IdentityRecordState,
    LocalIdentityPreflightDecision, StartupIdentityDecision, evaluate_local_identity_preflight,
    evaluate_startup_identity,
};
use uuid::Uuid;

fn complete_claim(id: u128) -> HardwareClaim {
    HardwareClaim {
        candidates: vec![HardwareCandidate {
            anchor_kind: "system_uuid".to_owned(),
            candidate_id: Uuid::from_u128(id),
            quality: EvidenceQuality::Strong,
        }],
        completeness: CollectionCompleteness::Complete,
    }
}

#[test]
fn copied_configured_state_on_different_hardware_uses_standard_reset_path() {
    assert_eq!(
        evaluate_startup_identity(Some(Uuid::from_u128(1)), &complete_claim(2)),
        StartupIdentityDecision::ResetRequired {
            stored: Uuid::from_u128(1),
            selected_current: Uuid::from_u128(2),
        },
    );
}

#[test]
fn temporary_hardware_collection_failure_never_deletes_state() {
    let claim = HardwareClaim {
        candidates: Vec::new(),
        completeness: CollectionCompleteness::TemporarilyUnavailable,
    };
    assert_eq!(
        evaluate_startup_identity(Some(Uuid::from_u128(1)), &claim),
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
