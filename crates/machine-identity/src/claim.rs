//! Whole-machine identity composition and claim decisions.
//!
//! [`CollectionCompleteness`] remains the R6 collection-health report: one `unsupported`
//! slot makes the collection unsupported, while every other non-present slot makes it
//! temporarily unavailable. Claim eligibility is deliberately layered on top of that report.
//! `Unsupported` is terminal, but `TemporarilyUnavailable` does not prevent derivation when
//! two slots are present. Every non-present slot occupies its frozen [`ANCHOR_ORDER`] position
//! with the single-byte `0x01` marker, and slot quality never gates the 2-of-3 decision.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    ANCHOR_ORDER, CollectionCompleteness, EvidenceStatus, SlotEvaluation, collection_completeness,
};

const MISSING_SLOT_MARKER: u8 = 0x01;

/// The closed result of applying the whole-machine 2-of-3 claim policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "decision")]
pub enum MachineIdentityDecision {
    /// At least two slots were present and the collection contained no unsupported slot.
    Derived {
        /// The whole-machine `UUIDv5` derived from all three frozen slot positions.
        machine_hardware_id: Uuid,
        /// The number of present slots; either two or three.
        present_slot_count: usize,
    },
    /// Fewer than two slots were present and none was unsupported.
    InsufficientSources {
        /// The number of present slots; either zero or one.
        present_slot_count: usize,
    },
    /// At least one slot is unsupported on this platform.
    Unsupported {
        /// The number of present slots, retained for collection-health reporting.
        present_slot_count: usize,
    },
}

impl MachineIdentityDecision {
    /// Returns the independently computed R6 collection-health classification.
    ///
    /// A derived 2-of-3 identity can therefore report
    /// [`CollectionCompleteness::TemporarilyUnavailable`].
    #[must_use]
    pub const fn collection_completeness(&self) -> CollectionCompleteness {
        match self {
            Self::Unsupported { .. } => CollectionCompleteness::Unsupported,
            Self::Derived {
                present_slot_count: 3,
                ..
            } => CollectionCompleteness::Complete,
            Self::Derived { .. } | Self::InsufficientSources { .. } => {
                CollectionCompleteness::TemporarilyUnavailable
            }
        }
    }
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

/// Applies R6 reporting, the terminal unsupported rule, the 2-of-3 claim decision, and the
/// frozen whole-machine byte recipe.
///
/// The evaluations must occupy [`ANCHOR_ORDER`] positions and must have been produced with the
/// same immutable Fleet namespace. The namespace retained by the first evaluation is the same
/// namespace used by the per-slot R7 derivation and by the whole-machine `UUIDv5` derivation.
#[must_use]
pub fn decide_machine_identity(evaluations: &[SlotEvaluation; 3]) -> MachineIdentityDecision {
    let statuses = evaluations.each_ref().map(|evaluation| evaluation.status);
    let completeness = collection_completeness(&statuses);
    let present_slot_count = statuses
        .iter()
        .filter(|status| **status == EvidenceStatus::Present)
        .count();

    if completeness == CollectionCompleteness::Unsupported {
        return MachineIdentityDecision::Unsupported { present_slot_count };
    }

    if present_slot_count < 2 {
        return MachineIdentityDecision::InsufficientSources { present_slot_count };
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
        present_slot_count,
    }
}

/// Startup identity result after comparing a whole-machine claim with persisted state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartupIdentityDecision {
    /// No identity was stored and the current claim produced the first immutable ID.
    FirstStart {
        /// The newly derived whole-machine ID.
        machine_hardware_id: Uuid,
    },
    /// Recomputed and persisted whole-machine IDs are byte-for-byte equal.
    Matched,
    /// Too few sources were available to recompute an identity safely.
    Indeterminate,
    /// The platform is unsupported, or a first start has no derivable identity.
    IdentityUnavailable,
    /// A derivable current identity differs from the persisted immutable ID.
    ResetRequired {
        /// The persisted whole-machine ID.
        stored: Uuid,
        /// The current whole-machine recomputation.
        recomputed_machine_hardware_id: Uuid,
    },
}

