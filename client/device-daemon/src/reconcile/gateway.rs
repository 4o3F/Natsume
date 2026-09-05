use std::{
    fs::{self, File},
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
    sync::Mutex,
};

use crate::{
    atomic_write::{WritePolicy, atomic_write, durable_remove},
    canonical_uuid_v7,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use natsume_device_protocol::generated::{
    GatewayActualState, GatewayCredentialInput, GatewayState, GatewayTarget,
};
use rcgen::{CertificateParams, KeyPair, PKCS_ECDSA_P256_SHA256};
use rustls_pki_types::{CertificateDer, PrivatePkcs8KeyDer, pem::PemObject as _};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use x509_parser::{
    certificate::X509Certificate,
    certification_request::X509CertificationRequest,
    oid_registry::{OID_EC_P256, OID_KEY_TYPE_EC_PUBLIC_KEY, OID_SIG_ECDSA_WITH_SHA256},
    prelude::{FromDer as _, X509Version},
};
use zeroize::Zeroizing;

use super::{
    SnapshotError,
    caddy::{CaddyModeArtifact, CaddyObservation},
    check_cancellation,
};

const INPUT_FORMAT_VERSION: u32 = 1;
const ACTIVE_FORMAT_VERSION: u32 = 1;
const GATEWAY_INPUT_NAME: &str = "input.json";
const GATEWAY_KEY_NAME: &str = "key.pem";
const GATEWAY_CERTIFICATE_NAME: &str = "fullchain.pem";

/// Persistent, generation-bound Gateway input metadata.
///
/// The private key is stored in the adjacent owner-controlled key file. This record is written
/// only after that key is durable and is therefore the publication barrier for the exact CSR.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct GatewayInputArtifact {
    format_version: u32,
    credential_id: String,
    csr_der_base64: String,
}

/// Non-secret pointer to the current installed Gateway certificate generation.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ActiveGatewayArtifact {
    format_version: u32,
    credential_id: String,
}

/// Gateway input and target reconciler backed by generation-specific durable artifacts.
pub(super) struct GatewayReconciler {
    intent: Mutex<Option<Uuid>>,
    generations_directory: PathBuf,
    active_path: PathBuf,
}

/// Fixed paths and leaf identity for one validated Gateway credential generation.
pub(super) struct GatewayMaterial {
    pub(super) credential_id: String,
    pub(super) certificate_path: PathBuf,
    pub(super) private_key_path: PathBuf,
    pub(super) leaf_sha256: Vec<u8>,
}

/// Result of reconciling the Gateway certificate artifacts for one Server target.
pub(super) enum GatewayMaterialState {
    Restoring,
    RecoveryRequired,
    Available(GatewayMaterial),
}

/// Certificate grant parsed at the complete Server snapshot boundary.
#[derive(PartialEq)]
struct ValidatedGatewayCertificate {
    leaf_der: Vec<u8>,
    leaf_public_key: Vec<u8>,
}

/// Gateway target accepted at the complete Server snapshot boundary.
#[derive(PartialEq)]
pub(super) struct ValidatedGatewayTarget {
    pub(super) credential_id: Uuid,
    certificate: Option<ValidatedGatewayCertificate>,
}

pub(super) fn validate_target(target: GatewayTarget) -> Option<ValidatedGatewayTarget> {
    let credential_id = canonical_uuid_v7(&target.credential_id)?;
    let certificate = match target.certificate {
        None => None,
        Some(grant) => {
            let (remainder, leaf) = X509Certificate::from_der(&grant.gateway_leaf_der).ok()?;
            if !remainder.is_empty() {
                return None;
            }
            Some(ValidatedGatewayCertificate {
                leaf_public_key: leaf.public_key().subject_public_key.data.as_ref().to_vec(),
                leaf_der: grant.gateway_leaf_der,
            })
        }
    };
    Some(ValidatedGatewayTarget {
        credential_id,
        certificate,
    })
}

impl GatewayReconciler {
    pub(super) fn production() -> Self {
        Self {
            intent: Mutex::new(None),
            generations_directory: PathBuf::from("/var/lib/natsume/keys/gateway"),
            active_path: PathBuf::from("/var/lib/natsume/keys/gateway-active.json"),
        }
    }

