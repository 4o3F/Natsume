//! Current confirmed results; complete file publication precedes the short database commit.
use super::db::{Database, PersistenceError};
use crate::photo::{ConfirmationId, PhotoDimensions, PhotoFormat, ResultId, SubmissionId};
use sha2::{Digest as _, Sha256};
use snafu::{IntoError as _, ResultExt as _, Snafu, ensure};
use std::{
    fs::{self, File, OpenOptions},
    io::{BufReader, Read, Seek as _, Write},
    os::windows::fs::OpenOptionsExt as _,
    path::{Path, PathBuf},
};

const IMAGE_LIMIT: u64 = 50 * 1024 * 1024;

#[derive(Debug, Snafu)]
pub enum FinalPhotoError {
    #[snafu(display("confirmed photo operations are unavailable"))]
    Operation { source: std::sync::PoisonError<()> },
    #[snafu(display("confirmed photo revision cannot advance"))]
    Revision,
    #[snafu(display("confirmed photo encoded metadata is invalid"))]
    Decode { source: image::ImageError },
    #[snafu(display("confirmed photo digest is invalid"))]
    Value {
        source: natsume_photo_protocol::ValueError,
    },
    #[snafu(display("confirmed photo file operation failed"))]
    Io { source: std::io::Error },
    #[snafu(display("confirmed photo database operation failed"))]
    Persistence { source: PersistenceError },
    #[snafu(display("confirmed photo path is outside its permitted directory"))]
    Path,
    #[snafu(display("confirmed photo content or metadata does not match its reference"))]
    Integrity,
    #[snafu(display("confirmed photo exceeds its image byte limit"))]
    Limit,
    #[snafu(display("the current team result changed or the confirmation identity was reused"))]
    Conflict,
    #[snafu(display("confirmed photo task failed"))]
    Worker { source: tokio::task::JoinError },
}
type Result<T> = std::result::Result<T, FinalPhotoError>;

/// One durably committed current team result; its image path is workspace-relative.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmedPhoto {
    pub(super) account_id: String,
    pub(super) confirmation_id: ConfirmationId,
    pub(super) result_id: ResultId,
    pub(super) submission_id: SubmissionId,
    pub(super) path: PathBuf,
    pub(super) sha256: String,
    pub(super) bytes: u64,
    pub(super) dimensions: PhotoDimensions,
    pub(super) revision: i64,
}
impl ConfirmedPhoto {
    pub(super) fn into_result(
        self,
        account: String,
    ) -> Result<natsume_photo_protocol::results::TeamResult> {
        Ok(natsume_photo_protocol::results::TeamResult {
            result_id: self.result_id,
            submission_id: self.submission_id,
            account_id: self.account_id,
            account,
            image_sha256: self.sha256.parse().context(ValueSnafu)?,
            image_bytes: self.bytes,
            image_format: self.dimensions.format,
            width: self.dimensions.width,
            height: self.dimensions.height,
        })
    }
    #[must_use]
    pub fn account_id(&self) -> &str {
        &self.account_id
    }
    #[must_use]
    pub fn result_id(&self) -> &ResultId {
        &self.result_id
    }
    #[must_use]
    pub fn submission_id(&self) -> &SubmissionId {
        &self.submission_id
    }
    /// Returns a validated path relative to the owning workspace.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
    #[must_use]
    pub fn sha256(&self) -> &str {
        &self.sha256
    }
    #[must_use]
    pub fn bytes(&self) -> u64 {
        self.bytes
    }
    #[must_use]
    pub fn dimensions(&self) -> PhotoDimensions {
        self.dimensions
    }
    #[must_use]
    pub fn revision(&self) -> i64 {
        self.revision
    }
}

