//! First-time HTTPS enrollment; ordinary workspace operations remain offline.
mod api;
mod doctor;
mod store;
mod windows;
mod wire;

use std::{collections::BTreeSet, path::PathBuf};

use snafu::{ResultExt as _, Snafu, ensure};
use zeroize::Zeroizing;

use crate::{roster::Roster, workspace::Workspace};
use api::Api;
use store::{Phase, Receipt, Store};
use wire::{Manifest, PairingCredentials};

pub use doctor::diagnose;
pub use windows::VaultSecrets;

/// Initialization failures never include credentials or response body contents.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub(super)))]
pub enum Error {
    #[snafu(display("Photo initialization blocking task failed"))]
    Worker { source: tokio::task::JoinError },
    #[snafu(display("Photo server URL could not be parsed"))]
    Url { source: url::ParseError },
    #[snafu(display("Photo signing credential encoding is invalid ({kind})"))]
    KeyEncoding { kind: &'static str },
    #[snafu(display("Photo initialization storage operation failed"))]
    Io { source: std::io::Error },
    #[snafu(display("Photo initialization record is invalid"))]
    Json { line: usize, column: usize },
    #[snafu(display("Photo initialization contract is invalid"))]
    Contract {
        source: natsume_photo_protocol::ValidationError,
    },
    #[snafu(display("Photo initialization journal is inconsistent"))]
    Journal,
    #[snafu(display("Photo workspace authorization is inconsistent"))]
    Authorization,
    #[snafu(display("Photo workspace path must be absolute without parent traversal"))]
    Path,
    #[snafu(display("Photo server origin is invalid"))]
    Origin,
    #[snafu(display("Photo response exceeds the byte limit ({limit})"))]
    Limit { limit: usize },
    #[snafu(display("Photo initialization input or response is invalid"))]
    Invalid,
    #[snafu(display("Photo initialization directory is already in use"))]
    Busy,
    #[snafu(display("Photo initialization requires the original signing key and credentials"))]
    MissingSecret,
    #[snafu(display("Photo initialization transport failed"))]
    Transport { source: reqwest::Error },
    #[snafu(display("Photo initialization was rejected by Server ({status})"))]
    Rejected { status: u16 },
    #[snafu(display("Photo initialization entropy is unavailable"))]
    Entropy { source: getrandom::Error },
    #[snafu(display("Photo initialization roster is invalid"))]
    Roster { source: crate::roster::Error },
    #[snafu(display("Photo initialization workspace could not be saved or verified"))]
    Workspace { source: crate::workspace::Error },
    #[snafu(display("Photo initialization Windows credential storage failed"))]
    Credential { source: ::windows::core::Error },
}

/// Result type shared with platform credential adapters.
pub type Result<T> = std::result::Result<T, Error>;

/// Platform secret storage, separated from public initialization metadata.
/// Implementations must reliably save values before returning success.
pub trait Secrets: Clone + Send + Sync + 'static {
    /// Reads a value without logging it; absence is distinct from storage failure.
    /// # Errors
    /// Returns a platform credential storage error.
    fn load(&self, origin: &str, slot: &str) -> Result<Option<Zeroizing<String>>>;
    /// Saves a credential under an initialization-specific slot.
    /// # Errors
    /// Returns a platform credential storage error.
    fn save(&self, origin: &str, slot: &str, value: &str) -> Result<()>;
}

// serde_json errors can embed the rejected value. Preserve location, never the
// source string or a Debug/source chain containing secret input.
fn json_error(error: serde_json::Error) -> Error {
    let redacted = JsonSnafu {
        line: error.line(),
        column: error.column(),
    }
    .build();
    drop(error);
    redacted
}

fn key_encoding_error(error: hex::FromHexError) -> Error {
    let kind = match error {
        hex::FromHexError::InvalidHexCharacter { .. } => "character",
        hex::FromHexError::OddLength => "odd length",
        hex::FromHexError::InvalidStringLength => "length",
    };
    KeyEncodingSnafu { kind }.build()
}

/// Inputs for a new initialization or resuming the same target.
#[derive(Debug)]
pub struct Config {
    pub server_url: String,
    pub workspace: PathBuf,
    pub ca_certificate: Option<PathBuf>,
}

