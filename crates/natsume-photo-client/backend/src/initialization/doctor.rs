use serde::Deserialize;
use snafu::{ResultExt as _, ensure};
use zeroize::Zeroizing;

use super::{
    InvalidSnafu, JsonSnafu, MissingSecretSnafu, Result, Secrets, WorkspaceSnafu, api, store, wire,
};
use crate::workspace::Workspace;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Authorization {
    server_url: String,
    client_id: String,
    signing_public_key: String,
    source_id: String,
    dataset_id: String,
    initialization_id: String,
    manifest_sha256: String,
}

/// Checks downloaded data and the original signing key without network access or writes.
/// # Errors
/// Reports missing credentials, corrupt authorization records or changed local data.
pub async fn diagnose(workspace: &Workspace, secrets: &impl Secrets) -> Result<()> {
    let identity: Authorization = serde_json::from_slice(&store::read_bounded(
        &workspace.root().join("authorization.json"),
        8192,
    )?)
    .context(JsonSnafu)?;
    ensure!(
        api::origin(&identity.server_url)? == identity.server_url
            && wire::id(&identity.client_id)
            && wire::hash(&identity.signing_public_key),
        InvalidSnafu
    );
    let secret = secrets
        .load(
            &identity.server_url,
            &format!("{}/signing", identity.client_id),
        )?
        .ok_or_else(|| MissingSecretSnafu.build())?;
    let mut seed = Zeroizing::new([0; 32]);
    hex::decode_to_slice(secret.as_str(), seed.as_mut()).map_err(|_| InvalidSnafu.build())?;
    let key = ed25519_dalek::SigningKey::from_bytes(&seed);
    ensure!(
        hex::encode(key.verifying_key().to_bytes()) == identity.signing_public_key,
        InvalidSnafu
    );
    let bytes = store::read_bounded(
        &workspace.root().join("initialization.json"),
        wire::MANIFEST_LIMIT as u64,
    )?;
    ensure!(
        wire::digest(&bytes) == identity.manifest_sha256,
        InvalidSnafu
    );
    let manifest: wire::Manifest = serde_json::from_slice(&bytes).context(JsonSnafu)?;
    manifest.validate(&identity.source_id)?;
    ensure!(
        manifest.dataset_id == identity.dataset_id
            && manifest.initialization_id == identity.initialization_id,
        InvalidSnafu
    );
    let roster = workspace.roster().await.context(WorkspaceSnafu)?;
    let actual: std::collections::BTreeMap<_, _> =
        roster.teams().iter().map(|t| (&t.account_id, t)).collect();
    let expected: std::collections::BTreeMap<_, _> =
        manifest.teams.iter().map(|t| (&t.account_id, t)).collect();
    ensure!(actual == expected, InvalidSnafu);
    for team in &manifest.teams {
        let actual = workspace
            .logo_path(&team.account_id)
            .await
            .context(WorkspaceSnafu)?;
        let expected = manifest
            .school_logos
            .iter()
            .find(|logo| logo.organization_id == team.organization_id);
        match (actual, expected) {
            (None, None) => {}
            (Some(path), Some(logo)) => ensure!(
                wire::digest(&store::read_bounded(&path, wire::IMAGE_LIMIT as u64)?) == logo.sha256,
                InvalidSnafu
            ),
            _ => return InvalidSnafu.fail(),
        }
    }
    Ok(())
}
