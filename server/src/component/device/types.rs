use std::fmt;

use snafu::Snafu;
use uuid::{Uuid, Variant, Version};

use crate::db::PersistenceError;

/// Canonical, lowercase, hyphenated `UUIDv7` identifier for a Device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct DeviceId(Uuid);

impl DeviceId {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        let parsed = Uuid::parse_str(value).ok()?;
        if parsed.hyphenated().to_string() != value {
            return None;
        }
        Self::from_uuid(parsed)
    }

    pub(in crate::component::device) fn new() -> Self {
        Self(Uuid::now_v7())
    }

    fn from_uuid(value: Uuid) -> Option<Self> {
        if value.get_version() != Some(Version::SortRand) || value.get_variant() != Variant::RFC4122
        {
            return None;
        }
        Some(Self(value))
    }

    const fn value(self) -> Uuid {
        self.0
    }

    pub(crate) fn as_text(&self) -> String {
        self.value().hyphenated().to_string()
    }
}

/// Canonical Machine Hardware ID derived by the Client's `UUIDv5` recipe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MachineHardwareId(Uuid);

impl MachineHardwareId {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        let parsed = Uuid::parse_str(value).ok()?;
        if parsed.hyphenated().to_string() != value
            || parsed.get_version() != Some(Version::Sha1)
            || parsed.get_variant() != Variant::RFC4122
        {
            return None;
        }
        Some(Self(parsed))
    }

    pub(crate) fn as_text(&self) -> String {
        self.0.hyphenated().to_string()
    }
}

/// Exact Ed25519 public-key bytes selected by the verified proof boundary.
///
/// This type owns only the fixed byte representation. The protocol verifier is
/// responsible for rejecting malformed or weak Ed25519 points before these
/// bytes can become an authority candidate.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct ControlPublicKey([u8; 32]);

impl ControlPublicKey {
    pub(crate) fn parse(value: &[u8]) -> Option<Self> {
        Some(Self(value.try_into().ok()?))
    }

    pub(crate) const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for ControlPublicKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ControlPublicKey([REDACTED])")
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Snafu)]
pub(crate) enum ActivationError {
    #[snafu(display("the candidate control key cannot become current authority"))]
    CandidateKeyRejected,
    #[snafu(display("persisted Device authority facts are invalid"))]
    InvalidAuthorityFacts,
    #[snafu(display("Device authority persistence failed"))]
    AuthorityPersistenceFailed,
}

impl From<PersistenceError> for ActivationError {
    fn from(error: PersistenceError) -> Self {
        match error {
            PersistenceError::InvalidPersistedData => Self::InvalidAuthorityFacts,
            PersistenceError::OperationFailed => Self::AuthorityPersistenceFailed,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeviceState {
    Enabled,
    Disabled,
    Revoked,
}

impl DeviceState {
    pub(crate) const fn is_enabled(self) -> bool {
        matches!(self, Self::Enabled)
    }

    pub(in crate::component::device) fn from_persisted(value: &str) -> Option<Self> {
        match value {
            "enabled" => Some(Self::Enabled),
            "disabled" => Some(Self::Disabled),
            "revoked" => Some(Self::Revoked),
            _ => None,
        }
    }

    pub(in crate::component::device) const fn as_persisted(self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
            Self::Revoked => "revoked",
        }
    }
}

/// The closed `devices.evidence_quality` vocabulary owned by this component.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EvidenceQuality {
    Medium,
    Strong,
}

impl EvidenceQuality {
    pub(in crate::component::device) fn from_persisted(value: &str) -> Option<Self> {
        match value {
            "medium" => Some(Self::Medium),
            "strong" => Some(Self::Strong),
            _ => None,
        }
    }