/// Owns an exclusive initialization journal and its platform secret adapter.
/// No network client is constructed until a network operation is required.
pub struct Initializer<S> {
    store: Store,
    config: Config,
    secrets: S,
}

impl<S: Secrets> Initializer<S> {
    /// Opens or resumes initialization without overwriting an existing workspace.
    /// # Errors
    /// Rejects unsafe paths, changed origins, concurrent runs or damaged local state.
    pub fn open(mut config: Config, secrets: S) -> Result<Self> {
        config.server_url = api::origin(&config.server_url)?;
        let store = Store::open(&config)?;
        Ok(Self {
            store,
            config,
            secrets,
        })
    }

    /// Indicates whether a new one-time code must be obtained from the administrator.
    /// # Errors
    /// Returns a credential-store error; credentials are never silently replaced.
    pub fn needs_pairing_code(&self) -> Result<bool> {
        Ok(matches!(self.store.state.phase, Phase::Unpaired)
            && self
                .secrets
                .load(&self.config.server_url, &self.slot("enrollment"))?
                .is_none())
    }

    fn slot(&self, name: &str) -> String {
        format!("{}/{name}", self.store.state.client_id)
    }

    async fn signing_key(&self) -> Result<ed25519_dalek::SigningKey> {
        let secrets = self.secrets.clone();
        let origin = self.config.server_url.clone();
        let slot = self.slot("signing");
        let unpaired = matches!(self.store.state.phase, Phase::Unpaired);
        let marker = self.store.root.join("pairing-started");
        self.store
            .blocking(move || {
                let secret = secrets.load(&origin, &slot)?;
                let seed = if let Some(secret) = secret {
                    let mut seed = Zeroizing::new([0; 32]);
                    hex::decode_to_slice(secret.as_str(), seed.as_mut())
                        .map_err(key_encoding_error)?;
                    seed
                } else {
                    ensure!(unpaired, MissingSecretSnafu);
                    // Do not replace a key after a pairing request could have reached Server.
                    ensure!(!marker.exists(), MissingSecretSnafu);
                    let mut seed = Zeroizing::new([0; 32]);
                    getrandom::fill(seed.as_mut()).context(EntropySnafu)?;
                    secrets.save(&origin, &slot, &Zeroizing::new(hex::encode(seed.as_ref())))?;
                    seed
                };
                Ok(ed25519_dalek::SigningKey::from_bytes(&seed))
            })
            .await
    }

    async fn enrollment(&self) -> Result<PairingCredentials> {
        let secrets = self.secrets.clone();
        let origin = self.config.server_url.clone();
        let slot = self.slot("enrollment");
        self.store
            .blocking(move || {
                let secret = secrets
                    .load(&origin, &slot)?
                    .ok_or_else(|| MissingSecretSnafu.build())?;
                let issued: PairingCredentials =
                    serde_json::from_str(&secret).map_err(json_error)?;
                issued.validate().context(ContractSnafu)?;
                Ok(issued)
            })
            .await
    }

    /// Pairs, downloads, saves, acknowledges and publishes the offline workspace.
    /// Supply a code only for a new pairing; completed runs make no network requests.
    /// # Errors
    /// Returns typed storage, TLS, authorization, validation or workspace errors.
    pub async fn run(mut self, code: Option<&str>) -> Result<Workspace> {
        if let Phase::Completed { receipt } = &self.store.state.phase {
            return self.store.publish(receipt, &self.secrets).await;
        }
        let key = self.signing_key().await?;
        let public_key = hex::encode(key.verifying_key().to_bytes());
        let config = Config {
            server_url: self.config.server_url.clone(),
            workspace: self.config.workspace.clone(),
            ca_certificate: self.config.ca_certificate.clone(),
        };
        let api = self.store.blocking(move || Api::new(&config)).await?;
        self.pair(&api, code, &public_key).await?;
        let enrollment = self.enrollment().await?;
        ensure!(
            self.store.state.source_id.as_deref() == Some(&enrollment.source_id),
            InvalidSnafu
        );
        if matches!(self.store.state.phase, Phase::Paired) {
            api.confirm(&enrollment, &public_key).await?;
            let (bytes, manifest) = api.manifest(&enrollment).await?;
            let receipt = self
                .prepare(&api, &enrollment, &public_key, &bytes, manifest)
                .await?;
            self.store.advance(Phase::Saved { receipt }).await?;
        }
        let Phase::Saved { receipt } = &self.store.state.phase else {
            return InvalidSnafu.fail();
        };
        // Revalidate every local file before closing download rights on Server.
        self.store.verify(receipt, &self.secrets).await?;
        api.complete(&enrollment, receipt).await?;
        let receipt = receipt.clone();
        self.store
            .advance(Phase::Completed {
                receipt: receipt.clone(),
            })
            .await?;
        self.store.publish(&receipt, &self.secrets).await
    }