/// Compares the whole-machine recomputation with persisted state using strict UUID equality.
///
/// No per-slot candidate match, priority, or nearest-match path exists. Insufficient sources
/// preserve existing state as indeterminate; an unsupported platform is terminal.
#[must_use]
pub fn evaluate_startup_identity(
    stored_machine_hardware_id: Option<Uuid>,
    current: &MachineIdentityDecision,
) -> StartupIdentityDecision {
    match (stored_machine_hardware_id, current) {
        (
            None,
            MachineIdentityDecision::Derived {
                machine_hardware_id,
                ..
            },
        ) => StartupIdentityDecision::FirstStart {
            machine_hardware_id: *machine_hardware_id,
        },
        (None, _) => StartupIdentityDecision::IdentityUnavailable,
        (
            Some(stored),
            MachineIdentityDecision::Derived {
                machine_hardware_id,
                ..
            },
        ) if stored == *machine_hardware_id => StartupIdentityDecision::Matched,
        (
            Some(stored),
            MachineIdentityDecision::Derived {
                machine_hardware_id,
                ..
            },
        ) => StartupIdentityDecision::ResetRequired {
            stored,
            recomputed_machine_hardware_id: *machine_hardware_id,
        },
        (Some(_), MachineIdentityDecision::InsufficientSources { .. }) => {
            StartupIdentityDecision::Indeterminate
        }
        (Some(_), MachineIdentityDecision::Unsupported { .. }) => {
            StartupIdentityDecision::IdentityUnavailable
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AnchorKind, EvidenceQuality, ReadOutcome, evaluate_slot};

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
            evaluation.candidate_id = None;
            evaluation.normalized_value = None;
        }
        evaluation
    }

    #[test]
    fn claim_decision_table_covers_all_343_status_combinations() {
        fn exhaustive_status_entry(status: EvidenceStatus) -> (usize, EvidenceStatus) {
            match status {
                EvidenceStatus::Present => (0, EvidenceStatus::Present),
                EvidenceStatus::Unavailable => (1, EvidenceStatus::Unavailable),
                EvidenceStatus::Unsupported => (2, EvidenceStatus::Unsupported),
                EvidenceStatus::PermissionDenied => (3, EvidenceStatus::PermissionDenied),
                EvidenceStatus::Malformed => (4, EvidenceStatus::Malformed),
                EvidenceStatus::RejectedPlaceholder => (5, EvidenceStatus::RejectedPlaceholder),
                EvidenceStatus::Conflict => (6, EvidenceStatus::Conflict),
            }
        }

        let status_entries = [
            exhaustive_status_entry(EvidenceStatus::Present),
            exhaustive_status_entry(EvidenceStatus::Unavailable),
            exhaustive_status_entry(EvidenceStatus::Unsupported),
            exhaustive_status_entry(EvidenceStatus::PermissionDenied),
            exhaustive_status_entry(EvidenceStatus::Malformed),
            exhaustive_status_entry(EvidenceStatus::RejectedPlaceholder),
            exhaustive_status_entry(EvidenceStatus::Conflict),
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
                    let expected_completeness = if has_unsupported {
                        CollectionCompleteness::Unsupported
                    } else if present_slot_count == 3 {
                        CollectionCompleteness::Complete
                    } else {
                        CollectionCompleteness::TemporarilyUnavailable
                    };

                    let actual = decide_machine_identity(&evaluations);
                    assert_eq!(actual.collection_completeness(), expected_completeness);
                    match actual {
                        MachineIdentityDecision::Unsupported {
                            present_slot_count: actual_count,
                        } if has_unsupported => assert_eq!(actual_count, present_slot_count),
                        MachineIdentityDecision::Derived {
                            present_slot_count: actual_count,
                            ..
                        } if !has_unsupported && present_slot_count >= 2 => {
                            assert_eq!(actual_count, present_slot_count);
                        }
                        MachineIdentityDecision::InsufficientSources {
                            present_slot_count: actual_count,
                        } if !has_unsupported && present_slot_count < 2 => {
                            assert_eq!(actual_count, present_slot_count);
                        }
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
        assert_eq!(evaluation.candidate_id, None);
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

    #[test]
    fn quality_does_not_gate_two_present_slots() {
        let mut evaluations = evaluate_fixture([Some(SYSTEM_UUID), Some("board-42"), None]);
        for evaluation in &mut evaluations {
            evaluation.quality = EvidenceQuality::Weak;
        }

        assert!(matches!(
            decide_machine_identity(&evaluations),
            MachineIdentityDecision::Derived {
                present_slot_count: 2,
                ..
            }
        ));
    }

    #[test]
    fn startup_requires_exact_whole_machine_equality() {
        let current = decide_machine_identity(&evaluate_fixture([
            Some(SYSTEM_UUID),
            Some("board-42"),
            Some("disk-99"),
        ]));
        let machine_hardware_id = derived_id(current);

        assert_eq!(
            evaluate_startup_identity(None, &current),
            StartupIdentityDecision::FirstStart {
                machine_hardware_id,
            }
        );
        assert_eq!(
            evaluate_startup_identity(Some(machine_hardware_id), &current),
            StartupIdentityDecision::Matched
        );
        assert_eq!(
            evaluate_startup_identity(Some(Uuid::from_u128(1)), &current),
            StartupIdentityDecision::ResetRequired {
                stored: Uuid::from_u128(1),
                recomputed_machine_hardware_id: machine_hardware_id,
            }
        );
    }

    #[test]
    fn startup_preserves_state_when_sources_are_insufficient() {
        let current = decide_machine_identity(&evaluate_fixture([Some(SYSTEM_UUID), None, None]));

        assert_eq!(
            evaluate_startup_identity(Some(Uuid::from_u128(1)), &current),
            StartupIdentityDecision::Indeterminate
        );
    }

    #[test]
    fn startup_treats_unsupported_platform_as_terminal() {
        let evaluations = [
            evaluation_with_status(ANCHOR_ORDER[0], EvidenceStatus::Present),
            evaluation_with_status(ANCHOR_ORDER[1], EvidenceStatus::Present),
            evaluation_with_status(ANCHOR_ORDER[2], EvidenceStatus::Unsupported),
        ];
        let current = decide_machine_identity(&evaluations);

        assert_eq!(
            evaluate_startup_identity(Some(Uuid::from_u128(1)), &current),
            StartupIdentityDecision::IdentityUnavailable
        );
    }
}
