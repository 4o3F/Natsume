use std::{fs, io::ErrorKind, os::unix::fs::MetadataExt as _, path::Path};

use serde::{Deserialize, Serialize};
use uuid::{Uuid, Variant, Version};

use crate::{
    atomic_write::{AtomicWriteError, WritePolicy, atomic_write},
    canonical_uuid, canonical_uuid_v7,
};

use super::ControlIdentityError;

const CONTROL_MANIFEST_MODE: u32 = 0o600;
const CONTROL_MANIFEST_FORMAT_VERSION: u32 = 1;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ControlManifest {
    format_version: u32,
    machine_hardware_id: String,
    control_public_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    device_id: Option<String>,
}

impl ControlManifest {
    pub(super) fn validate(
        &self,
        machine_hardware_id: Uuid,
        control_public_key: [u8; 32],
    ) -> Result<(), ControlIdentityError> {
        let stored_machine_hardware_id = canonical_uuid(&self.machine_hardware_id)
            .filter(|stored| {
                stored.get_version() == Some(Version::Sha1)
                    && stored.get_variant() == Variant::RFC4122
            })
            .ok_or(ControlIdentityError::Manifest)?;
        if self.format_version != CONTROL_MANIFEST_FORMAT_VERSION
            || stored_machine_hardware_id != machine_hardware_id
            || self.control_public_key != hex::encode(control_public_key)
            || self
                .device_id
                .as_deref()
                .is_some_and(|device_id| canonical_uuid_v7(device_id).is_none())
        {
            return Err(ControlIdentityError::Manifest);
        }
        Ok(())
    }

    pub(super) fn device_id(&self) -> Option<Uuid> {
        self.device_id.as_deref().and_then(canonical_uuid_v7)
    }

    pub(super) fn activate(
        &mut self,
        path: &Path,
        device_id: Uuid,
    ) -> Result<(), ControlIdentityError> {
        if self.device_id.is_some() {
            return (self.device_id() == Some(device_id))
                .then_some(())
                .ok_or(ControlIdentityError::Manifest);
        }

        let mut active = self.clone();
        active.device_id = Some(device_id.hyphenated().to_string());
        let bytes =
            serde_json::to_vec(&active).map_err(|_| ControlIdentityError::ManifestSerialization)?;
        atomic_write(path, &bytes, CONTROL_MANIFEST_MODE, WritePolicy::Replace)
            .map_err(|_| ControlIdentityError::ManifestPersistence)?;
        *self = active;
        Ok(())
    }
}

pub(super) fn load(path: &Path) -> Result<Option<ControlManifest>, ControlIdentityError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(ControlIdentityError::Manifest),
    };
    if !metadata.is_file() || metadata.mode() & 0o777 != CONTROL_MANIFEST_MODE {
        return Err(ControlIdentityError::Manifest);
    }
    let bytes = fs::read(path).map_err(|_| ControlIdentityError::Manifest)?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| ControlIdentityError::Manifest)
}

pub(super) fn create_or_validate(
    path: &Path,
    machine_hardware_id: Uuid,
    control_public_key: [u8; 32],
) -> Result<ControlManifest, ControlIdentityError> {
    let manifest = ControlManifest {
        format_version: CONTROL_MANIFEST_FORMAT_VERSION,
        machine_hardware_id: machine_hardware_id.to_string(),
        control_public_key: hex::encode(control_public_key),
        device_id: None,
    };
    let bytes =
        serde_json::to_vec(&manifest).map_err(|_| ControlIdentityError::ManifestSerialization)?;

    match atomic_write(path, &bytes, CONTROL_MANIFEST_MODE, WritePolicy::CreateOnly) {
        Ok(()) => Ok(manifest),
        Err(AtomicWriteError::Conflict) => load(path)?
            .ok_or(ControlIdentityError::ManifestPersistence)
            .and_then(|stored| {
                stored.validate(machine_hardware_id, control_public_key)?;
                Ok(stored)
            }),
        Err(AtomicWriteError::Failed) => Err(ControlIdentityError::ManifestPersistence),
    }
}

#[cfg(test)]
mod tests;