    async fn pair(&mut self, api: &Api, code: Option<&str>, public_key: &str) -> Result<()> {
        if !matches!(self.store.state.phase, Phase::Unpaired) {
            return Ok(());
        }
        let secrets = self.secrets.clone();
        let origin = self.config.server_url.clone();
        let slot = self.slot("enrollment");
        let existing = self
            .store
            .blocking(move || secrets.load(&origin, &slot))
            .await?;
        if existing.is_none() {
            let code = code
                .filter(|code| !code.trim().is_empty())
                .ok_or_else(|| MissingSecretSnafu.build())?;
            let marker = self.store.root.join("pairing-started");
            self.store
                .blocking(move || store::save_new_or_verify(&marker, b"1"))
                .await?;
            let issued = api.pair(code, public_key).await?;
            let encoded = Zeroizing::new(serde_json::to_string(&issued).map_err(json_error)?);
            let secrets = self.secrets.clone();
            let origin = self.config.server_url.clone();
            let slot = self.slot("enrollment");
            self.store
                .blocking(move || secrets.save(&origin, &slot, &encoded))
                .await?;
        }
        let issued = self.enrollment().await?;
        self.store.state.source_id = Some(issued.source_id.clone());
        self.store.advance(Phase::Paired).await
    }

    async fn prepare(
        &self,
        api: &Api,
        enrollment: &PairingCredentials,
        public_key: &str,
        bytes: &[u8],
        manifest: Manifest,
    ) -> Result<Receipt> {
        let roster = Roster::from_teams(manifest.teams).context(RosterSnafu)?;
        let prepared = uuid::Uuid::now_v7().to_string();
        let download = self
            .store
            .root
            .join(format!("downloads-{}", manifest.initialization_id));
        let directory = download.clone();
        self.store
            .blocking(move || {
                std::fs::create_dir_all(&directory).context(IoSnafu)?;
                ensure!(
                    std::fs::canonicalize(&directory).context(IoSnafu)? == directory,
                    InvalidSnafu
                );
                Ok(())
            })
            .await?;
        let mut downloaded = BTreeSet::new();
        let mut logos = Vec::new();
        for logo in manifest.school_logos {
            let filename = format!("{}.png", logo.sha256);
            if downloaded.insert(logo.sha256.clone()) {
                let image = api.resource(enrollment, &logo).await?;
                let path = download.join(&filename);
                self.store
                    .blocking(move || store::save_new_or_verify(&path, &image))
                    .await?;
            }
            logos.push(crate::workspace::resources::SchoolLogo {
                organization_id: logo.organization_id,
                path: filename.into(),
            });
        }
        let workspace = self.store.root.join(&prepared);
        drop(
            Workspace::create_with_logos(&workspace, &roster, download, logos)
                .await
                .context(WorkspaceSnafu)?,
        );
        let hash = wire::digest(bytes);
        let identity = doctor::Authorization {
            server_url: self.config.server_url.clone(),
            client_id: self.store.state.client_id.clone(),
            signing_public_key: public_key.to_owned(),
            source_id: enrollment.source_id.clone(),
            dataset_id: manifest.dataset_id,
            initialization_id: manifest.initialization_id.clone(),
            manifest_sha256: hash.clone(),
        };
        let bytes = bytes.to_owned();
        self.store
            .blocking(move || {
                store::save_new_or_verify(&workspace.join("initialization.json"), &bytes)?;
                store::save_new_or_verify(
                    &workspace.join("authorization.json"),
                    &serde_json::to_vec(&identity).map_err(json_error)?,
                )?;
                store::validate_directory(&workspace)
            })
            .await?;
        Ok(Receipt {
            prepared,
            initialization_id: manifest.initialization_id,
            manifest_sha256: hash,
            request_id: uuid::Uuid::now_v7().to_string(),
        })
    }
}
