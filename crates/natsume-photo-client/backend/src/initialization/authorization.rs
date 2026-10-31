//! Offline authorization shared by diagnostics and result export; no HTTP lifecycle errors.
use super::{ClientId, Secrets};
use crate::workspace::Workspace;
use natsume_photo_protocol::{DatasetId, InitializationId, Sha256Digest, SourceId};
use serde::{Deserialize, Serialize};
use snafu::{ResultExt as _, Snafu, ensure};
use std::{fs::File, io::Read as _, path::Path};
use zeroize::Zeroizing;

#[derive(Debug, Snafu)]
pub enum AuthorizationError {
    #[snafu(display("offline authorization file operation failed"))]
    Io { source: std::io::Error },
    #[snafu(display("offline authorization JSON is invalid at {line}:{column}"))]
    Json { line: usize, column: usize },
    #[snafu(display("offline authorization credential is missing"))]
    MissingSecret,
    #[snafu(display("offline signing credential encoding is invalid ({kind})"))]
    KeyEncoding { kind: &'static str },
    #[snafu(display("offline authorization does not match the workspace or original key"))]
    Mismatch,
    #[snafu(display("offline authorization file exceeds its byte limit"))]
    Limit,
    #[snafu(display("offline authorization credential access failed"))]
    Credential { source: super::CredentialError },
    #[snafu(display("offline authorization initialization contract is invalid"))]
    Contract {
        source: natsume_photo_protocol::ValidationError,
    },
    #[snafu(display("offline authorization workspace validation failed"))]
    Workspace { source: crate::workspace::Error },
    #[snafu(display("offline authorization task failed"))]
    Worker { source: tokio::task::JoinError },
}
type Result<T> = std::result::Result<T, AuthorizationError>;

fn json_error(error: serde_json::Error) -> AuthorizationError {
    let redacted = JsonSnafu {
        line: error.line(),
        column: error.column(),
    }
    .build();
    drop(error);
    redacted
}
fn key_encoding_error(error: hex::FromHexError) -> AuthorizationError {
    let kind = match error {
        hex::FromHexError::InvalidHexCharacter { .. } => "character",
        hex::FromHexError::OddLength => "odd length",
        hex::FromHexError::InvalidStringLength => "length",
    };
    KeyEncodingSnafu { kind }.build()
}
fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)
        .context(IoSnafu)?
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .context(IoSnafu)?;
    ensure!(bytes.len() as u64 <= limit, LimitSnafu);
    Ok(bytes)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Authorization {
    pub server_url: super::ServerOrigin,
    pub client_id: ClientId,
    pub signing_public_key: natsume_photo_protocol::SigningPublicKey,
    pub source_id: SourceId,
    pub dataset_id: DatasetId,
    pub initialization_id: InitializationId,
    pub manifest_sha256: Sha256Digest,
}

// Resource bytes are verified by WorkspaceReader::inspect or diagnose.
pub(crate) async fn verify<Access>(
    workspace: &Workspace<Access>,
    secrets: &impl Secrets,
) -> Result<Authorization> {
    let root = workspace.root().to_owned();
    let secrets = secrets.clone();
    let (identity, manifest) = tokio::task::spawn_blocking(move || {
        let identity: Authorization =
            serde_json::from_slice(&read_bounded(&root.join("authorization.json"), 8192)?)
                .map_err(json_error)?;
        load_signing_key(&identity, &secrets)?;
        let bytes = read_bounded(
            &root.join("initialization.json"),
            natsume_photo_protocol::MANIFEST_LIMIT as u64,
        )?;
        ensure!(
            Sha256Digest::of(&bytes) == identity.manifest_sha256,
            MismatchSnafu
        );
        let manifest: natsume_photo_protocol::PhotoInitialization =
            serde_json::from_slice(&bytes).map_err(json_error)?;
        manifest
            .validate(&identity.source_id)
            .context(ContractSnafu)?;
        ensure!(
            manifest.dataset_id == identity.dataset_id
                && manifest.initialization_id == identity.initialization_id,
            MismatchSnafu
        );
        Ok((identity, manifest))
    })
    .await
    .context(WorkerSnafu)??;
    let roster = workspace.roster().await.context(WorkspaceSnafu)?;
    let actual: std::collections::BTreeMap<_, _> =
        roster.teams().iter().map(|t| (&t.account_id, t)).collect();
    let expected: std::collections::BTreeMap<_, _> =
        manifest.teams.iter().map(|t| (&t.account_id, t)).collect();
    ensure!(
        actual == expected
            && workspace
                .matches_logos(&manifest)
                .await
                .context(WorkspaceSnafu)?,
        MismatchSnafu
    );
    Ok(identity)
}

// Only Rust application boundaries may borrow the original vault key; never generate a fallback.
pub(crate) fn load_signing_key(
    identity: &Authorization,
    secrets: &impl Secrets,
) -> Result<ed25519_dalek::SigningKey> {
    let secret = secrets
        .load(
            &identity.server_url,
            &super::SecretSlot::new(identity.client_id.clone(), super::SecretKind::Signing),
        )
        .context(CredentialSnafu)?
        .ok_or_else(|| MissingSecretSnafu.build())?;
    let mut seed = Zeroizing::new([0; 32]);
    hex::decode_to_slice(secret.as_str(), seed.as_mut()).map_err(key_encoding_error)?;
    let key = ed25519_dalek::SigningKey::from_bytes(&seed);
    ensure!(
        hex::encode(key.verifying_key().to_bytes()) == identity.signing_public_key.as_str(),
        MismatchSnafu
    );
    Ok(key)
}
