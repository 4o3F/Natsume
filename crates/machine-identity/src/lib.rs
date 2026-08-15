#![forbid(unsafe_code)]

use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub mod claim;

pub use claim::{
    MachineIdentityDecision, StartupIdentityDecision, decide_machine_identity,
    evaluate_startup_identity,
};

/// A frozen hardware-evidence source slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnchorKind {
    /// The DMI system UUID.
    DmiSystemUuid,
    /// The DMI motherboard serial.
    DmiBoardSerial,
    /// The serial of the whole disk backing the root filesystem.
    FirstDiskSerial,
}

impl AnchorKind {
    /// Returns the frozen domain-separation label for this source slot.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::DmiSystemUuid => "dmi_system_uuid",
            Self::DmiBoardSerial => "dmi_board_serial",
            Self::FirstDiskSerial => "first_disk_serial",
        }
    }
}

/// The frozen hardware-evidence collection order.
pub const ANCHOR_ORDER: [AnchorKind; 3] = [
    AnchorKind::DmiSystemUuid,
    AnchorKind::DmiBoardSerial,
    AnchorKind::FirstDiskSerial,
];

/// The I/O outcome supplied to the pure identity-policy evaluator.
pub enum ReadOutcome {
    /// Text read from the source. A Unicode replacement marker denotes a failed byte decoding.
    Value(String),
    /// The source is temporarily unavailable.
    Unavailable,
    /// The source could not be read with the caller's permissions.
    PermissionDenied,
    /// The platform does not expose this source.
    Unsupported,
}

/// The policy result for one frozen hardware-evidence source slot.
#[derive(Clone, PartialEq, Eq)]
pub struct SlotEvaluation {
    /// The classified collection status.
    pub status: EvidenceStatus,
    /// The source slot's inherent quality grade.
    pub quality: EvidenceQuality,
    /// The anonymized `UUIDv5` candidate, present only for valid evidence.
    pub candidate_id: Option<Uuid>,
    // Claim-layer input retained inside the pure-computation boundary. The custom Debug
    // implementation intentionally keeps normalized hardware values out of logs.
    normalized_value: Option<String>,
    fleet_namespace: Uuid,
}

