use super::ClientId;
use super::{AuthorizationSnafu, MissingSecretSnafu, Result, Secrets, WorkspaceSnafu, store};
use crate::workspace::{Workspace, WorkspaceReader};
use natsume_photo_protocol::{DatasetId, InitializationId, Sha256Digest, SourceId};
use serde::{Deserialize, Serialize};
use snafu::{ResultExt as _, ensure};
use zeroize::Zeroizing;

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

/// Checks local data and the original signing key without network access or writes.
/// # Errors
/// Reports missing credentials or changed local data.
pub async fn diagnose<Access>(workspace: &Workspace<Access>, secrets: &impl Secrets) -> Result<()> {
    workspace.verify_resources().await.context(WorkspaceSnafu)?;
    verify_authorization(workspace, secrets).await.map(|_| ())
}

/// Opens and diagnoses a path with one resource-integrity pass and no network access.
/// # Errors
/// Reports invalid workspace data, authorization or original signing credentials.
pub async fn diagnose_path(
    path: &std::path::Path,
    secrets: &impl Secrets,
) -> Result<WorkspaceReader> {
    let workspace = WorkspaceReader::inspect(path)
        .await
        .context(WorkspaceSnafu)?;
    verify_authorization(&workspace, secrets).await?;
    Ok(workspace)
}

// Resource bytes are verified by WorkspaceReader::inspect or diagnose.
pub(crate) async fn verify_authorization<Access>(
    workspace: &Workspace<Access>,
    secrets: &impl Secrets,
) -> Result<Authorization> {
    let root = workspace.root().to_owned();
    let secrets = secrets.clone();
    let (identity, manifest) = tokio::task::spawn_blocking(move || {
        let identity: Authorization = serde_json::from_slice(&store::read_bounded(
            &root.join("authorization.json"),
            8192,
        )?)
        .map_err(super::json_error)?;
        load_signing_key(&identity, &secrets)?;
        let bytes = store::read_bounded(
            &root.join("initialization.json"),
            natsume_photo_protocol::MANIFEST_LIMIT as u64,
        )?;
        ensure!(
            Sha256Digest::of(&bytes) == identity.manifest_sha256,
            AuthorizationSnafu
        );
        let manifest: natsume_photo_protocol::PhotoInitialization =
            serde_json::from_slice(&bytes).map_err(super::json_error)?;
        manifest
            .validate(&identity.source_id)
            .context(super::ContractSnafu)?;
        ensure!(
            manifest.dataset_id == identity.dataset_id
                && manifest.initialization_id == identity.initialization_id,
            AuthorizationSnafu
        );
        Ok((identity, manifest))
    })
    .await
    .context(super::WorkerSnafu)??;
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
        AuthorizationSnafu
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
        .context(super::CredentialSnafu)?
        .ok_or_else(|| MissingSecretSnafu.build())?;
    let mut seed = Zeroizing::new([0; 32]);
    hex::decode_to_slice(secret.as_str(), seed.as_mut()).map_err(super::key_encoding_error)?;
    let key = ed25519_dalek::SigningKey::from_bytes(&seed);
    ensure!(
        hex::encode(key.verifying_key().to_bytes()) == identity.signing_public_key.as_str(),
        AuthorizationSnafu
    );
    Ok(key)
}
