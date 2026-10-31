//! First-time HTTPS enrollment; ordinary workspace operations remain offline.
mod api;
pub(crate) mod doctor;
mod identity;
mod origin;
mod secrets;
mod store;
mod windows;

use std::{collections::BTreeSet, path::PathBuf};

use snafu::{ResultExt as _, Snafu, ensure};
use zeroize::Zeroizing;

use crate::{roster::Roster, workspace::Workspace};
use api::PhotoEnrollmentClient;
use natsume_photo_protocol::{InitializationRequestId, PhotoInitializationComplete, Sha256Digest};
use natsume_photo_protocol::{PhotoEnrollmentIssued, PhotoInitialization};
use store::{Phase, Receipt, Store};

pub use doctor::{diagnose, diagnose_path};
pub use identity::ClientId;
use identity::PreparedId;
pub use origin::{OriginError, ServerOrigin};
pub use secrets::{CredentialError, CredentialResult, SecretKind, SecretSlot, Secrets};
pub use windows::VaultSecrets;

/// Initialization failures never include credentials or response body contents.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub(super)))]
pub enum Error {
    #[snafu(display("Photo initialization blocking task failed"))]
    Worker { source: tokio::task::JoinError },
    #[snafu(display("Photo initialization value is invalid"))]
    Value {
        source: natsume_photo_protocol::ValueError,
    },
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
    #[snafu(display(
        "Photo workspace target must have a parent and file name and be absolute without parent traversal"
    ))]
    Path,
    #[snafu(display("Photo server origin is invalid"))]
    Origin { source: OriginError },
    #[snafu(display("Photo initialization data exceeds the byte limit ({limit})"))]
    Limit { limit: usize },
    #[snafu(display("Photo CA bundle contains no certificates"))]
    EmptyCa,
    #[snafu(display("Photo completion unexpectedly returned a response body"))]
    UnexpectedBody,
    #[snafu(display("Photo confirmation does not match this enrollment"))]
    Confirmation,
    #[snafu(display("Photo downloaded resource integrity check failed"))]
    Integrity,
    #[snafu(display("Photo initialization destination conflicts with existing content"))]
    FileConflict,
    #[snafu(display("Photo initialization expected a directory"))]
    NotDirectory,
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
    Credential { source: CredentialError },
}

/// Result type shared with platform credential adapters.
pub type Result<T> = std::result::Result<T, Error>;

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
    origin: ServerOrigin,
    ca_certificate: Option<PathBuf>,
    secrets: S,
}

impl<S: Secrets> Initializer<S> {
    /// Opens or resumes initialization without overwriting an existing workspace.
    /// # Errors
    /// Rejects unsafe paths, changed origins, concurrent runs or damaged local state.
    pub fn open(config: Config, secrets: S) -> Result<Self> {
        let origin = ServerOrigin::parse(&config.server_url).context(OriginSnafu)?;
        let store = Store::open(&config.workspace, &origin)?;
        Ok(Self {
            store,
            origin,
            ca_certificate: config.ca_certificate,
            secrets,
        })
    }

    /// Indicates whether a new one-time code must be obtained from the administrator.
    /// # Errors
    /// Returns a credential-store error; credentials are never silently replaced.
    pub fn needs_pairing_code(&self) -> Result<bool> {
        Ok(matches!(self.store.state().phase, Phase::Unpaired)
            && self
                .secrets
                .load(&self.origin, &self.slot(SecretKind::Enrollment))
                .context(CredentialSnafu)?
                .is_none())
    }

    fn slot(&self, kind: SecretKind) -> SecretSlot {
        SecretSlot::new(self.store.state().client_id.clone(), kind)
    }

