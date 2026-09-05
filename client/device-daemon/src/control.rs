use std::path::Path;

use natsume_device_protocol::generated::{
    ClientProof, EnrollmentAttempt, EnrollmentAuthority, EnrollmentEvidenceQuality, ResumeSession,
    ServerChallenge, client_proof,
};
use snafu::Snafu;
use uuid::{Uuid, Variant, Version};

use crate::canonical_uuid_v7;

pub(crate) mod connection;
mod enrollment;
mod key;
mod manifest;

const CONTROL_KEY_NAME: &str = "control-key-1.pk8";
const CONTROL_MANIFEST_NAME: &str = "manifest.json";

/// Fail-closed errors while loading or durably changing the Device control identity.
#[derive(Debug, Snafu)]
pub(crate) enum ControlIdentityError {
    #[snafu(display("the Device control private key is absent or invalid"))]
    ControlKey,

    #[snafu(display("entropy for the Device control private key is unavailable"))]
    ControlKeyEntropy,

    #[snafu(display("the Device control private key could not be encoded"))]
    ControlKeyEncoding,

    #[snafu(display("the Device control private key could not be persisted"))]
    ControlKeyPersistence,

    #[snafu(display("the Device control manifest is absent or invalid"))]
    Manifest,

    #[snafu(display("the Device control manifest could not be serialized"))]
    ManifestSerialization,

    #[snafu(display("the Device control manifest could not be persisted"))]
    ManifestPersistence,
}

/// Fatal local errors that prevent the control loop from safely retrying.
#[derive(Debug, Snafu)]
pub(crate) enum ControlLoopError {
    #[snafu(display("the Device control endpoint configuration is invalid"))]
    EndpointConfiguration,

    #[snafu(display("the Device control trust root is invalid"))]
    TrustRootConfiguration,

    #[snafu(display("the Device control TLS configuration is invalid"))]
    Tls,

    #[snafu(display("Device local access could not be deactivated"))]
    LocalDeactivation,

    #[snafu(display("the Device control authority could not be installed: {source}"))]
    AuthorityPersistence { source: ControlIdentityError },
}

/// Durable Device control identity used for Enrollment and Resume proofs.
///
/// The private key never leaves this type. A present Device ID means the exact
/// Enrollment authority was installed crash-safely and Resume is now required.
pub(crate) struct ControlIdentity {
    key: key::ControlKey,
    manifest: manifest::ControlManifest,
    manifest_path: std::path::PathBuf,
}

impl ControlIdentity {
    fn proof(
        &self,
        challenge: &ServerChallenge,
        machine_hardware_id: Uuid,
        evidence_quality: EnrollmentEvidenceQuality,
    ) -> ClientProof {
        let purpose = match self.manifest.device_id() {
            Some(_) => client_proof::Purpose::Resume(ResumeSession {}),
            None => client_proof::Purpose::Enrollment(EnrollmentAttempt {
                candidate_public_key: self.key.public_key().to_vec(),
                evidence_quality: evidence_quality.into(),
            }),
        };
        self.key.sign_proof(
            challenge,
            ClientProof {
                daemon_version: env!("CARGO_PKG_VERSION").to_owned(),
                agent_version: env!("CARGO_PKG_VERSION").to_owned(),
                machine_hardware_id: machine_hardware_id.hyphenated().to_string(),
                signature: Vec::new(),
                purpose: Some(purpose),
            },
        )
    }

    fn is_enrolling(&self) -> bool {
        self.manifest.device_id().is_none()
    }

    fn install_authority(
        &mut self,
        authority: &EnrollmentAuthority,
    ) -> Result<(), ControlIdentityError> {
        let device_id =
            canonical_uuid_v7(&authority.device_id).ok_or(ControlIdentityError::Manifest)?;
        self.manifest.activate(&self.manifest_path, device_id)
    }
}

pub(crate) fn load_or_create_identity(
    control_directory: &Path,
    machine_hardware_id: Uuid,
) -> Result<ControlIdentity, ControlIdentityError> {
    if machine_hardware_id.get_version() != Some(Version::Sha1)
        || machine_hardware_id.get_variant() != Variant::RFC4122
    {
        return Err(ControlIdentityError::Manifest);
    }

    let key_path = control_directory.join(CONTROL_KEY_NAME);
    let manifest_path = control_directory.join(CONTROL_MANIFEST_NAME);
    let control_key = key::load(&key_path)?;
    let stored_manifest = manifest::load(&manifest_path)?;
    reconcile_identity(
        &key_path,
        &manifest_path,
        machine_hardware_id,
        control_key,
        stored_manifest,
    )
}

fn reconcile_identity(
    key_path: &Path,
    manifest_path: &Path,
    machine_hardware_id: Uuid,
    control_key: Option<key::ControlKey>,
    stored_manifest: Option<manifest::ControlManifest>,
) -> Result<ControlIdentity, ControlIdentityError> {
    let key = match control_key {
        Some(key) => key,
        None if stored_manifest.is_some() => return Err(ControlIdentityError::ControlKey),
        None => key::create_or_load(key_path)?,
    };
    let manifest = match stored_manifest {
        Some(manifest) => {
            manifest.validate(machine_hardware_id, key.public_key())?;
            manifest
        }
        None => manifest::create_or_validate(manifest_path, machine_hardware_id, key.public_key())?,
    };
    Ok(ControlIdentity {
        key,
        manifest,
        manifest_path: manifest_path.to_owned(),
    })
}