    pub(super) fn current_input(
        &self,
        target: &ValidatedGatewayTarget,
    ) -> Result<GatewayCredentialInput, SnapshotError> {
        let mut intent = self.intent.lock().map_err(|_| SnapshotError::Artifact)?;
        let credential_id = target.credential_id;
        self.clear_replaced_active(credential_id)?;
        let input = if target.certificate.is_some() {
            self.read_input(credential_id)
                .unwrap_or_else(|| GatewayCredentialInput {
                    credential_id: credential_id.hyphenated().to_string(),
                    gateway_csr_der: None,
                })
        } else {
            self.load_or_create_input(credential_id)?
        };
        self.remove_other_generations(credential_id)?;
        *intent = Some(credential_id);
        Ok(input)
    }

    fn load_or_create_input(
        &self,
        credential_id: Uuid,
    ) -> Result<GatewayCredentialInput, SnapshotError> {
        let credential_id_text = credential_id.hyphenated().to_string();
        let generation_directory = self.generation_directory(credential_id);
        let input_path = generation_directory.join(GATEWAY_INPUT_NAME);
        let key_path = generation_directory.join(GATEWAY_KEY_NAME);

        if input_path.exists() {
            return Ok(GatewayCredentialInput {
                credential_id: credential_id_text,
                gateway_csr_der: read_input_and_key(&input_path, &key_path, credential_id)
                    .map(|(csr, _)| csr),
            });
        }

        fs::create_dir_all(&generation_directory).map_err(|_| SnapshotError::Artifact)?;
        fs::set_permissions(&generation_directory, fs::Permissions::from_mode(0o2750))
            .map_err(|_| SnapshotError::Artifact)?;
        // Both runtime-created directory entries must be durable before publishing the CSR.
        let generations_parent = self
            .generations_directory
            .parent()
            .ok_or(SnapshotError::Artifact)?;
        File::open(generations_parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| SnapshotError::Artifact)?;
        File::open(&self.generations_directory)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| SnapshotError::Artifact)?;
        let key = match fs::read(&key_path) {
            Ok(encoded) => {
                let encoded = Zeroizing::new(encoded);
                let Some(key) = parse_key(&encoded) else {
                    return Ok(GatewayCredentialInput {
                        credential_id: credential_id_text,
                        gateway_csr_der: None,
                    });
                };
                key
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)
                    .map_err(|_| SnapshotError::Artifact)?;
                let pem = pem_private_key(key.serialized_der());
                atomic_write(&key_path, pem.as_bytes(), 0o640, WritePolicy::CreateOnly)
                    .map_err(|_| SnapshotError::Artifact)?;
                key
            }
            Err(_) => return Err(SnapshotError::Artifact),
        };
        let csr_der = CertificateParams::default()
            .serialize_request(&key)
            .map_err(|_| SnapshotError::Artifact)?
            .der()
            .to_vec();
        let artifact = GatewayInputArtifact {
            format_version: INPUT_FORMAT_VERSION,
            credential_id: credential_id_text.clone(),
            csr_der_base64: STANDARD.encode(&csr_der),
        };
        let encoded = serde_json::to_vec(&artifact).map_err(|_| SnapshotError::Artifact)?;
        atomic_write(&input_path, &encoded, 0o600, WritePolicy::CreateOnly)
            .map_err(|_| SnapshotError::Artifact)?;
        Ok(GatewayCredentialInput {
            credential_id: credential_id_text,
            gateway_csr_der: Some(csr_der),
        })
    }

    fn clear_replaced_active(&self, credential_id: Uuid) -> Result<(), SnapshotError> {
        if self.active_credential_id() == Some(credential_id) {
            return Ok(());
        }
        durable_remove(&self.active_path).map_err(|_| SnapshotError::Artifact)
    }

    fn remove_other_generations(&self, credential_id: Uuid) -> Result<(), SnapshotError> {
        let current = credential_id.hyphenated().to_string();
        let mut removed = false;
        let entries = match fs::read_dir(&self.generations_directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) => return Err(SnapshotError::Artifact),
        };
        for entry in entries {
            let entry = entry.map_err(|_| SnapshotError::Artifact)?;
            if entry.file_name() == current.as_str() {
                continue;
            }
            let file_type = entry.file_type().map_err(|_| SnapshotError::Artifact)?;
            if file_type.is_dir() {
                fs::remove_dir_all(entry.path()).map_err(|_| SnapshotError::Artifact)?;
            } else {
                fs::remove_file(entry.path()).map_err(|_| SnapshotError::Artifact)?;
            }
            removed = true;
        }
        if removed {
            File::open(&self.generations_directory)
                .and_then(|directory| directory.sync_all())
                .map_err(|_| SnapshotError::Artifact)?;
        }
        Ok(())
    }

    /// Reads the input owned by the most recently completed Gateway intent.
    pub(super) fn observed_input(&self) -> Result<Option<GatewayCredentialInput>, SnapshotError> {
        let intent = self.intent.lock().map_err(|_| SnapshotError::Artifact)?;
        Ok(intent.and_then(|credential_id| self.read_input(credential_id)))
    }

    fn read_input(&self, credential_id: Uuid) -> Option<GatewayCredentialInput> {
        let credential_id_text = credential_id.hyphenated().to_string();
        let generation_directory = self.generation_directory(credential_id);
        let input_path = generation_directory.join(GATEWAY_INPUT_NAME);
        let key_path = generation_directory.join(GATEWAY_KEY_NAME);
        if !input_path.exists() {
            let encoded_key = Zeroizing::new(fs::read(&key_path).ok()?);
            return parse_key(&encoded_key)
                .is_none()
                .then_some(GatewayCredentialInput {
                    credential_id: credential_id_text,
                    gateway_csr_der: None,
                });
        }
        Some(GatewayCredentialInput {
            credential_id: credential_id_text,
            gateway_csr_der: read_input_and_key(&input_path, &key_path, credential_id)
                .map(|(csr, _)| csr),
        })
    }

    pub(super) fn active_material(&self) -> Option<GatewayMaterial> {
        self.installed_material(self.active_credential_id()?)
    }

    fn active_credential_id(&self) -> Option<Uuid> {
        let encoded = fs::read(&self.active_path).ok()?;
        let artifact = serde_json::from_slice::<ActiveGatewayArtifact>(&encoded).ok()?;
        if artifact.format_version != ACTIVE_FORMAT_VERSION {
            return None;
        }
        canonical_uuid_v7(&artifact.credential_id)
    }

    fn installed_material(&self, credential_id: Uuid) -> Option<GatewayMaterial> {
        let directory = self.generation_directory(credential_id);
        let input_path = directory.join(GATEWAY_INPUT_NAME);
        let certificate_path = directory.join(GATEWAY_CERTIFICATE_NAME);
        let private_key_path = directory.join(GATEWAY_KEY_NAME);
        let (_, key) = read_input_and_key(&input_path, &private_key_path, credential_id)?;
        let encoded_certificate = fs::read(&certificate_path).ok()?;
        let mut certificates = CertificateDer::pem_slice_iter(&encoded_certificate);
        let leaf = certificates.next()?.ok()?;
        if certificates.next().is_some() {
            return None;
        }
        valid_leaf_for_key(&leaf, &key).then_some(GatewayMaterial {
            credential_id: credential_id.hyphenated().to_string(),
            certificate_path,
            private_key_path,
            leaf_sha256: Sha256::digest(leaf.as_ref()).to_vec(),
        })
    }

    pub(super) fn current_material(&self, target: &ValidatedGatewayTarget) -> GatewayMaterialState {
        if target.certificate.is_none() {
            return GatewayMaterialState::Restoring;
        }
        if self.active_credential_id() != Some(target.credential_id) {
            return GatewayMaterialState::RecoveryRequired;
        }
        self.installed_target_material(target).map_or(
            GatewayMaterialState::RecoveryRequired,
            GatewayMaterialState::Available,
        )
    }

    fn installed_target_material(
        &self,
        target: &ValidatedGatewayTarget,
    ) -> Option<GatewayMaterial> {
        let grant = target.certificate.as_ref()?;
        let material = self.installed_material(target.credential_id)?;
        let encoded = fs::read(&material.certificate_path).ok()?;
        (encoded == pem_certificate(&grant.leaf_der).as_bytes()).then_some(material)
    }

    fn generation_directory(&self, credential_id: Uuid) -> PathBuf {
        self.generations_directory
            .join(credential_id.hyphenated().to_string())
    }

    fn install_certificate(&self, target: &ValidatedGatewayTarget) -> Option<GatewayMaterial> {
        let credential_id = target.credential_id;
        let grant = target.certificate.as_ref()?;
        let directory = self.generation_directory(credential_id);
        let input_path = directory.join(GATEWAY_INPUT_NAME);
        let key_path = directory.join(GATEWAY_KEY_NAME);
        let (_, key) = read_input_and_key(&input_path, &key_path, credential_id)?;
        if grant.leaf_public_key.as_slice() != key.public_key_raw() {
            return None;
        }
        let pem = pem_certificate(&grant.leaf_der);
        let certificate_path = directory.join(GATEWAY_CERTIFICATE_NAME);
        atomic_write(
            &certificate_path,
            pem.as_bytes(),
            0o640,
            WritePolicy::Replace,
        )
        .ok()?;
        Some(GatewayMaterial {
            credential_id: credential_id.hyphenated().to_string(),
            certificate_path,
            private_key_path: key_path,
            leaf_sha256: Sha256::digest(&grant.leaf_der).to_vec(),
        })
    }

    pub(super) fn reconcile(
        &self,
        target: &ValidatedGatewayTarget,
        cancellation: &CancellationToken,
    ) -> Result<GatewayMaterialState, SnapshotError> {
        let credential_id = target.credential_id.hyphenated().to_string();
        check_cancellation(cancellation)?;
        if target.certificate.is_none() {
            return Ok(GatewayMaterialState::Restoring);
        }
        let Some(material) = self
            .installed_target_material(target)
            .or_else(|| self.install_certificate(target))
        else {
            return Ok(GatewayMaterialState::RecoveryRequired);
        };
        check_cancellation(cancellation)?;
        if self.active_credential_id() != Some(target.credential_id) {
            let active = ActiveGatewayArtifact {
                format_version: ACTIVE_FORMAT_VERSION,
                credential_id,
            };
            let encoded = serde_json::to_vec(&active).map_err(|_| SnapshotError::Artifact)?;
            atomic_write(&self.active_path, &encoded, 0o600, WritePolicy::Replace)
                .map_err(|_| SnapshotError::Artifact)?;
        }
        Ok(GatewayMaterialState::Available(material))
    }

    pub(super) fn observe(&self, caddy: &CaddyObservation) -> GatewayActualState {
        let Some(material) = self.active_material() else {
            return GatewayActualState {
                credential_id: None,
                state: GatewayState::Absent.into(),
                gateway_leaf_sha256: None,
            };
        };
        actual_for_material(&material, caddy)
    }
}