pub(crate) struct FinalDraft {
    pub account_id: String,
    pub confirmation_id: ConfirmationId,
    pub source: PathBuf,
    pub sha256: String,
    pub bytes: u64,
    pub dimensions: PhotoDimensions,
}
#[derive(Clone)]
pub(crate) struct FinalPhotos {
    root: PathBuf,
    database: Database,
    operations: std::sync::Arc<std::sync::Mutex<()>>,
}
struct PinnedPhotos {
    references: Vec<ConfirmedPhoto>,
    images: Vec<(ConfirmedPhoto, File)>,
}
pub(crate) struct SnapshotImage {
    pub entry: natsume_photo_protocol::results::TeamResult,
    pub file: File,
}
pub(crate) struct FinalSnapshot {
    pub entries: Vec<natsume_photo_protocol::results::TeamResult>,
    pub images: Vec<SnapshotImage>,
}
impl FinalPhotos {
    pub(crate) fn snapshot(
        &self,
        accounts: &std::collections::BTreeMap<String, String>,
    ) -> Result<FinalSnapshot> {
        let pinned = self.pin_references()?;
        let mut snapshot = FinalSnapshot {
            entries: Vec::new(),
            images: Vec::new(),
        };
        for photo in pinned.references {
            let account = accounts
                .get(&photo.account_id)
                .ok_or_else(|| IntegritySnafu.build())?;
            let entry = photo.into_result(account.clone())?;
            snapshot.entries.push(entry);
        }
        for (photo, file) in pinned.images {
            let account = accounts
                .get(&photo.account_id)
                .ok_or_else(|| IntegritySnafu.build())?;
            snapshot.images.push(SnapshotImage {
                entry: photo.into_result(account.clone())?,
                file,
            });
        }
        snapshot
            .images
            .sort_by(|a, b| a.entry.image_sha256.cmp(&b.entry.image_sha256));
        Ok(snapshot)
    }
    pub(super) fn new(
        root: PathBuf,
        database: Database,
        operations: std::sync::Arc<std::sync::Mutex<()>>,
    ) -> Self {
        Self {
            root,
            database,
            operations,
        }
    }
    pub(super) fn lock_operations(&self) -> Result<std::sync::MutexGuard<'_, ()>> {
        self.operations
            .lock()
            .map_err(|_| OperationSnafu.into_error(std::sync::PoisonError::new(())))
    }
    pub(crate) fn save(&self, draft: &FinalDraft) -> Result<ConfirmedPhoto> {
        let _operation = self.lock_operations()?;
        ensure!(draft.bytes > 0 && draft.bytes <= IMAGE_LIMIT, LimitSnafu);
        let _: natsume_photo_protocol::Sha256Digest = draft.sha256.parse().context(ValueSnafu)?;
        ensure!(
            draft.dimensions.width > 0
                && draft.dimensions.height > 0
                && u64::from(draft.dimensions.width) * u64::from(draft.dimensions.height)
                    <= 80_000_000,
            IntegritySnafu
        );
        let current = self
            .database
            .final_photos(Some(&draft.account_id))
            .context(PersistenceSnafu)?
            .pop();
        let baseline = current.as_ref().map_or(0, |photo| photo.revision);
        if let Some(current) = &current
            && current.confirmation_id == draft.confirmation_id
        {
            ensure!(
                current.sha256 == draft.sha256
                    && current.bytes == draft.bytes
                    && current.dimensions == draft.dimensions,
                ConflictSnafu
            );
            self.verify(current)?;
            return Ok(current.clone());
        }
        let mut source = self.open_source(&draft.source)?;
        let directory = self.root.join("finals");
        fs::create_dir_all(&directory).context(IoSnafu)?;
        ensure!(
            fs::canonicalize(&directory).context(IoSnafu)? == directory,
            PathSnafu
        );
        let mut temporary = tempfile::Builder::new()
            .prefix(".final-")
            .tempfile_in(&directory)
            .context(IoSnafu)?;
        verify_bytes(
            &mut source,
            Some(temporary.as_file_mut()),
            draft.bytes,
            &draft.sha256,
        )?;
        verify_dimensions(&mut source, draft.dimensions)?;
        temporary.as_file().sync_all().context(IoSnafu)?;
        let extension = match draft.dimensions.format {
            PhotoFormat::Jpeg => "jpg",
            PhotoFormat::Png => "png",
        };
        let relative = PathBuf::from(format!("finals/{}.{extension}", draft.sha256));
        let target = self.root.join(&relative);
        match temporary.persist_noclobber(&target) {
            Ok(_) => {}
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                drop(error.file);
                verify_bytes(
                    &mut open_regular(&target)?,
                    None,
                    draft.bytes,
                    &draft.sha256,
                )?;
            }
            Err(error) => return Err(IoSnafu.into_error(error.error)),
        }
        let photo = ConfirmedPhoto {
            account_id: draft.account_id.clone(),
            confirmation_id: draft.confirmation_id.clone(),
            result_id: ResultId::generate(),
            submission_id: SubmissionId::generate(),
            path: relative,
            sha256: draft.sha256.clone(),
            bytes: draft.bytes,
            dimensions: draft.dimensions,
            revision: baseline
                .checked_add(1)
                .ok_or_else(|| RevisionSnafu.build())?,
        };
        Self::check_reference(&photo)?;
        ensure!(
            self.database
                .apply_final(&photo, baseline)
                .context(PersistenceSnafu)?,
            ConflictSnafu
        );
        Ok(photo)
    }
    pub(super) fn list(&self) -> Result<Vec<ConfirmedPhoto>> {
        let pinned = self.pin_references()?;
        for (photo, mut file) in pinned.images {
            verify_bytes(&mut file, None, photo.bytes, &photo.sha256)?;
            verify_dimensions(&mut file, photo.dimensions)?;
        }
        Ok(pinned.references)
    }
    fn references_locked(&self) -> Result<Vec<ConfirmedPhoto>> {
        let photos = self.database.final_photos(None).context(PersistenceSnafu)?;
        let known = self.database.roster_ids().context(PersistenceSnafu)?;
        let mut verified = std::collections::BTreeMap::new();
        for photo in &photos {
            ensure!(known.contains(&photo.account_id), IntegritySnafu);
            Self::check_reference(photo)?;
            if let Some(existing) = verified.get(&photo.path) {
                ensure!(*existing == (photo.bytes, photo.dimensions), IntegritySnafu);
            } else {
                verified.insert(photo.path.clone(), (photo.bytes, photo.dimensions));
            }
        }
        Ok(photos)
    }
    fn open_final(&self, photo: &ConfirmedPhoto) -> Result<File> {
        Self::check_reference(photo)?;
        let path = self.root.join(&photo.path);
        ensure!(fs::canonicalize(&path).context(IoSnafu)? == path, PathSnafu);
        let file = open_regular(&path)?;
        ensure!(
            file.metadata().context(IoSnafu)?.len() == photo.bytes,
            IntegritySnafu
        );
        Ok(file)
    }
    fn pin_references(&self) -> Result<PinnedPhotos> {
        let _operation = self.lock_operations()?;
        let references = self.references_locked()?;
        let mut images = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for photo in &references {
            if seen.insert(&photo.path) {
                images.push((photo.clone(), self.open_final(photo)?));
            }
        }
        Ok(PinnedPhotos { references, images })
    }
    pub(crate) fn verify_cleanup_result(&self, saved: &ConfirmedPhoto) -> Result<()> {
        let (current, mut file) = {
            let _operation = self.lock_operations()?;
            let current = self
                .database
                .final_photos(Some(&saved.account_id))
                .context(PersistenceSnafu)?
                .pop()
                .ok_or_else(|| IntegritySnafu.build())?;
            let file = self.open_final(&current)?;
            (current, file)
        };
        verify_bytes(&mut file, None, current.bytes, &current.sha256)?;
        verify_dimensions(&mut file, current.dimensions)
    }
    /// Re-read committed references before collecting reserved, content-addressed objects.
    fn unused_paths(&self) -> Result<Vec<PathBuf>> {
        let referenced: std::collections::BTreeSet<_> = self
            .references_locked()?
            .into_iter()
            .map(|photo| self.root.join(photo.path))
            .collect();
        let directory = self.root.join("finals");
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(source) => return Err(IoSnafu.into_error(source)),
        };
        ensure!(
            fs::canonicalize(&directory).context(IoSnafu)? == directory,
            PathSnafu
        );
        let mut unused = Vec::new();
        for entry in entries {
            let entry = entry.context(IoSnafu)?;
            let path = entry.path();
            let Some(hash) = path.file_stem().and_then(|value| value.to_str()) else {
                continue;
            };
            if matches!(
                path.extension().and_then(|value| value.to_str()),
                Some("jpg" | "png")
            ) && hash.parse::<natsume_photo_protocol::Sha256Digest>().is_ok()
                && !referenced.contains(&path)
                && entry.file_type().context(IoSnafu)?.is_file()
            {
                unused.push(path);
            }
        }
        Ok(unused)
    }
    /// Shares the save/publication fence across every session of this workspace.
    pub(crate) fn remove_unused(&self) -> Result<(usize, Vec<(PathBuf, std::io::Error)>)> {
        let _operation = self.lock_operations()?;
        let mut removed = 0;
        let mut failures = Vec::new();
        for path in self.unused_paths()? {
            ensure!(fs::canonicalize(&path).context(IoSnafu)? == path, PathSnafu);
            match fs::remove_file(&path) {
                Ok(()) => removed += 1,
                Err(source) => failures.push((path, source)),
            }
        }
        Ok((removed, failures))
    }
    fn check_reference(photo: &ConfirmedPhoto) -> Result<()> {
        let ext = match photo.dimensions.format {
            PhotoFormat::Jpeg => "jpg",
            PhotoFormat::Png => "png",
        };
        ensure!(
            photo.path == Path::new(&format!("finals/{}.{ext}", photo.sha256))
                && photo.bytes > 0
                && photo.bytes <= IMAGE_LIMIT
                && photo.dimensions.width > 0
                && photo.dimensions.height > 0
                && u64::from(photo.dimensions.width) * u64::from(photo.dimensions.height)
                    <= 80_000_000,
            IntegritySnafu
        );
        Ok(())
    }
    fn verify(&self, photo: &ConfirmedPhoto) -> Result<()> {
        Self::check_reference(photo)?;
        let path = self.root.join(&photo.path);
        ensure!(fs::canonicalize(&path).context(IoSnafu)? == path, PathSnafu);
        let mut file = open_regular(&path)?;
        verify_bytes(&mut file, None, photo.bytes, &photo.sha256)?;
        verify_dimensions(&mut file, photo.dimensions)
    }
    fn open_source(&self, path: &Path) -> Result<File> {
        ensure!(
            path.starts_with(self.root.join("captures"))
                && fs::canonicalize(path).context(IoSnafu)? == path,
            PathSnafu
        );
        open_regular(path)
    }
}
fn open_regular(path: &Path) -> Result<File> {
    let metadata = fs::symlink_metadata(path).context(IoSnafu)?;
    ensure!(metadata.is_file() && !metadata.is_symlink(), PathSnafu);
    OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(path)
        .context(IoSnafu)
}
fn verify_bytes(
    file: &mut File,
    mut destination: Option<&mut File>,
    expected: u64,
    sha256: &str,
) -> Result<()> {
    ensure!(
        file.metadata().context(IoSnafu)?.len() <= IMAGE_LIMIT,
        LimitSnafu
    );
    let mut buffer = vec![0; 64 * 1024];
    let mut hash = Sha256::new();
    let mut bytes = 0;
    loop {
        let read = file.read(&mut buffer).context(IoSnafu)?;
        if read == 0 {
            break;
        }
        bytes += read as u64;
        ensure!(bytes <= IMAGE_LIMIT, LimitSnafu);
        hash.update(&buffer[..read]);
        if let Some(output) = &mut destination {
            output.write_all(&buffer[..read]).context(IoSnafu)?;
        }
    }
    ensure!(
        bytes == expected && hex::encode(hash.finalize()) == sha256,
        IntegritySnafu
    );
    Ok(())
}

fn verify_dimensions(file: &mut File, dimensions: PhotoDimensions) -> Result<()> {
    file.rewind().context(IoSnafu)?;
    let mut reader = image::ImageReader::new(BufReader::new(file))
        .with_guessed_format()
        .context(IoSnafu)?;
    let format = match dimensions.format {
        PhotoFormat::Jpeg => image::ImageFormat::Jpeg,
        PhotoFormat::Png => image::ImageFormat::Png,
    };
    ensure!(reader.format() == Some(format), IntegritySnafu);
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    ensure!(
        reader.into_dimensions().context(DecodeSnafu)? == (dimensions.width, dimensions.height),
        IntegritySnafu
    );
    Ok(())
}