    async fn signing_key(&self) -> Result<ed25519_dalek::SigningKey> {
        let secrets = self.secrets.clone();
        let origin = self.origin.clone();
        let slot = self.slot(SecretKind::Signing);
        let unpaired = matches!(self.store.state().phase, Phase::Unpaired);
        let marker = self.store.root().join("pairing-started");
        self.store
            .lease()
            .run(move || {
                let secret = secrets.load(&origin, &slot).context(CredentialSnafu)?;
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
                    secrets
                        .save(&origin, &slot, &Zeroizing::new(hex::encode(seed.as_ref())))
                        .context(CredentialSnafu)?;
                    seed
                };
                Ok(ed25519_dalek::SigningKey::from_bytes(&seed))
            })
            .await
    }

    async fn enrollment(&self) -> Result<PhotoEnrollmentIssued> {
        let secrets = self.secrets.clone();
        let origin = self.origin.clone();
        let slot = self.slot(SecretKind::Enrollment);
        self.store
            .lease()
            .run(move || {
                let secret = secrets
                    .load(&origin, &slot)
                    .context(CredentialSnafu)?
                    .ok_or_else(|| MissingSecretSnafu.build())?;
                let issued: PhotoEnrollmentIssued =
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
        if let Phase::Completed { receipt } = &self.store.state().phase {
            return self.store.publish(receipt, &self.secrets).await;
        }
        let key = self.signing_key().await?;
        let public_key = key.verifying_key();
        let origin = self.origin.clone();
        let ca = self.ca_certificate.clone();
        let api = run_blocking(move || PhotoEnrollmentClient::load_tls(origin, ca)).await?;
        self.pair(&api, code, &public_key).await?;
        let enrollment = self.enrollment().await?;
        ensure!(
            self.store.state().source_id.as_ref() == Some(&enrollment.source_id),
            AuthorizationSnafu
        );
        if matches!(self.store.state().phase, Phase::Paired) {
            api.confirm(&enrollment, &public_key).await?;
            let download = api.prepare_initialization(&enrollment).await?;
            let receipt = self
                .prepare(
                    &api,
                    &enrollment,
                    &public_key,
                    &download.bytes,
                    download.manifest,
                )
                .await?;
            self.store.record_saved(receipt).await?;
        }
        let Phase::Saved { receipt } = &self.store.state().phase else {
            return JournalSnafu.fail();
        };
        // Revalidate every local file before closing download rights on Server.
        self.store.verify(receipt, &self.secrets).await?;
        api.complete(
            &enrollment,
            &PhotoInitializationComplete {
                initialization_id: receipt.initialization_id.clone(),
                manifest_sha256: receipt.manifest_sha256.clone(),
                request_id: receipt.request_id.clone(),
            },
        )
        .await?;
        let receipt = self.store.record_completed().await?;
        self.store.publish(&receipt, &self.secrets).await
    }

    async fn pair(
        &mut self,
        api: &PhotoEnrollmentClient,
        code: Option<&str>,
        public_key: &ed25519_dalek::VerifyingKey,
    ) -> Result<()> {
        if !matches!(self.store.state().phase, Phase::Unpaired) {
            return Ok(());
        }
        let secrets = self.secrets.clone();
        let origin = self.origin.clone();
        let slot = self.slot(SecretKind::Enrollment);
        let existing = self
            .store
            .lease()
            .run(move || secrets.load(&origin, &slot).context(CredentialSnafu))
            .await?;
        if existing.is_none() {
            let code = code
                .filter(|code| !code.trim().is_empty())
                .ok_or_else(|| MissingSecretSnafu.build())?;
            let marker = self.store.root().join("pairing-started");
            self.store
                .lease()
                .run(move || store::save_new_or_verify(&marker, b"1"))
                .await?;
            let issued = api.pair(code, public_key).await?;
            let encoded = Zeroizing::new(serde_json::to_string(&issued).map_err(json_error)?);
            let secrets = self.secrets.clone();
            let origin = self.origin.clone();
            let slot = self.slot(SecretKind::Enrollment);
            self.store
                .lease()
                .run(move || {
                    secrets
                        .save(&origin, &slot, &encoded)
                        .context(CredentialSnafu)
                })
                .await?;
        }
        let issued = self.enrollment().await?;
        self.store.record_pairing(issued.source_id.clone()).await
    }

    async fn prepare(
        &self,
        api: &PhotoEnrollmentClient,
        enrollment: &PhotoEnrollmentIssued,
        public_key: &ed25519_dalek::VerifyingKey,
        bytes: &[u8],
        manifest: PhotoInitialization,
    ) -> Result<Receipt> {
        let roster = Roster::from_teams(manifest.teams).context(RosterSnafu)?;
        let prepared = PreparedId::generate();
        let download = self
            .store
            .root()
            .join(format!("downloads-{}", manifest.initialization_id));
        let directory = download.clone();
        self.store
            .lease()
            .run(move || {
                std::fs::create_dir_all(&directory).context(IoSnafu)?;
                ensure!(
                    std::fs::canonicalize(&directory).context(IoSnafu)? == directory,
                    PathSnafu
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
                let directory = download.clone();
                let digest = logo.sha256.clone();
                self.store
                    .lease()
                    .run(move || store::save_download(&directory, &digest, &image))
                    .await?;
            }
            logos.push(crate::workspace::resources::SchoolLogo {
                organization_id: logo.organization_id,
                path: filename.into(),
            });
        }
        let workspace = self.store.root().join(prepared.as_str());
        drop(
            self.store
                .lease()
                .run({
                    let workspace = workspace.clone();
                    move || {
                        Workspace::create_with_logos(
                            workspace,
                            roster.teams().to_vec(),
                            &download,
                            logos,
                        )
                        .context(WorkspaceSnafu)
                    }
                })
                .await?,
        );
        let hash = Sha256Digest::of(bytes);
        let identity = doctor::Authorization {
            server_url: self.origin.clone(),
            client_id: self.store.state().client_id.clone(),
            signing_public_key: hex::encode(public_key.as_bytes())
                .parse()
                .context(ValueSnafu)?,
            source_id: enrollment.source_id.clone(),
            dataset_id: manifest.dataset_id,
            initialization_id: manifest.initialization_id.clone(),
            manifest_sha256: hash.clone(),
        };
        let bytes = bytes.to_owned();
        self.store
            .lease()
            .run(move || {
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
            request_id: InitializationRequestId::generate(),
        })
    }
}

async fn run_blocking<T: Send + 'static>(
    operation: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(operation)
        .await
        .context(WorkerSnafu)?
}
