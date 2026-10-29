use super::{
    AuthorizationSnafu, MissingSecretSnafu, Result, Secrets, WorkspaceSnafu, api, store, wire,
};
use crate::workspace::Workspace;
use serde::{Deserialize, Serialize};
use snafu::{ResultExt as _, ensure};
use zeroize::Zeroizing;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Authorization {
    pub server_url: String,
    pub client_id: String,
    pub signing_public_key: String,
    pub source_id: String,
    pub dataset_id: String,
    pub initialization_id: String,
    pub manifest_sha256: String,
}

/// Checks local data and the original signing key without network access or writes.
/// # Errors
/// Reports missing credentials or changed local data.
pub async fn diagnose(workspace: &Workspace, secrets: &impl Secrets) -> Result<()> {
    verify(workspace, secrets).await.map(|_| ())
}

pub(super) async fn verify(workspace: &Workspace, secrets: &impl Secrets) -> Result<Authorization> {
    let root = workspace.root().to_owned();
    let secrets = secrets.clone();
    let (identity, manifest) = tokio::task::spawn_blocking(move || {
        let identity: Authorization = serde_json::from_slice(&store::read_bounded(
            &root.join("authorization.json"),
            8192,
        )?)
        .map_err(super::json_error)?;
        ensure!(
            api::origin(&identity.server_url)? == identity.server_url
                && wire::id(&identity.client_id)
                && wire::hash(&identity.signing_public_key),
            AuthorizationSnafu
        );
        let secret = secrets
            .load(
                &identity.server_url,
                &format!("{}/signing", identity.client_id),
            )?
            .ok_or_else(|| MissingSecretSnafu.build())?;
        let mut seed = Zeroizing::new([0; 32]);
        hex::decode_to_slice(secret.as_str(), seed.as_mut()).map_err(super::key_encoding_error)?;
        let key = ed25519_dalek::SigningKey::from_bytes(&seed);
        ensure!(
            hex::encode(key.verifying_key().to_bytes()) == identity.signing_public_key,
            AuthorizationSnafu
        );
        let bytes = store::read_bounded(
            &root.join("initialization.json"),
            wire::MANIFEST_LIMIT as u64,
        )?;
        ensure!(
            wire::digest(&bytes) == identity.manifest_sha256,
            AuthorizationSnafu
        );
        let manifest: wire::Manifest = serde_json::from_slice(&bytes).map_err(super::json_error)?;
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
    workspace.verify_resources().await.context(WorkspaceSnafu)?;
    Ok(identity)
}
