//! Offline, bounded result export. File handles pin one snapshot independently of retakes.
use crate::{
    initialization::{self, Secrets, authorization},
    workspace::{FinalSnapshot, Workspace},
};
use natsume_photo_protocol::results::{
    ManifestId, ManifestKind, ResultContext, ResultsManifest, SignedResults,
};
use natsume_photo_protocol::{FormatVersion, Sha256Digest};
use serde::{Deserialize, Serialize};
use snafu::{IntoError as _, ResultExt as _, Snafu, ensure};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, Write},
    path::{Component, Path, PathBuf},
};

mod archive;
mod journal;
pub use journal::JournalError;

#[derive(Debug, Snafu)]
pub enum ExportError {
    #[snafu(display("offline export authorization failed: {source}"))]
    Authorization {
        source: initialization::AuthorizationError,
    },
    #[snafu(display("offline export workspace validation failed: {source}"))]
    Workspace { source: crate::workspace::Error },
    #[snafu(display("offline export contract is invalid"))]
    Contract {
        source: natsume_photo_protocol::results::ResultError,
    },
    #[snafu(display("offline export file operation failed"))]
    Io { source: std::io::Error },
    #[snafu(display("offline export ZIP operation failed"))]
    Zip { source: zip::result::ZipError },
    #[snafu(display("offline export journal failed"))]
    Journal { source: JournalError },
    #[snafu(display("offline ZIP was published but its completion record failed"))]
    PublishedButUnrecorded {
        package: ExportedPackage,
        source: JournalError,
    },
    #[snafu(display("another offline export is already active in this workspace"))]
    Busy,
    #[snafu(display("offline export task failed"))]
    Worker { source: tokio::task::JoinError },
    #[snafu(display("offline export target must be an absolute local path outside the workspace"))]
    Path,
    #[snafu(display("offline export content or journal integrity check failed"))]
    Integrity,
    #[snafu(display("offline export target conflicts with existing content"))]
    Conflict,
    #[snafu(display("offline export resource limit exceeded"))]
    Limit,
}
pub type Result<T> = std::result::Result<T, ExportError>;

/// A completely verified and published local ZIP, not a Server receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportedPackage {
    pub manifest_id: ManifestId,
    pub sha256: Sha256Digest,
    pub bytes: u64,
    pub result_count: usize,
}

/// Exports current confirmed results using the original authorized key without network access.
/// The destination parent must exist. Existing different content is never overwritten.
/// # Errors
/// Reports invalid authorization, empty results, file changes, journal/ZIP or publication failures.
pub async fn export_results(
    workspace: &Workspace,
    target: &Path,
    secrets: &impl Secrets,
) -> Result<ExportedPackage> {
    let root = workspace.root().to_owned();
    let ownership = workspace.writable_owner();
    let lease = tokio::task::spawn_blocking(move || ExportLease::acquire(&root, ownership))
        .await
        .context(WorkerSnafu)??;
    let workspace = workspace.share_for_export();
    let target = target.to_owned();
    let secrets = secrets.clone();
    // Detaching the caller must not drop admission while any accepted I/O is still running.
    tokio::spawn(async move {
        prepare_export(workspace, target, secrets, lease)
            .await
            .inspect_err(|error| {
                tracing::error!(error = %error, "Accepted offline export failed");
            })
    })
    .await
    .context(WorkerSnafu)?
}

struct ExportLease {
    _file: File,
    _ownership: crate::workspace::WorkspaceWriter,
}
impl ExportLease {
    fn acquire(root: &Path, ownership: crate::workspace::WorkspaceWriter) -> Result<Self> {
        let path = root.join(".export.lock");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .context(IoSnafu)?;
        ensure!(
            file.metadata().context(IoSnafu)?.is_file()
                && fs::canonicalize(&path).context(IoSnafu)? == path,
            PathSnafu
        );
        match file.try_lock() {
            Ok(()) => Ok(Self {
                _file: file,
                _ownership: ownership,
            }),
            Err(std::fs::TryLockError::WouldBlock) => BusySnafu.fail(),
            Err(std::fs::TryLockError::Error(source)) => Err(IoSnafu.into_error(source)),
        }
    }
}