impl fmt::Debug for SlotEvaluation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SlotEvaluation")
            .field("status", &self.status)
            .field("quality", &self.quality)
            .field("candidate_id", &self.candidate_id)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceQuality {
    Weak,
    Medium,
    Strong,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceStatus {
    Present,
    Unavailable,
    Unsupported,
    PermissionDenied,
    Malformed,
    RejectedPlaceholder,
    Conflict,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollectionCompleteness {
    Complete,
    TemporarilyUnavailable,
    Unsupported,
}

/// Initial vendor placeholders frozen by the 2026-08-15 ADR-0032 amendment; additions require
/// another dated amendment.
const REJECTED_PLACEHOLDERS: [&str; 9] = [
    "tobefilledbyoem",
    "defaultstring",
    "systemserialnumber",
    "notspecified",
    "none",
    "unknown",
    "na",
    "invalid",
    "0123456789",
];

enum NormalizedValue {
    Present(String),
    Malformed,
    RejectedPlaceholder,
}

const fn anchor_quality(kind: AnchorKind) -> EvidenceQuality {
    match kind {
        AnchorKind::DmiSystemUuid | AnchorKind::DmiBoardSerial => EvidenceQuality::Strong,
        AnchorKind::FirstDiskSerial => EvidenceQuality::Medium,
    }
}

fn is_rejected_placeholder(value: &str) -> bool {
    value.is_empty()
        || value.bytes().all(|byte| byte == b'0')
        || value.bytes().all(|byte| byte == b'f')
        || {
            let projection = value
                .chars()
                .filter(|character| character.is_alphanumeric())
                .collect::<String>();
            REJECTED_PLACEHOLDERS.contains(&projection.as_str())
        }
}

fn normalize_value(kind: AnchorKind, value: &str) -> NormalizedValue {
    if value.contains('\u{fffd}') {
        return NormalizedValue::Malformed;
    }

    let trimmed = value.trim_matches(|character: char| character.is_ascii_whitespace());
    if trimmed.chars().any(char::is_control) {
        return NormalizedValue::Malformed;
    }

    let normalized = trimmed
        .chars()
        .filter(|character| !matches!(character, '-' | '_' | ':' | ' '))
        .flat_map(char::to_lowercase)
        .collect::<String>();

    if is_rejected_placeholder(&normalized) {
        return NormalizedValue::RejectedPlaceholder;
    }

    if kind == AnchorKind::DmiSystemUuid {
        return match Uuid::parse_str(&normalized) {
            Ok(uuid) => NormalizedValue::Present(uuid.simple().to_string()),
            Err(_) => NormalizedValue::Malformed,
        };
    }

    NormalizedValue::Present(normalized)
}

fn derive_slot_candidate(namespace: Uuid, kind: AnchorKind, normalized_value: &str) -> Uuid {
    let mut name = Vec::with_capacity(kind.label().len() + normalized_value.len() + 1);
    name.extend_from_slice(kind.label().as_bytes());
    name.push(0);
    name.extend_from_slice(normalized_value.as_bytes());
    Uuid::new_v5(&namespace, &name)
}

/// Normalizes and classifies one source reading and derives its anonymized candidate.
///
/// This function is pure: callers map platform I/O into [`ReadOutcome`], while normalization,
/// placeholder rejection, quality assignment and candidate derivation remain here.
#[must_use]
pub fn evaluate_slot(
    kind: AnchorKind,
    reading: &ReadOutcome,
    fleet_namespace: Uuid,
) -> SlotEvaluation {
    let quality = anchor_quality(kind);
    match reading {
        ReadOutcome::Value(value) => match normalize_value(kind, value) {
            NormalizedValue::Present(normalized) => {
                let candidate_id = derive_slot_candidate(fleet_namespace, kind, &normalized);
                SlotEvaluation {
                    status: EvidenceStatus::Present,
                    quality,
                    candidate_id: Some(candidate_id),
                    normalized_value: Some(normalized),
                    fleet_namespace,
                }
            }
            NormalizedValue::Malformed => SlotEvaluation {
                status: EvidenceStatus::Malformed,
                quality,
                candidate_id: None,
                normalized_value: None,
                fleet_namespace,
            },
            NormalizedValue::RejectedPlaceholder => SlotEvaluation {
                status: EvidenceStatus::RejectedPlaceholder,
                quality,
                candidate_id: None,
                normalized_value: None,
                fleet_namespace,
            },
        },
        ReadOutcome::Unavailable => SlotEvaluation {
            status: EvidenceStatus::Unavailable,
            quality,
            candidate_id: None,
            normalized_value: None,
            fleet_namespace,
        },
        ReadOutcome::PermissionDenied => SlotEvaluation {
            status: EvidenceStatus::PermissionDenied,
            quality,
            candidate_id: None,
            normalized_value: None,
            fleet_namespace,
        },
        ReadOutcome::Unsupported => SlotEvaluation {
            status: EvidenceStatus::Unsupported,
            quality,
            candidate_id: None,
            normalized_value: None,
            fleet_namespace,
        },
    }
}

/// Classifies the completeness of the three frozen source slots.
#[must_use]
pub fn collection_completeness(statuses: &[EvidenceStatus; 3]) -> CollectionCompleteness {
    if statuses.contains(&EvidenceStatus::Unsupported) {
        CollectionCompleteness::Unsupported
    } else if statuses
        .iter()
        .all(|status| *status == EvidenceStatus::Present)
    {
        CollectionCompleteness::Complete
    } else {
        CollectionCompleteness::TemporarilyUnavailable
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum IdentityRecordState {
    Absent,
    Corrupt,
    Valid {
        fleet_namespace_uuid: Uuid,
        machine_hardware_id: Uuid,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "decision")]
pub enum LocalIdentityPreflightDecision {
    CleanFirstStart,
    ReadyForHardwareCheck {
        stored_machine_hardware_id: Uuid,
    },
    IdentityRecordMissingWithState,
    IdentityRecordCorrupt,
    SiteNamespaceMismatch {
        configured_namespace: Uuid,
        stored_namespace: Uuid,
    },
}

/// Evaluates local files before hardware collection or any vault open.
///
/// `identity_bound_artifacts_present` means at least one Client database, root key,
/// certificate, LKG or initialization journal exists. An absent identity record is
/// only a clean first start when all of those artifacts are also absent.
#[must_use]
pub fn evaluate_local_identity_preflight(
    configured_namespace: Uuid,
    identity_record: IdentityRecordState,
    identity_bound_artifacts_present: bool,
) -> LocalIdentityPreflightDecision {
    match identity_record {
        IdentityRecordState::Absent if identity_bound_artifacts_present => {
            LocalIdentityPreflightDecision::IdentityRecordMissingWithState
        }
        IdentityRecordState::Absent => LocalIdentityPreflightDecision::CleanFirstStart,
        IdentityRecordState::Corrupt => LocalIdentityPreflightDecision::IdentityRecordCorrupt,
        IdentityRecordState::Valid {
            fleet_namespace_uuid,
            ..
        } if fleet_namespace_uuid != configured_namespace => {
            LocalIdentityPreflightDecision::SiteNamespaceMismatch {
                configured_namespace,
                stored_namespace: fleet_namespace_uuid,
            }
        }
        IdentityRecordState::Valid {
            machine_hardware_id,
            ..
        } => LocalIdentityPreflightDecision::ReadyForHardwareCheck {
            stored_machine_hardware_id: machine_hardware_id,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frozen_anchor_order_and_labels_match_the_recipe() {
        assert_eq!(
            ANCHOR_ORDER,
            [
                AnchorKind::DmiSystemUuid,
                AnchorKind::DmiBoardSerial,
                AnchorKind::FirstDiskSerial,
            ]
        );
        assert_eq!(AnchorKind::DmiSystemUuid.label(), "dmi_system_uuid");
        assert_eq!(AnchorKind::DmiBoardSerial.label(), "dmi_board_serial");
        assert_eq!(AnchorKind::FirstDiskSerial.label(), "first_disk_serial");
    }

    #[test]
    fn placeholder_decision_table_rejects_every_frozen_variant() {
        let namespace = Uuid::from_u128(100);
        let cases = [
            " \t-_:\r\n",
            " 00-00_00:00 ",
            " Ff-FF_ff:ff ",
            " To-Be_Filled: By OEM ",
            " To Be Filled By O.E.M. ",
            " DEFAULT-STRING ",
            " System Serial_Number ",
            " Not-Specified ",
            " No-Ne ",
            " Un_known ",
            " N-A ",
            " N/A ",
            " In:Valid ",
            " 0123-45 67_89 ",
        ];

        for kind in ANCHOR_ORDER {
            for value in cases {
                let evaluation =
                    evaluate_slot(kind, &ReadOutcome::Value(value.to_owned()), namespace);
                assert_eq!(evaluation.status, EvidenceStatus::RejectedPlaceholder);
                assert_eq!(evaluation.candidate_id, None);
            }
        }
    }

    #[test]
    fn placeholder_table_is_pinned_to_the_adr_snapshot() {
        assert_eq!(REJECTED_PLACEHOLDERS.len(), 9);
        assert_eq!(
            REJECTED_PLACEHOLDERS,
            [
                "tobefilledbyoem",
                "defaultstring",
                "systemserialnumber",
                "notspecified",
                "none",
                "unknown",
                "na",
                "invalid",
                "0123456789",
            ]
        );
    }

    #[test]
    fn placeholder_projection_near_misses_remain_present() {
        let namespace = Uuid::from_u128(106);
        for value in ["unknown2", "01234567890", "0001"] {
            let evaluation = evaluate_slot(
                AnchorKind::DmiBoardSerial,
                &ReadOutcome::Value(value.to_owned()),
                namespace,
            );
            assert_eq!(evaluation.status, EvidenceStatus::Present);
            assert!(evaluation.candidate_id.is_some());
        }
    }

    #[test]
    fn placeholder_projection_does_not_change_candidate_input() {
        let namespace = Uuid::from_u128(107);
        let evaluation = evaluate_slot(
            AnchorKind::DmiBoardSerial,
            &ReadOutcome::Value(" Board/42 ".to_owned()),
            namespace,
        );
        let expected = Uuid::new_v5(&namespace, b"dmi_board_serial\0board/42");

        assert_eq!(evaluation.status, EvidenceStatus::Present);
        assert_eq!(evaluation.candidate_id, Some(expected));
    }

    #[test]
    fn separator_case_and_space_normalization_is_equivalent() {
        let namespace = Uuid::from_u128(101);
        let noisy = evaluate_slot(
            AnchorKind::DmiBoardSerial,
            &ReadOutcome::Value(" AB-12 cd ".to_owned()),
            namespace,
        );
        let canonical = evaluate_slot(
            AnchorKind::DmiBoardSerial,
            &ReadOutcome::Value("ab12cd".to_owned()),
            namespace,
        );

        assert_eq!(noisy.status, EvidenceStatus::Present);
        assert_eq!(noisy.candidate_id, canonical.candidate_id);
    }

    #[test]
    fn system_uuid_decision_table_parses_and_canonicalizes() {
        let namespace = Uuid::from_u128(102);
        let malformed = evaluate_slot(
            AnchorKind::DmiSystemUuid,
            &ReadOutcome::Value("not-a-uuid".to_owned()),
            namespace,
        );
        let mixed_case = evaluate_slot(
            AnchorKind::DmiSystemUuid,
            &ReadOutcome::Value(" 550E8400-E29B-41D4-A716-446655440000\r\n".to_owned()),
            namespace,
        );
        let expected = Uuid::new_v5(
            &namespace,
            concat!("dmi_system_uuid", "\0", "550e8400e29b41d4a716446655440000").as_bytes(),
        );

        assert_eq!(malformed.status, EvidenceStatus::Malformed);
        assert_eq!(malformed.candidate_id, None);
        assert_eq!(mixed_case.status, EvidenceStatus::Present);
        assert_eq!(mixed_case.candidate_id, Some(expected));
    }

    #[test]
    fn read_outcome_decision_table_maps_every_status() {
        let namespace = Uuid::from_u128(103);
        let cases = [
            (
                ReadOutcome::Value("board-42".to_owned()),
                EvidenceStatus::Present,
            ),
            (ReadOutcome::Unavailable, EvidenceStatus::Unavailable),
            (
                ReadOutcome::PermissionDenied,
                EvidenceStatus::PermissionDenied,
            ),
            (ReadOutcome::Unsupported, EvidenceStatus::Unsupported),
            (
                ReadOutcome::Value("unknown".to_owned()),
                EvidenceStatus::RejectedPlaceholder,
            ),
            (
                ReadOutcome::Value("serial-\u{fffd}".to_owned()),
                EvidenceStatus::Malformed,
            ),
        ];

        for (reading, expected) in cases {
            let evaluation = evaluate_slot(AnchorKind::DmiBoardSerial, &reading, namespace);
            assert_eq!(evaluation.status, expected);
            assert_eq!(
                evaluation.candidate_id.is_some(),
                expected == EvidenceStatus::Present
            );
        }
    }

    #[test]
    fn quality_is_constant_for_each_anchor_kind() {
        let namespace = Uuid::from_u128(104);
        let cases = [
            (
                AnchorKind::DmiSystemUuid,
                "550e8400-e29b-41d4-a716-446655440000",
                EvidenceQuality::Strong,
            ),
            (
                AnchorKind::DmiBoardSerial,
                "board-42",
                EvidenceQuality::Strong,
            ),
            (
                AnchorKind::FirstDiskSerial,
                "disk-42",
                EvidenceQuality::Medium,
            ),
        ];

        for (kind, value, expected) in cases {
            let present = evaluate_slot(kind, &ReadOutcome::Value(value.to_owned()), namespace);
            let unsupported = evaluate_slot(kind, &ReadOutcome::Unsupported, namespace);
            assert_eq!(present.quality, expected);
            assert_eq!(unsupported.quality, expected);
        }
    }

    #[test]
    fn completeness_decision_table_covers_every_status_combination() {
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
                    let readings = [first, second, third];
                    // Keep the oracle in the reviewer-specified `any` form instead of sharing the
                    // implementation's `contains`/`all` operator chain.
                    #[allow(clippy::manual_contains)]
                    let has_unsupported = readings
                        .iter()
                        .any(|status| *status == EvidenceStatus::Unsupported);
                    let expected = if has_unsupported {
                        CollectionCompleteness::Unsupported
                    } else if readings
                        .iter()
                        .any(|status| *status != EvidenceStatus::Present)
                    {
                        CollectionCompleteness::TemporarilyUnavailable
                    } else {
                        CollectionCompleteness::Complete
                    };
                    assert_eq!(collection_completeness(&readings), expected);
                }
            }
        }
    }

    #[test]
    fn candidate_derivation_is_domain_separated_and_deterministic() {
        let namespace = Uuid::from_u128(105);
        let reading = ReadOutcome::Value("AB-12 cd".to_owned());
        let board = evaluate_slot(AnchorKind::DmiBoardSerial, &reading, namespace);
        let disk = evaluate_slot(AnchorKind::FirstDiskSerial, &reading, namespace);
        let repeated = evaluate_slot(AnchorKind::DmiBoardSerial, &reading, namespace);
        let expected = Uuid::new_v5(&namespace, b"dmi_board_serial\0ab12cd");

        assert_ne!(board.candidate_id, disk.candidate_id);
        assert_eq!(board.candidate_id, repeated.candidate_id);
        assert_eq!(board.candidate_id, Some(expected));
    }

    #[test]
    fn only_a_fully_empty_local_state_is_a_clean_first_start() {
        let namespace = Uuid::from_u128(10);
        assert_eq!(
            evaluate_local_identity_preflight(namespace, IdentityRecordState::Absent, false),
            LocalIdentityPreflightDecision::CleanFirstStart,
        );
        assert_eq!(
            evaluate_local_identity_preflight(namespace, IdentityRecordState::Absent, true),
            LocalIdentityPreflightDecision::IdentityRecordMissingWithState,
        );
    }

    #[test]
    fn corrupt_identity_record_is_never_treated_as_first_start() {
        assert_eq!(
            evaluate_local_identity_preflight(
                Uuid::from_u128(10),
                IdentityRecordState::Corrupt,
                false,
            ),
            LocalIdentityPreflightDecision::IdentityRecordCorrupt,
        );
    }

    #[test]
    fn site_namespace_mismatch_fails_before_hardware_or_vault_checks() {
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

    #[test]
    fn valid_local_record_proceeds_with_its_stored_machine_id() {
        let namespace = Uuid::from_u128(10);
        assert_eq!(
            evaluate_local_identity_preflight(
                namespace,
                IdentityRecordState::Valid {
                    fleet_namespace_uuid: namespace,
                    machine_hardware_id: Uuid::from_u128(1),
                },
                true,
            ),
            LocalIdentityPreflightDecision::ReadyForHardwareCheck {
                stored_machine_hardware_id: Uuid::from_u128(1),
            },
        );
    }
}
