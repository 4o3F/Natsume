//! Whole-machine identity composition and claim decisions.
//!
//! One `unsupported` slot makes the platform unsupported. Otherwise, any two present slots can
//! derive the identity. Every non-present slot occupies its frozen [`ANCHOR_ORDER`] position with
//! the single-byte `0x01` marker.

use uuid::Uuid;

use super::{ANCHOR_ORDER, EvidenceStatus, SlotEvaluation};

const MISSING_SLOT_MARKER: u8 = 0x01;

/// The closed result of applying the whole-machine 2-of-3 claim policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::hardware_identity) enum MachineIdentityDecision {
    /// At least two slots were present and the collection contained no unsupported slot.
    Derived {
        /// The whole-machine `UUIDv5` derived from all three frozen slot positions.
        machine_hardware_id: Uuid,
    },
    /// Fewer than two slots were present and none was unsupported.
    InsufficientSources,
    /// At least one slot is unsupported on this platform.
    Unsupported,
}

fn whole_machine_name_bytes(evaluations: &[SlotEvaluation; 3]) -> Vec<u8> {
    let mut name = Vec::new();

    for (anchor, evaluation) in ANCHOR_ORDER.iter().zip(evaluations) {
        name.extend_from_slice(anchor.label().as_bytes());
        name.push(0);
        if evaluation.status == EvidenceStatus::Present {
            if let Some(normalized_value) = evaluation.normalized_value.as_deref() {
                name.extend_from_slice(normalized_value.as_bytes());
            } else {
                // `evaluate_slot` makes this branch unreachable, but the marker keeps this
                // function total if a crate-internal test constructs an inconsistent value.
                name.push(MISSING_SLOT_MARKER);
            }
        } else {
            name.push(MISSING_SLOT_MARKER);
        }
        name.push(0);
    }

    name
}

/// Applies the terminal unsupported rule, the 2-of-3 claim decision, and the frozen whole-machine
/// byte recipe.
///
/// The evaluations must occupy [`ANCHOR_ORDER`] positions and must have been produced with the
/// same immutable Fleet namespace. The namespace retained by the first evaluation is the same
/// namespace used by the whole-machine `UUIDv5` derivation.
#[must_use]
pub(in crate::hardware_identity) fn decide_machine_identity(
    evaluations: &[SlotEvaluation; 3],
) -> MachineIdentityDecision {
    let statuses = evaluations.each_ref().map(|evaluation| evaluation.status);
    let present_slot_count = statuses
        .iter()
        .filter(|status| **status == EvidenceStatus::Present)
        .count();

    if statuses.contains(&EvidenceStatus::Unsupported) {
        return MachineIdentityDecision::Unsupported;
    }

    if present_slot_count < 2 {
        return MachineIdentityDecision::InsufficientSources;
    }

    let namespace = evaluations[0].fleet_namespace;
    debug_assert!(
        evaluations
            .iter()
            .all(|evaluation| evaluation.fleet_namespace == namespace),
        "all slot evaluations must use one immutable Fleet namespace"
    );
    let name = whole_machine_name_bytes(evaluations);

    MachineIdentityDecision::Derived {
        machine_hardware_id: Uuid::new_v5(&namespace, &name),
    }
}

#[cfg(test)]
mod tests {
    use super::super::{AnchorKind, ReadOutcome, evaluate_slot};
    use super::*;

    const TEST_NAMESPACE: Uuid = Uuid::from_u128(0x1234_5678_1234_5678_9234_5678_1234_5678);
    const SYSTEM_UUID: &str = "550e8400-e29b-41d4-a716-446655440000";

    fn evaluate_fixture(values: [Option<&str>; 3]) -> [SlotEvaluation; 3] {
        std::array::from_fn(|index| {
            let reading = values[index].map_or(ReadOutcome::Unavailable, |value| {
                ReadOutcome::Value(value.to_owned())
            });
            evaluate_slot(ANCHOR_ORDER[index], &reading, TEST_NAMESPACE)
        })
    }

    fn derived_id(decision: MachineIdentityDecision) -> Uuid {
        match decision {
            MachineIdentityDecision::Derived {
                machine_hardware_id,
                ..
            } => machine_hardware_id,
            other => panic!("expected a derived identity, got {other:?}"),
        }
    }

