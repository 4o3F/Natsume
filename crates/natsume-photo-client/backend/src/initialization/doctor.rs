//! Read-only diagnosis; reusable offline authorization belongs to its own module.
use super::{Result, Secrets, WorkspaceSnafu};
use crate::workspace::{Workspace, WorkspaceReader};
use snafu::ResultExt as _;

/// Checks local data and the original signing key without network access or writes.
/// # Errors
/// Reports missing credentials or changed local data.
pub async fn diagnose<Access>(workspace: &Workspace<Access>, secrets: &impl Secrets) -> Result<()> {
    workspace.verify_resources().await.context(WorkspaceSnafu)?;
    super::authorization::verify(workspace, secrets)
        .await
        .context(super::OfflineAuthorizationSnafu)
        .map(|_| ())
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
    super::authorization::verify(&workspace, secrets)
        .await
        .context(super::OfflineAuthorizationSnafu)?;
    Ok(workspace)
}
