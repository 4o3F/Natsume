//! Read-only diagnosis; reusable offline authorization belongs to its own module.
use super::{Result, Secrets, WorkspaceSnafu};
use crate::workspace::{Workspace, WorkspaceReader};
use snafu::ResultExt as _;

/// Opens a writable workspace and checks its local authorization offline.
/// # Errors
/// Reports invalid workspace data, writer conflicts or missing signing credentials.
pub async fn open_authorized(path: &std::path::Path, secrets: &impl Secrets) -> Result<Workspace> {
    open_authorized_with_progress(path, secrets, &crate::progress::Reporter::default()).await
}
/// Opens offline with observable verification stages.
/// # Errors
/// Returns the same validation and credential failures as `open_authorized`.
pub async fn open_authorized_with_progress(
    path: &std::path::Path,
    secrets: &impl Secrets,
    progress: &crate::progress::Reporter,
) -> Result<Workspace> {
    let workspace = Workspace::open_with_progress(path, progress)
        .await
        .context(WorkspaceSnafu)?;
    progress.report(crate::progress::Stage::Verify, "正在校验离线授权", 0, 0);
    super::authorization::verify(&workspace, secrets)
        .await
        .context(super::OfflineAuthorizationSnafu)?;
    progress.report(crate::progress::Stage::Publish, "工作区校验完成", 1, 1);
    Ok(workspace)
}

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
