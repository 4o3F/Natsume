#![allow(dead_code)]

use snafu::Snafu;
use uuid::{Uuid, Variant, Version};

use crate::db::PersistenceError;

/// Canonical, lowercase, hyphenated `UUIDv7` identifier for a Device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DeviceId(Uuid);

impl DeviceId {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        let parsed = Uuid::parse_str(value).ok()?;
        if parsed.hyphenated().to_string() != value {
            return None;
        }
        Self::from_uuid(parsed)
    }

    fn from_uuid(value: Uuid) -> Option<Self> {
        if value.get_version() != Some(Version::SortRand) || value.get_variant() != Variant::RFC4122
        {
            return None;
        }
        Some(Self(value))
    }

    const fn value(&self) -> Uuid {
        self.0
    }

    pub(crate) fn as_text(&self) -> String {
        self.value().hyphenated().to_string()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Snafu)]
pub(crate) enum DeviceError {
    #[snafu(display("the Device ID is invalid"))]
    InvalidDeviceId,
    #[snafu(display("the Device does not exist"))]
    DeviceNotFound,
    #[snafu(display("persisted Device facts are invalid"))]
    InvalidPersistedFacts,
    #[snafu(display("Device persistence failed"))]
    PersistenceFailed,
}

impl From<PersistenceError> for DeviceError {
    fn from(error: PersistenceError) -> Self {
        match error {
            PersistenceError::InvalidPersistedData => Self::InvalidPersistedFacts,
            PersistenceError::OperationFailed => Self::PersistenceFailed,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::component::device) enum DeviceState {
    Enabled,
    Revoked,
    Disabled,
}

impl DeviceState {
    pub(in crate::component::device) fn from_persisted(value: &str) -> Option<Self> {
        match value {
            "enabled" => Some(Self::Enabled),
            "revoked" => Some(Self::Revoked),
            "disabled" => Some(Self::Disabled),
            _ => None,
        }
    }

    pub(in crate::component::device) const fn as_persisted(self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::Revoked => "revoked",
            Self::Disabled => "disabled",
        }
    }
}

// The closed `devices.evidence_quality` vocabulary owned by this component.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::component::device) enum EvidenceQuality {
    Strong,
    Medium,
}

impl EvidenceQuality {
    pub(in crate::component::device) fn parse(value: &str) -> Option<Self> {
        match value {
            "strong" => Some(Self::Strong),
            "medium" => Some(Self::Medium),
            _ => None,
        }
    }

    pub(in crate::component::device) const fn as_persisted(self) -> &'static str {
        match self {
            Self::Strong => "strong",
            Self::Medium => "medium",
        }
    }
}

/// Application-owned projection of the Device row selected by hardware ID.
#[derive(Clone, Copy)]
struct DeviceByHardwareProjection {
    device_id: DeviceId,
    evidence_quality: EvidenceQuality,
    state: DeviceState,
}

impl DeviceByHardwareProjection {
    fn from_persisted(
        device_id: &str,
        evidence_quality: &str,
        state: &str,
    ) -> Result<Self, PersistenceError> {
        Ok(Self {
            device_id: DeviceId::parse(device_id).ok_or(PersistenceError::InvalidPersistedData)?,
            evidence_quality: EvidenceQuality::parse(evidence_quality)
                .ok_or(PersistenceError::InvalidPersistedData)?,
            state: DeviceState::from_persisted(state)
                .ok_or(PersistenceError::InvalidPersistedData)?,
        })
    }
}

pub(in crate::component::device) struct DeviceFacts {
    device_id: DeviceId,
    state: DeviceState,
    evidence_quality: EvidenceQuality,
}

impl DeviceFacts {
    pub(in crate::component::device) fn new(
        device_id: DeviceId,
        state: DeviceState,
        evidence_quality: EvidenceQuality,
    ) -> Self {
        Self {
            device_id,
            state,
            evidence_quality,
        }
    }

    fn into_parts(self) -> (DeviceId, DeviceState, EvidenceQuality) {
        (self.device_id, self.state, self.evidence_quality)
    }
}

#[cfg(test)]
mod tests {
    use crate::db::PersistenceError;

    use super::{DeviceError, DeviceId, DeviceState, EvidenceQuality};

    #[test]
    fn persistence_mappings_cover_every_neutral_variant() {
        assert_eq!(
            DeviceError::from(PersistenceError::InvalidPersistedData),
            DeviceError::InvalidPersistedFacts
        );
        assert_eq!(
            DeviceError::from(PersistenceError::OperationFailed),
            DeviceError::PersistenceFailed
        );
    }

    #[test]
    fn device_id_accepts_only_canonical_uuid_v7_text() {
        const CANONICAL_V7: &str = "01900000-0000-7000-8000-000000000001";

        let parsed = DeviceId::parse(CANONICAL_V7)
            .unwrap_or_else(|| panic!("a canonical UUIDv7 Device ID was rejected"));
        assert_eq!(parsed.as_text(), CANONICAL_V7);
        assert_eq!(parsed.value().get_version_num(), 7);

        for invalid in [
            "01900000-0000-7000-8000-00000000000A",
            "01900000000070008000000000000001",
            "550e8400-e29b-41d4-a716-446655440000",
            "01900000-0000-7000-0000-000000000001",
        ] {
            assert!(
                DeviceId::parse(invalid).is_none(),
                "a non-canonical or non-v7 Device ID was accepted: {invalid}"
            );
        }
    }

    #[test]
    fn device_state_persisted_vocabulary_roundtrips_exhaustively() {
        for (persisted, state) in [
            ("enabled", DeviceState::Enabled),
            ("revoked", DeviceState::Revoked),
            ("disabled", DeviceState::Disabled),
        ] {
            assert_eq!(DeviceState::from_persisted(persisted), Some(state));
            assert_eq!(state.as_persisted(), persisted);
        }

        for invalid in ["", "Enabled", "disabled ", "active", "weak"] {
            assert!(DeviceState::from_persisted(invalid).is_none());
        }
    }

    #[test]
    fn evidence_quality_persisted_vocabulary_roundtrips_exhaustively() {
        for (persisted, quality) in [
            ("strong", EvidenceQuality::Strong),
            ("medium", EvidenceQuality::Medium),
        ] {
            assert_eq!(EvidenceQuality::parse(persisted), Some(quality));
            assert_eq!(quality.as_persisted(), persisted);
        }

        for invalid in ["", "Strong", "medium ", "weak", "unknown", "enabled"] {
            assert!(EvidenceQuality::parse(invalid).is_none());
        }
    }
}
