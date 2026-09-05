use std::{fs, io::ErrorKind, os::unix::fs::MetadataExt as _, path::Path};

use ed25519_dalek::{
    SigningKey,
    pkcs8::{DecodePrivateKey as _, EncodePrivateKey as _},
};
use natsume_device_protocol::{
    generated::{ClientProof, ServerChallenge},
    sign_client_proof,
};
use zeroize::{Zeroize as _, Zeroizing};

use crate::atomic_write::{AtomicWriteError, WritePolicy, atomic_write};

use super::ControlIdentityError;

const CONTROL_KEY_MODE: u32 = 0o600;

pub(super) struct ControlKey {
    signing_key: SigningKey,
}

impl ControlKey {
    pub(super) fn public_key(&self) -> [u8; 32] {
        self.signing_key.verifying_key().to_bytes()
    }

    pub(super) fn sign_proof(
        &self,
        challenge: &ServerChallenge,
        proof: ClientProof,
    ) -> ClientProof {
        sign_client_proof(&self.signing_key, challenge, proof)
    }
}

pub(super) fn load(path: &Path) -> Result<Option<ControlKey>, ControlIdentityError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(ControlIdentityError::ControlKey),
    };
    if !metadata.is_file() || metadata.mode() & 0o777 != CONTROL_KEY_MODE {
        return Err(ControlIdentityError::ControlKey);
    }

    let encoded = Zeroizing::new(fs::read(path).map_err(|_| ControlIdentityError::ControlKey)?);
    let signing_key =
        SigningKey::from_pkcs8_der(&encoded).map_err(|_| ControlIdentityError::ControlKey)?;
    Ok(Some(ControlKey { signing_key }))
}

pub(super) fn create_or_load(path: &Path) -> Result<ControlKey, ControlIdentityError> {
    let mut seed = [0_u8; 32];
    if getrandom::fill(&mut seed).is_err() {
        seed.zeroize();
        return Err(ControlIdentityError::ControlKeyEntropy);
    }
    let signing_key = SigningKey::from_bytes(&seed);
    seed.zeroize();

    let encoded = signing_key
        .to_pkcs8_der()
        .map_err(|_| ControlIdentityError::ControlKeyEncoding)?;
    match atomic_write(
        path,
        encoded.as_bytes(),
        CONTROL_KEY_MODE,
        WritePolicy::CreateOnly,
    ) {
        Ok(()) => Ok(ControlKey { signing_key }),
        Err(AtomicWriteError::Conflict) => {
            load(path)?.ok_or(ControlIdentityError::ControlKeyPersistence)
        }
        Err(AtomicWriteError::Failed) => Err(ControlIdentityError::ControlKeyPersistence),
    }
}