pub(super) fn actual(
    target: &ValidatedGatewayTarget,
    material: &GatewayMaterialState,
    caddy: &CaddyObservation,
) -> GatewayActualState {
    let credential_id = target.credential_id.hyphenated().to_string();
    match material {
        GatewayMaterialState::Restoring => GatewayActualState {
            credential_id: Some(credential_id),
            state: GatewayState::Restoring.into(),
            gateway_leaf_sha256: None,
        },
        GatewayMaterialState::RecoveryRequired => recovery_required(&credential_id),
        GatewayMaterialState::Available(material) => actual_for_material(material, caddy),
    }
}

fn actual_for_material(material: &GatewayMaterial, caddy: &CaddyObservation) -> GatewayActualState {
    let Some(mode) = caddy.mode.as_ref() else {
        return GatewayActualState {
            credential_id: Some(material.credential_id.clone()),
            state: GatewayState::Restoring.into(),
            gateway_leaf_sha256: None,
        };
    };
    let mode_credential_id = match mode {
        CaddyModeArtifact::Blocked { credential_id, .. } => credential_id.as_deref(),
        CaddyModeArtifact::Ready { credential_id, .. } => Some(credential_id.as_str()),
    };
    if mode_credential_id != Some(material.credential_id.as_str())
        || caddy.gateway_leaf_sha256.as_deref() != Some(material.leaf_sha256.as_slice())
    {
        return recovery_required(&material.credential_id);
    }
    let state = match mode {
        CaddyModeArtifact::Blocked { .. } => GatewayState::Blocked,
        CaddyModeArtifact::Ready { .. } => GatewayState::Ready,
    };
    GatewayActualState {
        credential_id: Some(material.credential_id.clone()),
        state: state.into(),
        gateway_leaf_sha256: Some(material.leaf_sha256.clone()),
    }
}

