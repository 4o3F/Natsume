#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use uuid::Uuid;

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HardwareCandidate {
    pub anchor_kind: String,
    pub candidate_id: Uuid,
    pub quality: EvidenceQuality,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HardwareClaim {
    pub candidates: Vec<HardwareCandidate>,
    pub completeness: CollectionCompleteness,
}

impl Default for HardwareClaim {
    fn default() -> Self {
        Self {
            candidates: Vec::new(),
            completeness: CollectionCompleteness::TemporarilyUnavailable,
        }
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartupIdentityDecision {
    FirstStart {
        selected: Uuid,
    },
    Matched,
    Indeterminate,
    IdentityUnavailable,
    ResetRequired {
        stored: Uuid,
        selected_current: Uuid,
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

/// Pure deterministic candidate derivation. Linux I/O stays in the privileged helper.
#[must_use]
pub fn derive_candidate(
    namespace: Uuid,
    anchor_kind: &str,
    component_fingerprints: &[&[u8]],
) -> Uuid {
    let mut name = b"natsume/machine-hardware-id/v1\0".to_vec();
    name.extend_from_slice(anchor_kind.as_bytes());
    name.push(0);
    let mut sorted = component_fingerprints.to_vec();
    sorted.sort_unstable();
    for fingerprint in sorted {
        name.extend_from_slice(fingerprint);
    }
    Uuid::new_v5(&namespace, &name)
}

fn anchor_priority(kind: &str) -> u8 {
    match kind {
        "system_uuid_board" => 0,
        "product_board" => 1,
        "system_uuid" => 2,
        "board_chassis" => 3,
        "board_processor" => 4,
        "board_root_disk" => 5,
        "board_only" => 6,
        "root_disk_only" => 7,
        _ => u8::MAX,
    }
}

/// Selects exactly one immutable ID at first start. The deterministic tie-breaker prevents
/// collector ordering from changing the choice. Unknown anchor kinds are rejected here even
/// if a compromised or future collector emits them.
#[must_use]
pub fn select_machine_hardware_id(claim: &HardwareClaim) -> Option<Uuid> {
    let mut candidates = claim
        .candidates
        .iter()
        .filter(|candidate| anchor_priority(&candidate.anchor_kind) != u8::MAX)
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        right
            .quality
            .cmp(&left.quality)
            .then_with(|| {
                anchor_priority(&left.anchor_kind).cmp(&anchor_priority(&right.anchor_kind))
            })
            .then_with(|| {
                left.candidate_id
                    .as_bytes()
                    .cmp(right.candidate_id.as_bytes())
            })
    });
    candidates.first().map(|candidate| candidate.candidate_id)
}

/// Evaluates hardware evidence before any identity-bound vault record, certificate or LKG is used.
/// A transient collection failure never destroys state; a complete contradictory claim does.
#[must_use]
pub fn evaluate_startup_identity(
    stored_machine_hardware_id: Option<Uuid>,
    current: &HardwareClaim,
) -> StartupIdentityDecision {
    let selected = select_machine_hardware_id(current);
    let Some(stored) = stored_machine_hardware_id else {
        return selected.map_or(StartupIdentityDecision::IdentityUnavailable, |selected| {
            StartupIdentityDecision::FirstStart { selected }
        });
    };

    if current
        .candidates
        .iter()
        .any(|candidate| candidate.candidate_id == stored)
    {
        return StartupIdentityDecision::Matched;
    }

    if current.completeness != CollectionCompleteness::Complete {
        return StartupIdentityDecision::Indeterminate;
    }

    selected.map_or(
        StartupIdentityDecision::IdentityUnavailable,
        |selected_current| StartupIdentityDecision::ResetRequired {
            stored,
            selected_current,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claim(id: u128, completeness: CollectionCompleteness) -> HardwareClaim {
        HardwareClaim {
            candidates: vec![HardwareCandidate {
                anchor_kind: "system_uuid".to_owned(),
                candidate_id: Uuid::from_u128(id),
                quality: EvidenceQuality::Strong,
            }],
            completeness,
        }
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

    #[test]
    fn first_start_selects_one_immutable_id() {
        assert_eq!(
            evaluate_startup_identity(None, &claim(1, CollectionCompleteness::Complete)),
            StartupIdentityDecision::FirstStart {
                selected: Uuid::from_u128(1),
            },
        );
    }

    #[test]
    fn unknown_anchor_kind_is_not_selected() {
        let claim = HardwareClaim {
            candidates: vec![HardwareCandidate {
                anchor_kind: "future_unapproved_anchor".to_owned(),
                candidate_id: Uuid::from_u128(1),
                quality: EvidenceQuality::Strong,
            }],
            completeness: CollectionCompleteness::Complete,
        };
        assert_eq!(select_machine_hardware_id(&claim), None);
    }

    #[test]
    fn stored_id_matching_any_current_candidate_is_valid() {
        assert_eq!(
            evaluate_startup_identity(
                Some(Uuid::from_u128(1)),
                &claim(1, CollectionCompleteness::Complete),
            ),
            StartupIdentityDecision::Matched,
        );
    }

    #[test]
    fn temporary_collection_failure_is_never_destructive() {
        assert_eq!(
            evaluate_startup_identity(
                Some(Uuid::from_u128(1)),
                &HardwareClaim {
                    candidates: Vec::new(),
                    completeness: CollectionCompleteness::TemporarilyUnavailable,
                },
            ),
            StartupIdentityDecision::Indeterminate,
        );
    }

    #[test]
    fn complete_contradictory_evidence_requires_standard_local_reset() {
        assert_eq!(
            evaluate_startup_identity(
                Some(Uuid::from_u128(1)),
                &claim(2, CollectionCompleteness::Complete),
            ),
            StartupIdentityDecision::ResetRequired {
                stored: Uuid::from_u128(1),
                selected_current: Uuid::from_u128(2),
            },
        );
    }
}
