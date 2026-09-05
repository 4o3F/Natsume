#![forbid(unsafe_code)]

use std::fmt;

use uuid::Uuid;

mod claim;

pub(super) use claim::{MachineIdentityDecision, decide_machine_identity};

/// A frozen hardware-evidence source slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AnchorKind {
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
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::DmiSystemUuid => "dmi_system_uuid",
            Self::DmiBoardSerial => "dmi_board_serial",
            Self::FirstDiskSerial => "first_disk_serial",
        }
    }
}

/// The frozen hardware-evidence collection order.
pub(super) const ANCHOR_ORDER: [AnchorKind; 3] = [
    AnchorKind::DmiSystemUuid,
    AnchorKind::DmiBoardSerial,
    AnchorKind::FirstDiskSerial,
];

/// The I/O outcome supplied to the pure identity-policy evaluator.
pub(super) enum ReadOutcome {
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
pub(super) struct SlotEvaluation {
    /// The classified collection status.
    pub(super) status: EvidenceStatus,
    /// The source slot's inherent quality grade.
    pub(super) quality: EvidenceQuality,
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
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum EvidenceQuality {
    Medium,
    Strong,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EvidenceStatus {
    Present,
    Unavailable,
    Unsupported,
    PermissionDenied,
    Malformed,
    RejectedPlaceholder,
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

/// Normalizes and classifies one source reading.
///
/// This function is pure: callers map platform I/O into [`ReadOutcome`], while normalization,
/// placeholder rejection and quality assignment remain here.
#[must_use]
pub(super) fn evaluate_slot(
    kind: AnchorKind,
    reading: &ReadOutcome,
    fleet_namespace: Uuid,
) -> SlotEvaluation {
    let quality = anchor_quality(kind);
    match reading {
        ReadOutcome::Value(value) => match normalize_value(kind, value) {
            NormalizedValue::Present(normalized) => SlotEvaluation {
                status: EvidenceStatus::Present,
                quality,
                normalized_value: Some(normalized),
                fleet_namespace,
            },
            NormalizedValue::Malformed => SlotEvaluation {
                status: EvidenceStatus::Malformed,
                quality,
                normalized_value: None,
                fleet_namespace,
            },
            NormalizedValue::RejectedPlaceholder => SlotEvaluation {
                status: EvidenceStatus::RejectedPlaceholder,
                quality,
                normalized_value: None,
                fleet_namespace,
            },
        },
        ReadOutcome::Unavailable => SlotEvaluation {
            status: EvidenceStatus::Unavailable,
            quality,
            normalized_value: None,
            fleet_namespace,
        },
        ReadOutcome::PermissionDenied => SlotEvaluation {
            status: EvidenceStatus::PermissionDenied,
            quality,
            normalized_value: None,
            fleet_namespace,
        },
        ReadOutcome::Unsupported => SlotEvaluation {
            status: EvidenceStatus::Unsupported,
            quality,
            normalized_value: None,
            fleet_namespace,
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
                assert_eq!(evaluation.normalized_value, None);
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
            assert!(evaluation.normalized_value.is_some());
        }
    }

    #[test]
    fn placeholder_projection_does_not_change_normalized_value() {
        let namespace = Uuid::from_u128(107);
        let evaluation = evaluate_slot(
            AnchorKind::DmiBoardSerial,
            &ReadOutcome::Value(" Board/42 ".to_owned()),
            namespace,
        );

        assert_eq!(evaluation.status, EvidenceStatus::Present);
        assert_eq!(evaluation.normalized_value.as_deref(), Some("board/42"));
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
        assert_eq!(noisy.normalized_value, canonical.normalized_value);
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
        assert_eq!(malformed.status, EvidenceStatus::Malformed);
        assert_eq!(malformed.normalized_value, None);
        assert_eq!(mixed_case.status, EvidenceStatus::Present);
        assert_eq!(
            mixed_case.normalized_value.as_deref(),
            Some("550e8400e29b41d4a716446655440000")
        );
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
                evaluation.normalized_value.is_some(),
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
}
