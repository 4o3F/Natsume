use std::{fs, io::ErrorKind, os::unix::fs::MetadataExt as _, path::Path};

use natsume_device_protocol::ControlKeyId;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    atomic_write::{AtomicWriteError, WritePolicy, atomic_write},
    canonical_uuid,
};

use super::DormantControlIdentityError;

const CONTROL_MANIFEST_MODE: u32 = 0o600;
const CONTROL_MANIFEST_FORMAT_VERSION: u32 = 1;
const CONTROL_KEY_GENERATION: u32 = 1;

#[derive(Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum ControlManifestState {
    Dormant,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ControlManifest {
    format_version: u32,
    machine_hardware_id: String,
    control_key_generation: u32,
    control_key_id: String,
    state: ControlManifestState,
}

impl ControlManifest {
    pub(super) fn validate(
        &self,
        machine_hardware_id: Uuid,
        control_public_key: [u8; 32],
    ) -> Result<(), DormantControlIdentityError> {
        let stored_machine_hardware_id = canonical_uuid(&self.machine_hardware_id)
            .filter(|stored| stored.get_version_num() == 5)
            .ok_or(DormantControlIdentityError::Manifest)?;
        let expected_key_id = ControlKeyId::derive(control_public_key);
        if self.format_version != CONTROL_MANIFEST_FORMAT_VERSION
            || stored_machine_hardware_id != machine_hardware_id
            || self.control_key_generation != CONTROL_KEY_GENERATION
            || self.control_key_id != hex::encode(expected_key_id.as_bytes())
            || self.state != ControlManifestState::Dormant
        {
            return Err(DormantControlIdentityError::Manifest);
        }
        Ok(())
    }
}

pub(super) fn load(path: &Path) -> Result<Option<ControlManifest>, DormantControlIdentityError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(DormantControlIdentityError::Manifest),
    };
    if !metadata.is_file() || metadata.mode() & 0o777 != CONTROL_MANIFEST_MODE {
        return Err(DormantControlIdentityError::Manifest);
    }
    let bytes = fs::read(path).map_err(|_| DormantControlIdentityError::Manifest)?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| DormantControlIdentityError::Manifest)
}

pub(super) fn create_or_validate(
    path: &Path,
    machine_hardware_id: Uuid,
    control_public_key: [u8; 32],
) -> Result<(), DormantControlIdentityError> {
    let key_id = ControlKeyId::derive(control_public_key);
    let manifest = ControlManifest {
        format_version: CONTROL_MANIFEST_FORMAT_VERSION,
        machine_hardware_id: machine_hardware_id.to_string(),
        control_key_generation: CONTROL_KEY_GENERATION,
        control_key_id: hex::encode(key_id.as_bytes()),
        state: ControlManifestState::Dormant,
    };
    manifest.validate(machine_hardware_id, control_public_key)?;
    let bytes = serde_json::to_vec(&manifest)
        .map_err(|_| DormantControlIdentityError::ManifestSerialization)?;

    match atomic_write(path, &bytes, CONTROL_MANIFEST_MODE, WritePolicy::CreateOnly) {
        Ok(()) => Ok(()),
        Err(AtomicWriteError::Rename) => load(path)?
            .ok_or(DormantControlIdentityError::ManifestPersistence)?
            .validate(machine_hardware_id, control_public_key),
        Err(_) => Err(DormantControlIdentityError::ManifestPersistence),
    }
}

#[cfg(test)]
mod tests;
