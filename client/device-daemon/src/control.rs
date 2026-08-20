use std::path::Path;

use snafu::Snafu;
use uuid::Uuid;

mod key;
mod manifest;

const CONTROL_KEY_NAME: &str = "control-key-1.pk8";
const CONTROL_MANIFEST_NAME: &str = "manifest.json";

#[derive(Debug, Snafu)]
pub enum DormantControlIdentityError {
    #[snafu(display("the dormant Device control private key is absent or invalid"))]
    ControlKey,

    #[snafu(display("entropy for the dormant Device control private key is unavailable"))]
    ControlKeyEntropy,

    #[snafu(display("the dormant Device control private key could not be encoded"))]
    ControlKeyEncoding,

    #[snafu(display("the dormant Device control private key could not be persisted"))]
    ControlKeyPersistence,

    #[snafu(display("the dormant Device control manifest is absent or invalid"))]
    Manifest,

    #[snafu(display("the dormant Device control manifest could not be serialized"))]
    ManifestSerialization,

    #[snafu(display("the dormant Device control manifest could not be persisted"))]
    ManifestPersistence,
}

pub(crate) fn ensure_dormant_identity(
    control_directory: &Path,
    machine_hardware_id: Uuid,
) -> Result<(), DormantControlIdentityError> {
    if machine_hardware_id.get_version_num() != 5 {
        return Err(DormantControlIdentityError::Manifest);
    }

    let key_path = control_directory.join(CONTROL_KEY_NAME);
    let manifest_path = control_directory.join(CONTROL_MANIFEST_NAME);
    let control_key = key::load(&key_path)?;
    let stored_manifest = manifest::load(&manifest_path)?;
    reconcile_dormant_identity(
        &key_path,
        &manifest_path,
        machine_hardware_id,
        control_key,
        stored_manifest,
    )
}

fn reconcile_dormant_identity(
    key_path: &Path,
    manifest_path: &Path,
    machine_hardware_id: Uuid,
    control_key: Option<key::ControlKey>,
    stored_manifest: Option<manifest::ControlManifest>,
) -> Result<(), DormantControlIdentityError> {
    match (control_key, stored_manifest) {
        (None, Some(stored_manifest)) => key::load(key_path)?
            .ok_or(DormantControlIdentityError::ControlKey)
            .and_then(|control_key| {
                stored_manifest.validate(machine_hardware_id, control_key.public_key())
            }),
        (Some(control_key), Some(stored_manifest)) => {
            stored_manifest.validate(machine_hardware_id, control_key.public_key())
        }
        (Some(control_key), None) => manifest::create_or_validate(
            manifest_path,
            machine_hardware_id,
            control_key.public_key(),
        ),
        (None, None) => {
            let control_key = key::create_or_load(key_path)?;
            manifest::create_or_validate(
                manifest_path,
                machine_hardware_id,
                control_key.public_key(),
            )
        }
    }
}
