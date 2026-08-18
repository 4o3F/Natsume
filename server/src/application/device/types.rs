use snafu::Snafu;
use uuid::{Uuid, Variant, Version};

use crate::audit::AuditPersistenceError;

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

    pub(crate) fn from_uuid(value: Uuid) -> Option<Self> {
        if value.get_version() != Some(Version::SortRand) || value.get_variant() != Variant::RFC4122
        {
            return None;
        }
        Some(Self(value))
    }

    pub(crate) const fn value(&self) -> Uuid {
        self.0
    }

    pub(crate) fn as_text(&self) -> String {
        self.value().hyphenated().to_string()
    }
}

/// Redacted persistence boundary shared by Device-owned adapters and read models.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DevicePersistenceError {
    InvalidPersistedFacts,
    PersistenceFailed,
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

impl DeviceError {
    pub(crate) const fn from_persistence(error: DevicePersistenceError) -> Self {
        match error {
            DevicePersistenceError::InvalidPersistedFacts => Self::InvalidPersistedFacts,
            DevicePersistenceError::PersistenceFailed => Self::PersistenceFailed,
        }
    }

    pub(crate) const fn from_audit_persistence(error: AuditPersistenceError) -> Self {
        match error {
            AuditPersistenceError::PersistenceFailed => Self::PersistenceFailed,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeviceState {
    Enrolled,
    Revoked,
    Disabled,
}

impl DeviceState {
    pub(crate) fn from_persisted(value: &str) -> Option<Self> {
        match value {
            "enrolled" => Some(Self::Enrolled),
            "revoked" => Some(Self::Revoked),
            "disabled" => Some(Self::Disabled),
            _ => None,
        }
    }

    pub(crate) const fn as_persisted(self) -> &'static str {
        match self {
            Self::Enrolled => "enrolled",
            Self::Revoked => "revoked",
            Self::Disabled => "disabled",
        }
    }
}

// The closed `devices.hardware_identity_quality` vocabulary frozen by the
// migration's CHECK constraint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HardwareIdentityQuality {
    Strong,
    Medium,
    Weak,
}

impl HardwareIdentityQuality {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "strong" => Some(Self::Strong),
            "medium" => Some(Self::Medium),
            "weak" => Some(Self::Weak),
            _ => None,
        }
    }

    pub(crate) const fn as_persisted(self) -> &'static str {
        match self {
            Self::Strong => "strong",
            Self::Medium => "medium",
            Self::Weak => "weak",
        }
    }
}

/// Application-owned projection of the Device row selected by hardware ID.
#[derive(Clone, Copy)]
pub(crate) struct DeviceByHardwareProjection {
    pub(in crate::application::device) device_id: DeviceId,
    pub(in crate::application::device) hardware_identity_quality: HardwareIdentityQuality,
    pub(in crate::application::device) state: DeviceState,
}

impl DeviceByHardwareProjection {
    pub(crate) fn from_persisted(
        device_id: &str,
        hardware_identity_quality: &str,
        state: &str,
    ) -> Result<Self, DevicePersistenceError> {
        Ok(Self {
            device_id: DeviceId::parse(device_id)
                .ok_or(DevicePersistenceError::InvalidPersistedFacts)?,
            hardware_identity_quality: HardwareIdentityQuality::parse(hardware_identity_quality)
                .ok_or(DevicePersistenceError::InvalidPersistedFacts)?,
            state: DeviceState::from_persisted(state)
                .ok_or(DevicePersistenceError::InvalidPersistedFacts)?,
        })
    }
}

pub(crate) struct DeviceFacts {
    device_id: DeviceId,
    state: DeviceState,
    hardware_identity_quality: HardwareIdentityQuality,
}

impl DeviceFacts {
    pub(crate) fn new(
        device_id: DeviceId,
        state: DeviceState,
        hardware_identity_quality: HardwareIdentityQuality,
    ) -> Self {
        Self {
            device_id,
            state,
            hardware_identity_quality,
        }
    }

    pub(crate) fn into_parts(self) -> (DeviceId, DeviceState, HardwareIdentityQuality) {
        (self.device_id, self.state, self.hardware_identity_quality)
    }
}

#[cfg(test)]
mod tests {
    use crate::audit::AuditPersistenceError;

    use super::{
        DeviceError, DeviceId, DevicePersistenceError, DeviceState, HardwareIdentityQuality,
    };

    #[test]
    fn persistence_mappings_cover_every_neutral_variant() {
        assert_eq!(
            DeviceError::from_persistence(DevicePersistenceError::InvalidPersistedFacts),
            DeviceError::InvalidPersistedFacts
        );
        assert_eq!(
            DeviceError::from_persistence(DevicePersistenceError::PersistenceFailed),
            DeviceError::PersistenceFailed
        );
        assert_eq!(
            DeviceError::from_audit_persistence(AuditPersistenceError::PersistenceFailed),
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
            ("enrolled", DeviceState::Enrolled),
            ("revoked", DeviceState::Revoked),
            ("disabled", DeviceState::Disabled),
        ] {
            assert_eq!(DeviceState::from_persisted(persisted), Some(state));
            assert_eq!(state.as_persisted(), persisted);
        }

        for invalid in ["", "Enrolled", "disabled ", "active", "weak"] {
            assert!(DeviceState::from_persisted(invalid).is_none());
        }
    }

    #[test]
    fn hardware_identity_quality_persisted_vocabulary_roundtrips_exhaustively() {
        for (persisted, quality) in [
            ("strong", HardwareIdentityQuality::Strong),
            ("medium", HardwareIdentityQuality::Medium),
            ("weak", HardwareIdentityQuality::Weak),
        ] {
            assert_eq!(HardwareIdentityQuality::parse(persisted), Some(quality));
            assert_eq!(quality.as_persisted(), persisted);
        }

        for invalid in ["", "Strong", "medium ", "unknown", "enrolled"] {
            assert!(HardwareIdentityQuality::parse(invalid).is_none());
        }
    }
}