async fn prepare_export(
    workspace: Workspace,
    target: PathBuf,
    secrets: impl Secrets,
    lease: ExportLease,
) -> Result<ExportedPackage> {
    workspace.verify_resources().await.context(WorkspaceSnafu)?;
    let identity = authorization::verify(&workspace, &secrets)
        .await
        .context(AuthorizationSnafu)?;
    let snapshot = workspace.export_snapshot().await.context(WorkspaceSnafu)?;
    let root = workspace.root().to_owned();
    tokio::task::spawn_blocking(move || {
        export_blocking(lease, &root, &target, &identity, &secrets, snapshot)
    })
    .await
    .context(WorkerSnafu)?
}

fn export_blocking(
    lease: ExportLease,
    root: &Path,
    target: &Path,
    identity: &authorization::Authorization,
    secrets: &impl Secrets,
    mut snapshot: FinalSnapshot,
) -> Result<ExportedPackage> {
    let _lease = lease;
    let target = target_path(root, target)?;
    let key = authorization::load_signing_key(identity, secrets).context(AuthorizationSnafu)?;
    let manifest = ResultsManifest {
        format_version: FormatVersion::V1,
        kind: ManifestKind::TeamResults,
        manifest_id: ManifestId::generate(),
        context: ResultContext {
            source_id: identity.source_id.clone(),
            dataset_id: identity.dataset_id.clone(),
            initialization_id: identity.initialization_id.clone(),
            initialization_sha256: identity.manifest_sha256.clone(),
        },
        signing_public_key: identity.signing_public_key.clone(),
        results: snapshot.entries,
    };
    manifest.validate().context(ContractSnafu)?;
    let (signed, fingerprint) = journal::prepare(root, &manifest, &key).context(JournalSnafu)?;
    drop(key);
    let json = signed.to_json().context(ContractSnafu)?;
    let parent = target.parent().ok_or_else(|| PathSnafu.build())?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".photo-export-")
        .tempfile_in(parent)
        .context(IoSnafu)?;
    archive::write(temporary.as_file_mut(), &json, &mut snapshot.images)?;
    temporary.as_file().sync_all().context(IoSnafu)?;
    archive::verify(temporary.as_file_mut(), &signed)?;
    let (bytes, sha256) = archive::digest(temporary.as_file_mut())?;
    let result = ExportedPackage {
        manifest_id: signed.manifest.manifest_id,
        sha256,
        bytes,
        result_count: signed.manifest.results.len(),
    };
    match temporary.persist_noclobber(&target) {
        Ok(_) => {}
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            drop(error.file);
            let mut existing = open_regular(&target)?;
            let (bytes, hash) = archive::digest(&mut existing)?;
            ensure!(
                bytes == result.bytes && hash == result.sha256,
                ConflictSnafu
            );
        }
        Err(error) => return Err(IoSnafu.into_error(error.error)),
    }
    journal::complete(root, &fingerprint, &result).context(PublishedButUnrecordedSnafu {
        package: result.clone(),
    })?;
    tracing::info!(
        results = result.result_count,
        bytes = result.bytes,
        "Offline Photo ZIP published and verified"
    );
    Ok(result)
}

fn target_path(root: &Path, target: &Path) -> Result<PathBuf> {
    use std::path::Prefix;
    let local = |path: &Path| matches!(path.components().next(), Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)));
    ensure!(
        target.is_absolute()
            && local(target)
            && !target
                .components()
                .any(|c| matches!(c, Component::ParentDir | Component::CurDir)),
        PathSnafu
    );
    let parent = target.parent().ok_or_else(|| PathSnafu.build())?;
    let name = target.file_name().ok_or_else(|| PathSnafu.build())?;
    ensure!(!name.to_string_lossy().contains(':'), PathSnafu);
    let target = fs::canonicalize(parent).context(IoSnafu)?.join(name);
    ensure!(local(&target) && !target.starts_with(root), PathSnafu);
    Ok(target)
}

fn open_regular(path: &Path) -> Result<File> {
    use std::os::windows::fs::OpenOptionsExt as _;
    let metadata = fs::symlink_metadata(path).context(IoSnafu)?;
    ensure!(
        metadata.is_file()
            && !metadata.is_symlink()
            && fs::canonicalize(path).context(IoSnafu)? == path,
        PathSnafu
    );
    OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(path)
        .context(IoSnafu)
}

#[cfg(test)]
#[path = "../tests/unit/export.rs"]
mod tests;