fn read_input_and_key(
    input_path: &Path,
    key_path: &Path,
    credential_id: Uuid,
) -> Option<(Vec<u8>, KeyPair)> {
    let encoded = fs::read(input_path).ok()?;
    let artifact = serde_json::from_slice::<GatewayInputArtifact>(&encoded).ok()?;
    if artifact.format_version != INPUT_FORMAT_VERSION
        || artifact.credential_id != credential_id.hyphenated().to_string()
    {
        return None;
    }
    let csr_der = STANDARD.decode(artifact.csr_der_base64).ok()?;
    let encoded_key = Zeroizing::new(fs::read(key_path).ok()?);
    let key = parse_key(&encoded_key)?;
    valid_csr_for_key(&csr_der, &key).then_some((csr_der, key))
}

fn valid_csr_for_key(csr_der: &[u8], key: &KeyPair) -> bool {
    let Ok((remainder, csr)) = X509CertificationRequest::from_der(csr_der) else {
        return false;
    };
    let public_key = &csr.certification_request_info.subject_pki;
    remainder.is_empty()
        && csr.certification_request_info.version == X509Version::V1
        && csr.signature_algorithm.algorithm == OID_SIG_ECDSA_WITH_SHA256
        && csr.verify_signature().is_ok()
        && public_key.algorithm.algorithm == OID_KEY_TYPE_EC_PUBLIC_KEY
        && public_key
            .algorithm
            .parameters
            .as_ref()
            .and_then(|parameters| parameters.as_oid().ok())
            .is_some_and(|curve| curve == OID_EC_P256)
        && public_key.subject_public_key.data.as_ref() == key.public_key_raw()
}