    fn valid_value(kind: AnchorKind) -> &'static str {
        match kind {
            AnchorKind::DmiSystemUuid => SYSTEM_UUID,
            AnchorKind::DmiBoardSerial => "board-42",
            AnchorKind::FirstDiskSerial => "disk-99",
        }
    }

    fn evaluation_with_status(kind: AnchorKind, status: EvidenceStatus) -> SlotEvaluation {
        let reading = ReadOutcome::Value(valid_value(kind).to_owned());
        let mut evaluation = evaluate_slot(kind, &reading, TEST_NAMESPACE);
        if status != EvidenceStatus::Present {
            evaluation.status = status;
            evaluation.normalized_value = None;
        }
        evaluation
    }

    #[test]
    fn claim_decision_table_covers_all_216_status_combinations() {
        fn exhaustive_status_entry(status: EvidenceStatus) -> (usize, EvidenceStatus) {
            match status {
                EvidenceStatus::Present => (0, EvidenceStatus::Present),
                EvidenceStatus::Unavailable => (1, EvidenceStatus::Unavailable),
                EvidenceStatus::Unsupported => (2, EvidenceStatus::Unsupported),
                EvidenceStatus::PermissionDenied => (3, EvidenceStatus::PermissionDenied),
                EvidenceStatus::Malformed => (4, EvidenceStatus::Malformed),
                EvidenceStatus::RejectedPlaceholder => (5, EvidenceStatus::RejectedPlaceholder),
            }
        }

        let status_entries = [
            exhaustive_status_entry(EvidenceStatus::Present),
            exhaustive_status_entry(EvidenceStatus::Unavailable),
            exhaustive_status_entry(EvidenceStatus::Unsupported),
            exhaustive_status_entry(EvidenceStatus::PermissionDenied),
            exhaustive_status_entry(EvidenceStatus::Malformed),
            exhaustive_status_entry(EvidenceStatus::RejectedPlaceholder),
        ];
        for (expected_index, (actual_index, _)) in status_entries.iter().enumerate() {
            assert_eq!(expected_index, *actual_index);
        }
        let statuses = status_entries.map(|(_, status)| status);

        for first in statuses {
            for second in statuses {
                for third in statuses {
                    let status_combination = [first, second, third];
                    let evaluations = [
                        evaluation_with_status(ANCHOR_ORDER[0], first),
                        evaluation_with_status(ANCHOR_ORDER[1], second),
                        evaluation_with_status(ANCHOR_ORDER[2], third),
                    ];
                    let present_slot_count = status_combination
                        .iter()
                        .filter(|status| **status == EvidenceStatus::Present)
                        .count();
                    #[allow(clippy::manual_contains)]
                    let has_unsupported = status_combination
                        .iter()
                        .any(|status| *status == EvidenceStatus::Unsupported);
                    let actual = decide_machine_identity(&evaluations);
                    match actual {
                        MachineIdentityDecision::Unsupported if has_unsupported => {}
                        MachineIdentityDecision::Derived { .. }
                            if !has_unsupported && present_slot_count >= 2 => {}
                        MachineIdentityDecision::InsufficientSources
                            if !has_unsupported && present_slot_count < 2 => {}
                        unexpected => {
                            panic!("wrong decision for {status_combination:?}: {unexpected:?}")
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn whole_machine_byte_recipe_has_a_pinned_golden_uuid() {
        let evaluations = evaluate_fixture([
            Some(" 550E8400-E29B-41D4-A716-446655440000 "),
            Some(" BOARD-42 "),
            Some("DISK_99"),
        ]);
        let actual = derived_id(decide_machine_identity(&evaluations));
        let expected_name = concat!(
            "dmi_system_uuid\0",
            "550e8400e29b41d4a716446655440000\0",
            "dmi_board_serial\0",
            "board42\0",
            "first_disk_serial\0",
            "disk99\0",
        )
        .as_bytes();

        assert_eq!(actual, Uuid::new_v5(&TEST_NAMESPACE, expected_name));
        assert_eq!(actual.to_string(), "a9aa9d04-3ece-5567-8260-910930ff5e03");
    }

    #[test]
    fn missing_slot_marker_byte_has_a_pinned_golden_uuid() {
        let evaluations = evaluate_fixture([
            Some(" 550E8400-E29B-41D4-A716-446655440000 "),
            Some(" BOARD-42 "),
            None,
        ]);
        let actual = derived_id(decide_machine_identity(&evaluations));
        let expected_name: &[u8] = b"dmi_system_uuid\x00\
              550e8400e29b41d4a716446655440000\x00\
              dmi_board_serial\x00board42\x00\
              first_disk_serial\x00\x01\x00";

        assert_eq!(actual, Uuid::new_v5(&TEST_NAMESPACE, expected_name));
        assert_eq!(actual.to_string(), "7868c4db-ba77-52b9-a93c-f1ee2445e5f8");
    }

    #[test]
    fn missing_slot_position_and_presence_both_change_the_machine_id() {
        let all_present = derived_id(decide_machine_identity(&evaluate_fixture([
            Some(SYSTEM_UUID),
            Some("board-42"),
            Some("disk-99"),
        ])));
        let missing_system = derived_id(decide_machine_identity(&evaluate_fixture([
            None,
            Some("board-42"),
            Some("disk-99"),
        ])));
        let missing_board = derived_id(decide_machine_identity(&evaluate_fixture([
            Some(SYSTEM_UUID),
            None,
            Some("disk-99"),
        ])));
        let missing_disk = derived_id(decide_machine_identity(&evaluate_fixture([
            Some(SYSTEM_UUID),
            Some("board-42"),
            None,
        ])));

        assert_ne!(missing_system, missing_board);
        assert_ne!(missing_system, missing_disk);
        assert_ne!(missing_board, missing_disk);
        assert_ne!(all_present, missing_disk);
    }

    #[test]
    fn control_byte_marker_cannot_be_a_normalized_value() {
        let evaluation = evaluate_slot(
            AnchorKind::DmiBoardSerial,
            &ReadOutcome::Value("\u{1}".to_owned()),
            TEST_NAMESPACE,
        );

        assert_eq!(evaluation.status, EvidenceStatus::Malformed);
        assert_eq!(evaluation.normalized_value, None);
    }

    #[test]
    fn recomputation_is_byte_identical() {
        let evaluations = evaluate_fixture([Some(SYSTEM_UUID), Some("board-42"), Some("disk-99")]);
        let first = derived_id(decide_machine_identity(&evaluations));
        let second = derived_id(decide_machine_identity(&evaluations));

        assert_eq!(first.as_bytes(), second.as_bytes());
    }

    #[test]
    fn swapping_two_slot_values_changes_the_machine_id() {
        let original = derived_id(decide_machine_identity(&evaluate_fixture([
            Some(SYSTEM_UUID),
            Some("board-value"),
            Some("disk-value"),
        ])));
        let swapped = derived_id(decide_machine_identity(&evaluate_fixture([
            Some(SYSTEM_UUID),
            Some("disk-value"),
            Some("board-value"),
        ])));

        assert_ne!(original, swapped);
    }
}
