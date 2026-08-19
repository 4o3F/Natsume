use std::{fs, io::ErrorKind, os::unix::fs::MetadataExt as _, path::Path};

use ed25519_dalek::{
    SigningKey,
    pkcs8::{DecodePrivateKey as _, EncodePrivateKey as _},
};
use zeroize::{Zeroize as _, Zeroizing};

use crate::atomic_write::{AtomicWriteError, WritePolicy, atomic_write};

use super::DormantControlIdentityError;

const CONTROL_KEY_MODE: u32 = 0o600;

pub(super) struct ControlKey {
    signing_key: SigningKey,
}

impl ControlKey {
    pub(super) fn public_key(&self) -> [u8; 32] {
        self.signing_key.verifying_key().to_bytes()
    }
}

pub(super) fn load(path: &Path) -> Result<Option<ControlKey>, DormantControlIdentityError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(DormantControlIdentityError::ControlKey),
    };
    if !metadata.is_file() || metadata.mode() & 0o777 != CONTROL_KEY_MODE {
        return Err(DormantControlIdentityError::ControlKey);
    }

    let encoded =
        Zeroizing::new(fs::read(path).map_err(|_| DormantControlIdentityError::ControlKey)?);
    let signing_key = SigningKey::from_pkcs8_der(&encoded)
        .map_err(|_| DormantControlIdentityError::ControlKey)?;
    Ok(Some(ControlKey { signing_key }))
}

pub(super) fn create_or_load(path: &Path) -> Result<ControlKey, DormantControlIdentityError> {
    let mut seed = [0_u8; 32];
    if getrandom::fill(&mut seed).is_err() {
        seed.zeroize();
        return Err(DormantControlIdentityError::ControlKeyEntropy);
    }
    let signing_key = SigningKey::from_bytes(&seed);
    seed.zeroize();

    let encoded = signing_key
        .to_pkcs8_der()
        .map_err(|_| DormantControlIdentityError::ControlKeyEncoding)?;
    match atomic_write(
        path,
        encoded.as_bytes(),
        CONTROL_KEY_MODE,
        WritePolicy::CreateOnly,
    ) {
        Ok(()) => Ok(ControlKey { signing_key }),
        Err(AtomicWriteError::Rename) => {
            load(path)?.ok_or(DormantControlIdentityError::ControlKeyPersistence)
        }
        Err(_) => Err(DormantControlIdentityError::ControlKeyPersistence),
    }
}