fn valid_leaf_for_key(leaf_der: &[u8], key: &KeyPair) -> bool {
    let Ok((remainder, certificate)) = X509Certificate::from_der(leaf_der) else {
        return false;
    };
    remainder.is_empty()
        && certificate.public_key().subject_public_key.data.as_ref() == key.public_key_raw()
}

fn pem_certificate(der: &[u8]) -> String {
    let encoded = STANDARD.encode(der);
    let mut pem = String::from("-----BEGIN CERTIFICATE-----\n");
    for chunk in encoded.as_bytes().chunks(64) {
        pem.push_str(std::str::from_utf8(chunk).unwrap_or_default());
        pem.push('\n');
    }
    pem.push_str("-----END CERTIFICATE-----\n");
    pem
}

fn pem_private_key(der: &[u8]) -> Zeroizing<String> {
    let encoded = Zeroizing::new(STANDARD.encode(der));
    let mut pem = Zeroizing::new(String::from("-----BEGIN PRIVATE KEY-----\n"));
    for chunk in encoded.as_bytes().chunks(64) {
        pem.push_str(std::str::from_utf8(chunk).unwrap_or_default());
        pem.push('\n');
    }
    pem.push_str("-----END PRIVATE KEY-----\n");
    pem
}

fn parse_key(encoded: &[u8]) -> Option<KeyPair> {
    let key = PrivatePkcs8KeyDer::from_pem_slice(encoded).ok()?;
    KeyPair::try_from(&key).ok()
}