    pub(in crate::component::device) const fn as_persisted(self) -> &'static str {
        match self {
            Self::Medium => "medium",
            Self::Strong => "strong",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ControlAuthority {
    device_id: DeviceId,
    control_public_key: ControlPublicKey,
    device_state: DeviceState,
}

impl ControlAuthority {
    pub(crate) const fn new(
        device_id: DeviceId,
        control_public_key: ControlPublicKey,
        device_state: DeviceState,
    ) -> Option<Self> {
        if matches!(device_state, DeviceState::Revoked) {
            return None;
        }
        Some(Self {
            device_id,
            control_public_key,
            device_state,
        })
    }

    pub(crate) const fn device_id(self) -> DeviceId {
        self.device_id
    }

    pub(crate) const fn control_public_key(self) -> ControlPublicKey {
        self.control_public_key
    }

    pub(crate) const fn device_state(self) -> DeviceState {
        self.device_state
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LifecycleOutcome {
    Changed,
    Unchanged,
    RejectedTerminal,
}

/// Non-secret durable Device fields shown by the Operator Panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DeviceProjection {
    device_id: DeviceId,
    machine_hardware_id: MachineHardwareId,
    evidence_quality: EvidenceQuality,
    state: DeviceState,
    created_at_unix_ms: u64,
}

impl DeviceProjection {
    pub(in crate::component::device) fn from_persisted(
        device_id: &str,
        machine_hardware_id: &str,
        evidence_quality: &str,
        state: &str,
        created_at_unix_ms: i64,
    ) -> Result<Self, PersistenceError> {
        Ok(Self {
            device_id: DeviceId::parse(device_id).ok_or(PersistenceError::InvalidPersistedData)?,
            machine_hardware_id: MachineHardwareId::parse(machine_hardware_id)
                .ok_or(PersistenceError::InvalidPersistedData)?,
            evidence_quality: EvidenceQuality::from_persisted(evidence_quality)
                .ok_or(PersistenceError::InvalidPersistedData)?,
            state: DeviceState::from_persisted(state)
                .ok_or(PersistenceError::InvalidPersistedData)?,
            created_at_unix_ms: u64::try_from(created_at_unix_ms)
                .ok()
                .filter(|value| *value > 0)
                .ok_or(PersistenceError::InvalidPersistedData)?,
        })
    }

    pub(crate) const fn device_id(self) -> DeviceId {
        self.device_id
    }

    pub(crate) const fn machine_hardware_id(self) -> MachineHardwareId {
        self.machine_hardware_id
    }

    pub(crate) const fn evidence_quality(self) -> EvidenceQuality {
        self.evidence_quality
    }

    pub(crate) const fn state(self) -> DeviceState {
        self.state
    }

    pub(crate) const fn created_at_unix_ms(self) -> u64 {
        self.created_at_unix_ms
    }
}

pub(in crate::component::device) struct DeviceRecord {
    device_id: DeviceId,
    state: DeviceState,
}

impl DeviceRecord {
    pub(in crate::component::device) fn from_persisted(
        device_id: &str,
        state: &str,
    ) -> Result<Self, PersistenceError> {
        Ok(Self {
            device_id: DeviceId::parse(device_id).ok_or(PersistenceError::InvalidPersistedData)?,
            state: DeviceState::from_persisted(state)
                .ok_or(PersistenceError::InvalidPersistedData)?,
        })
    }

    pub(in crate::component::device) const fn device_id(&self) -> DeviceId {
        self.device_id
    }

    pub(in crate::component::device) const fn state(&self) -> DeviceState {
        self.state
    }
}

#[cfg(test)]
mod tests {
    use crate::db::PersistenceError;

    use super::{
        ControlPublicKey, DeviceError, DeviceId, DeviceState, EvidenceQuality, MachineHardwareId,
    };

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
    fn machine_hardware_id_accepts_only_canonical_uuid_v5_text() {
        const CANONICAL_V5: &str = "a9aa9d04-3ece-5567-8260-910930ff5e03";
        assert_eq!(
            MachineHardwareId::parse(CANONICAL_V5).map(|value| value.as_text()),
            Some(CANONICAL_V5.to_owned())
        );
        for invalid in [
            "A9AA9D04-3ECE-5567-8260-910930FF5E03",
            "a9aa9d043ece55678260910930ff5e03",
            "01900000-0000-7000-8000-000000000001",
            "550e8400-e29b-41d4-a716-446655440000",
        ] {
            assert!(MachineHardwareId::parse(invalid).is_none());
        }
    }

    #[test]
    fn control_public_key_has_exact_length_and_redacted_debug() {
        let canary = [0xA5; 32];
        let key = ControlPublicKey::parse(&canary)
            .unwrap_or_else(|| panic!("a fixed-size control public key was rejected"));
        assert_eq!(key.as_bytes(), &canary);
        assert_eq!(format!("{key:?}"), "ControlPublicKey([REDACTED])");
        assert!(ControlPublicKey::parse(&canary[..31]).is_none());
        assert!(ControlPublicKey::parse(&[0_u8; 33]).is_none());
    }

    #[test]
    fn device_state_persisted_vocabulary_roundtrips_exhaustively() {
        for (persisted, state) in [
            ("enabled", DeviceState::Enabled),
            ("disabled", DeviceState::Disabled),
            ("revoked", DeviceState::Revoked),
        ] {
            assert_eq!(DeviceState::from_persisted(persisted), Some(state));
            assert_eq!(state.as_persisted(), persisted);
        }
        assert!(DeviceState::Enabled.is_enabled());
        assert!(!DeviceState::Disabled.is_enabled());
        assert!(!DeviceState::Revoked.is_enabled());

        for invalid in ["", "Enabled", "disabled ", "active", "weak"] {
            assert!(DeviceState::from_persisted(invalid).is_none());
        }
    }

    #[test]
    fn evidence_quality_persisted_vocabulary_roundtrips_exhaustively() {
        for (persisted, quality) in [
            ("medium", EvidenceQuality::Medium),
            ("strong", EvidenceQuality::Strong),
        ] {
            assert_eq!(EvidenceQuality::from_persisted(persisted), Some(quality));
            assert_eq!(quality.as_persisted(), persisted);
        }

        for invalid in ["", "Strong", "medium ", "weak", "unknown", "enabled"] {
            assert!(EvidenceQuality::from_persisted(invalid).is_none());
        }
    }
}
