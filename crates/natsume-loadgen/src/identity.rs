use std::{
    fs::{self, File},
    io::Write as _,
    path::{Path, PathBuf},
};

use ed25519_dalek::SigningKey;
use natsume_workstation_protocol::{
    generated::{
        ClientProof, EnrollmentAttempt, EnrollmentEvidenceQuality, ResumeSession, ServerChallenge,
        client_proof,
    },
    sign_client_proof,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use snafu::ResultExt as _;
use uuid::Uuid;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::error::{self, Error, Result};

#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(deny_unknown_fields)]
pub(crate) struct Gateway {
    pub(crate) credential_id: String,
    pub(crate) key_der: Vec<u8>,
    pub(crate) csr_der: Vec<u8>,
    pub(crate) leaf_der: Vec<u8>,
}

#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(deny_unknown_fields)]
pub(crate) struct Durable {
    version: u32,
    seed: u64,
    index: usize,
    pub(crate) seat: String,
    pub(crate) hardware_id: String,
    control_key: [u8; 32],
    pub(crate) device_id: Option<String>,
    pub(crate) gateway: Option<Gateway>,
    /// Encoded Client snapshot contains inputs and non-secret local facts only.
    pub(crate) local: Vec<u8>,
    pub(crate) power_accepted_epoch: Option<u64>,
}

pub(crate) struct Identity {
    pub(crate) state: Durable,
    path: PathBuf,
}

impl Identity {
    pub(crate) fn load(directory: &Path, seed: u64, index: usize, seat: String) -> Result<Self> {
        let path = directory.join(format!("device-{index:05}.json"));
        let hardware_id = Uuid::new_v5(
            &Uuid::NAMESPACE_OID,
            format!("natsume-loadgen:{seed}:{index}").as_bytes(),
        )
        .to_string();
        let state = match fs::read(&path) {
            Ok(encoded) => {
                let encoded = Zeroizing::new(encoded);
                let state: Durable = serde_json::from_slice(&encoded).context(error::JsonSnafu)?;
                if state.version != 1
                    || state.seed != seed
                    || state.index != index
                    || state.seat != seat
                    || state.hardware_id != hardware_id
                    || state.control_key != key_seed(seed, index)
                {
                    return Err(Error::Invalid {
                        code: "FLEET_STATE_MISMATCH",
                    });
                }
                state
            }
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => Durable {
                version: 1,
                seed,
                index,
                seat,
                hardware_id,
                control_key: key_seed(seed, index),
                device_id: None,
                gateway: None,
                local: Vec::new(),
                power_accepted_epoch: None,
            },
            Err(source) => return Err(Error::Io { source }),
        };
        let identity = Self { state, path };
        identity.save()?;
        Ok(identity)
    }

    pub(crate) fn save(&self) -> Result<()> {
        let parent = self
            .path
            .parent()
            .ok_or(Error::Invalid { code: "STATE_PATH" })?;
        atomic_write(
            &self.path,
            parent,
            &Zeroizing::new(serde_json::to_vec(&self.state).context(error::JsonSnafu)?),
        )
    }

    pub(crate) fn proof(&self, challenge: &ServerChallenge, client_ip: String) -> ClientProof {
        let key = SigningKey::from_bytes(&self.state.control_key);
        sign_client_proof(
            &key,
            challenge,
            ClientProof {
                daemon_version: env!("CARGO_PKG_VERSION").to_owned(),
                agent_version: env!("CARGO_PKG_VERSION").to_owned(),
                machine_hardware_id: self.state.hardware_id.clone(),
                signature: Vec::new(),
                purpose: Some(if self.state.device_id.is_some() {
                    client_proof::Purpose::Resume(ResumeSession {})
                } else {
                    client_proof::Purpose::Enrollment(EnrollmentAttempt {
                        candidate_public_key: key.verifying_key().to_bytes().to_vec(),
                        evidence_quality: EnrollmentEvidenceQuality::Strong.into(),
                    })
                }),
                client_ip: Some(client_ip),
            },
        )
    }
}

fn key_seed(seed: u64, index: usize) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"natsume-loadgen-control-key-v1");
    hash.update(seed.to_le_bytes());
    hash.update((index as u64).to_le_bytes());
    hash.finalize().into()
}

pub(crate) fn atomic_write(path: &Path, parent: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = tempfile::NamedTempFile::new_in(parent).context(error::IoSnafu)?;
    file.write_all(bytes).context(error::IoSnafu)?;
    file.as_file().sync_all().context(error::IoSnafu)?;
    file.persist(path).map_err(|error| Error::Io {
        source: error.error,
    })?;
    File::open(parent)
        .context(error::IoSnafu)?
        .sync_all()
        .context(error::IoSnafu)
}

/// Counter-based deterministic samples, independent for each device and purpose.
pub(crate) struct Samples {
    seed: u64,
    index: usize,
    purpose: &'static str,
    counter: u64,
}

impl Samples {
    pub(crate) const fn new(seed: u64, index: usize, purpose: &'static str) -> Self {
        Self {
            seed,
            index,
            purpose,
            counter: 0,
        }
    }

    pub(crate) fn uniform(&mut self, maximum: u64) -> u64 {
        let range = maximum + 1;
        let ceiling = u64::MAX - u64::MAX % range;
        loop {
            let mut hash = Sha256::new();
            hash.update(self.seed.to_le_bytes());
            hash.update((self.index as u64).to_le_bytes());
            hash.update(self.purpose.as_bytes());
            hash.update(self.counter.to_le_bytes());
            self.counter += 1;
            let digest = hash.finalize();
            let mut bytes = [0_u8; 8];
            bytes.copy_from_slice(&digest[..8]);
            let sample = u64::from_le_bytes(bytes);
            if sample < ceiling {
                return sample % range;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use natsume_workstation_protocol::{generated::client_proof::Purpose, verify_client_proof};

    #[test]
    fn authority_and_key_survive_reload_and_proofs_verify() -> Result<()> {
        let directory = tempfile::tempdir().context(error::IoSnafu)?;
        let challenge = ServerChallenge {
            challenge_nonce: vec![7; 32],
        };
        let mut identity = Identity::load(directory.path(), 42, 3, "003".to_owned())?;
        let proof = identity.proof(&challenge, "127.0.0.1".to_owned());
        let Some(Purpose::Enrollment(attempt)) = proof.purpose.as_ref() else {
            return Err(Error::Invalid {
                code: "TEST_ENROLLMENT",
            });
        };
        assert!(verify_client_proof(&attempt.candidate_public_key, &challenge, &proof).is_ok());
        identity.state.device_id = Some(Uuid::now_v7().to_string());
        identity.save()?;
        let resumed = Identity::load(directory.path(), 42, 3, "003".to_owned())?;
        assert_eq!(resumed.state.device_id, identity.state.device_id);
        assert!(matches!(
            resumed.proof(&challenge, "127.0.0.1".to_owned()).purpose,
            Some(Purpose::Resume(_))
        ));
        assert!(Identity::load(directory.path(), 43, 3, "003".to_owned()).is_err());
        Ok(())
    }
}