pub(super) fn recovery_required(credential_id: &str) -> GatewayActualState {
    GatewayActualState {
        credential_id: Some(credential_id.to_owned()),
        state: GatewayState::RecoveryRequired.into(),
        gateway_leaf_sha256: None,
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::MetadataExt as _;

    use tempfile::TempDir;

    use super::*;

    fn tempdir() -> TempDir {
        TempDir::new().unwrap_or_else(|error| panic!("test directory must be created: {error}"))
    }

    fn reconciler(directory: &TempDir) -> GatewayReconciler {
        GatewayReconciler {
            intent: Mutex::new(None),
            generations_directory: directory.path().join("gateway"),
            active_path: directory.path().join("gateway-active.json"),
        }
    }

    fn target_without_grant(credential_id: Uuid) -> ValidatedGatewayTarget {
        ValidatedGatewayTarget {
            credential_id,
            certificate: None,
        }
    }

    #[test]
    fn gateway_input_is_durable_and_exactly_replayed() {
        let directory = tempdir();
        let reconciler = reconciler(&directory);
        let credential_id = Uuid::now_v7();
        let credential_id_text = credential_id.hyphenated().to_string();

        let first = reconciler
            .current_input(&target_without_grant(credential_id))
            .unwrap_or_else(|error| panic!("input must be generated: {error}"));
        let replay = reconciler
            .current_input(&target_without_grant(credential_id))
            .unwrap_or_else(|error| panic!("input replay must load: {error}"));

        assert_eq!(first, replay);
        let generation = reconciler.generations_directory.join(&credential_id_text);
        assert!(generation.join(GATEWAY_KEY_NAME).is_file());
        assert!(generation.join(GATEWAY_INPUT_NAME).is_file());
        let csr = first
            .gateway_csr_der
            .unwrap_or_else(|| panic!("new input must carry the durable CSR"));
        let encoded_key = Zeroizing::new(
            fs::read(generation.join(GATEWAY_KEY_NAME))
                .unwrap_or_else(|error| panic!("key must be readable: {error}")),
        );
        let key = parse_key(&encoded_key).unwrap_or_else(|| panic!("key must parse"));
        assert!(valid_csr_for_key(&csr, &key));
    }

    #[test]
    fn replacement_removes_the_old_generation_and_active_pointer() {
        let directory = tempdir();
        let reconciler = reconciler(&directory);
        let old = Uuid::now_v7();
        reconciler
            .current_input(&target_without_grant(old))
            .unwrap_or_else(|error| panic!("old input must be generated: {error}"));
        fs::write(
            &reconciler.active_path,
            serde_json::to_vec(&ActiveGatewayArtifact {
                format_version: ACTIVE_FORMAT_VERSION,
                credential_id: old.hyphenated().to_string(),
            })
            .unwrap_or_else(|error| panic!("active pointer must encode: {error}")),
        )
        .unwrap_or_else(|error| panic!("active pointer fixture must be written: {error}"));

        let current = Uuid::now_v7();
        let input = reconciler
            .current_input(&target_without_grant(current))
            .unwrap_or_else(|error| panic!("replacement input must be generated: {error}"));

        assert_eq!(input.credential_id, current.hyphenated().to_string());
        assert!(!reconciler.generation_directory(old).exists());
        assert!(reconciler.generation_directory(current).is_dir());
        assert!(!reconciler.active_path.exists());
    }

    #[test]
    fn failed_pointer_cleanup_does_not_publish_a_new_input() {
        let directory = tempdir();
        let reconciler = reconciler(&directory);
        let previous = Uuid::now_v7();
        reconciler
            .current_input(&target_without_grant(previous))
            .unwrap_or_else(|error| panic!("previous input must be generated: {error}"));
        fs::create_dir(&reconciler.active_path)
            .unwrap_or_else(|error| panic!("invalid pointer fixture must be created: {error}"));
        let current = Uuid::now_v7();

        let result = reconciler.current_input(&target_without_grant(current));

        assert!(matches!(result, Err(SnapshotError::Artifact)));
        assert_eq!(
            reconciler
                .observed_input()
                .unwrap_or_else(|error| panic!("intent must remain observable: {error}"))
                .map(|input| input.credential_id),
            Some(previous.hyphenated().to_string())
        );
        assert!(!reconciler.generation_directory(current).exists());
    }

    #[test]
    fn missing_key_reports_unrecoverable_input_without_replacing_the_csr() {
        let directory = tempdir();
        let reconciler = reconciler(&directory);
        let credential_id = Uuid::now_v7();
        let credential_id_text = credential_id.hyphenated().to_string();
        let generated = reconciler.current_input(&target_without_grant(credential_id));
        assert!(generated.is_ok());
        let key_path = reconciler
            .generations_directory
            .join(&credential_id_text)
            .join(GATEWAY_KEY_NAME);
        fs::remove_file(key_path)
            .unwrap_or_else(|error| panic!("key fixture must be removed: {error}"));

        let input = reconciler
            .current_input(&target_without_grant(credential_id))
            .unwrap_or_else(|error| panic!("lost input must be represented: {error}"));
        assert_eq!(input.credential_id, credential_id_text);
        assert!(input.gateway_csr_der.is_none());
    }

    #[test]
    fn granted_gateway_directory_loss_never_generates_a_new_csr() {
        let directory = tempdir();
        let reconciler = reconciler(&directory);
        let credential_id = Uuid::now_v7();
        reconciler
            .current_input(&target_without_grant(credential_id))
            .unwrap_or_else(|error| panic!("input must be generated: {error}"));
        let generation = reconciler.generation_directory(credential_id);
        let encoded_key = Zeroizing::new(
            fs::read(generation.join(GATEWAY_KEY_NAME))
                .unwrap_or_else(|error| panic!("key must be readable: {error}")),
        );
        let key = parse_key(&encoded_key).unwrap_or_else(|| panic!("key must parse"));
        let leaf_der = CertificateParams::default()
            .self_signed(&key)
            .unwrap_or_else(|error| panic!("certificate must be generated: {error}"))
            .der()
            .to_vec();
        let target = ValidatedGatewayTarget {
            credential_id,
            certificate: Some(ValidatedGatewayCertificate {
                leaf_der,
                leaf_public_key: key.public_key_raw().to_vec(),
            }),
        };
        fs::remove_dir_all(&reconciler.generations_directory)
            .unwrap_or_else(|error| panic!("lost Gateway directory must be removed: {error}"));

        let first = reconciler
            .current_input(&target)
            .unwrap_or_else(|error| panic!("lost granted input must be represented: {error}"));
        let replay = reconciler
            .current_input(&target)
            .unwrap_or_else(|error| panic!("lost granted input must replay: {error}"));

        assert!(first.gateway_csr_der.is_none());
        assert_eq!(first, replay);
        assert!(!generation.exists());
    }

    #[test]
    fn corrupt_existing_key_reports_an_unrecoverable_generation() {
        let directory = tempdir();
        let reconciler = reconciler(&directory);
        let credential_id = Uuid::now_v7();
        let credential_id_text = credential_id.hyphenated().to_string();
        let generation = reconciler.generations_directory.join(&credential_id_text);
        fs::create_dir_all(&generation)
            .unwrap_or_else(|error| panic!("generation fixture must be created: {error}"));
        fs::write(generation.join(GATEWAY_KEY_NAME), b"not-a-private-key")
            .unwrap_or_else(|error| panic!("key fixture must be written: {error}"));

        let input = reconciler
            .current_input(&target_without_grant(credential_id))
            .unwrap_or_else(|error| panic!("corrupt key must be represented: {error}"));

        assert_eq!(input.credential_id, credential_id_text);
        assert!(input.gateway_csr_der.is_none());
        assert!(!generation.join(GATEWAY_INPUT_NAME).exists());
    }

    #[test]
    fn corrupt_installed_material_is_never_used_to_block_caddy() {
        let directory = tempdir();
        let reconciler = reconciler(&directory);
        let credential_id = Uuid::now_v7();
        reconciler
            .current_input(&target_without_grant(credential_id))
            .unwrap_or_else(|error| panic!("input must be generated: {error}"));
        let credential_id_text = credential_id.hyphenated().to_string();
        let generation = reconciler.generations_directory.join(&credential_id_text);
        fs::write(
            generation.join(GATEWAY_CERTIFICATE_NAME),
            b"not-a-certificate",
        )
        .unwrap_or_else(|error| panic!("certificate fixture must be written: {error}"));
        let active = ActiveGatewayArtifact {
            format_version: ACTIVE_FORMAT_VERSION,
            credential_id: credential_id_text,
        };
        fs::write(
            &reconciler.active_path,
            serde_json::to_vec(&active)
                .unwrap_or_else(|error| panic!("active fixture must encode: {error}")),
        )
        .unwrap_or_else(|error| panic!("active fixture must be written: {error}"));

        assert!(reconciler.active_material().is_none());
    }

    #[test]
    fn corrupted_csr_signature_is_reported_as_absent() {
        let directory = tempdir();
        let reconciler = reconciler(&directory);
        let credential_id = Uuid::now_v7();
        let credential_id_text = credential_id.hyphenated().to_string();
        let generated = reconciler
            .current_input(&target_without_grant(credential_id))
            .unwrap_or_else(|error| panic!("input must be generated: {error}"));
        let mut csr = generated
            .gateway_csr_der
            .unwrap_or_else(|| panic!("generated CSR must be present"));
        let Some(last) = csr.last_mut() else {
            panic!("generated CSR must not be empty");
        };
        *last ^= 1;
        let artifact = GatewayInputArtifact {
            format_version: INPUT_FORMAT_VERSION,
            credential_id: credential_id_text.clone(),
            csr_der_base64: STANDARD.encode(csr),
        };
        let input_path = reconciler
            .generations_directory
            .join(&credential_id_text)
            .join(GATEWAY_INPUT_NAME);
        fs::write(
            input_path,
            serde_json::to_vec(&artifact)
                .unwrap_or_else(|error| panic!("artifact must encode: {error}")),
        )
        .unwrap_or_else(|error| panic!("artifact fixture must be written: {error}"));

        let replay = reconciler
            .current_input(&target_without_grant(credential_id))
            .unwrap_or_else(|error| panic!("corrupt CSR must be represented: {error}"));

        assert!(replay.gateway_csr_der.is_none());
    }

    #[test]
    fn exact_gateway_target_does_not_replace_installed_artifacts() {
        let directory = tempdir();
        let reconciler = reconciler(&directory);
        let credential_id = Uuid::now_v7();
        reconciler
            .current_input(&target_without_grant(credential_id))
            .unwrap_or_else(|error| panic!("input must be generated: {error}"));
        let generation = reconciler.generation_directory(credential_id);
        let encoded_key = Zeroizing::new(
            fs::read(generation.join(GATEWAY_KEY_NAME))
                .unwrap_or_else(|error| panic!("key must be readable: {error}")),
        );
        let key = parse_key(&encoded_key).unwrap_or_else(|| panic!("key must parse"));
        let leaf_der = CertificateParams::default()
            .self_signed(&key)
            .unwrap_or_else(|error| panic!("certificate must be generated: {error}"))
            .der()
            .to_vec();
        let target = ValidatedGatewayTarget {
            credential_id,
            certificate: Some(ValidatedGatewayCertificate {
                leaf_der,
                leaf_public_key: key.public_key_raw().to_vec(),
            }),
        };
        let first = reconciler
            .reconcile(&target, &CancellationToken::new())
            .unwrap_or_else(|error| panic!("target must reconcile: {error}"));
        assert!(matches!(first, GatewayMaterialState::Available(_)));
        let certificate_path = generation.join(GATEWAY_CERTIFICATE_NAME);
        let certificate_inode = fs::metadata(&certificate_path)
            .unwrap_or_else(|error| panic!("certificate metadata must load: {error}"))
            .ino();
        let active_inode = fs::metadata(&reconciler.active_path)
            .unwrap_or_else(|error| panic!("active metadata must load: {error}"))
            .ino();

        let replay = reconciler
            .reconcile(&target, &CancellationToken::new())
            .unwrap_or_else(|error| panic!("target replay must reconcile: {error}"));

        assert!(matches!(replay, GatewayMaterialState::Available(_)));
        assert_eq!(
            fs::metadata(certificate_path)
                .unwrap_or_else(|error| panic!("certificate metadata must reload: {error}"))
                .ino(),
            certificate_inode
        );
        assert_eq!(
            fs::metadata(&reconciler.active_path)
                .unwrap_or_else(|error| panic!("active metadata must reload: {error}"))
                .ino(),
            active_inode
        );
    }

    #[test]
    fn gateway_actual_requires_the_exact_served_leaf() {
        let credential_id = Uuid::now_v7().hyphenated().to_string();
        let leaf_sha256 = Sha256::digest(b"leaf").to_vec();
        let material = GatewayMaterial {
            credential_id: credential_id.clone(),
            certificate_path: PathBuf::from("/test/fullchain.pem"),
            private_key_path: PathBuf::from("/test/key.pem"),
            leaf_sha256: leaf_sha256.clone(),
        };
        let mode = CaddyModeArtifact::Blocked {
            format_version: 1,
            credential_id: Some(credential_id),
        };
        let exact = CaddyObservation {
            mode: Some(mode.clone()),
            gateway_leaf_sha256: Some(leaf_sha256),
        };
        let wrong = CaddyObservation {
            mode: Some(mode),
            gateway_leaf_sha256: Some(Sha256::digest(b"other").to_vec()),
        };

        assert_eq!(
            actual_for_material(&material, &exact).state,
            i32::from(GatewayState::Blocked)
        );
        assert_eq!(
            actual_for_material(&material, &wrong).state,
            i32::from(GatewayState::RecoveryRequired)
        );
    }
}
