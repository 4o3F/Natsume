//! Current identity projections and bounded, disk-backed image/ZIP delivery.
use super::{PhotoComponent, db, results_storage};
use crate::{component::contest::PhotoTeam, db::Transaction, lifecycle::Admission};
use natsume_photo_protocol::{
    Sha256Digest,
    results::{PACKAGE_LIMIT, PhotoFormat, RESULT_IMAGES_LIMIT},
};
use sha2::{Digest as _, Sha256};
use snafu::{ResultExt as _, Snafu, ensure};
use std::{
    collections::BTreeSet,
    fs::File,
    io::{Read as _, Seek as _},
    time::{Duration, Instant},
};
use tokio::sync::OwnedSemaphorePermit;

#[derive(Debug, Snafu)]
pub(crate) enum DeliveryError {
    #[snafu(display("Photo delivery capacity is exhausted"))]
    Busy,
    #[snafu(display("Photo delivery request is invalid"))]
    Invalid,
    #[snafu(display("Current Photo does not exist"))]
    NotFound,
    #[snafu(display("Photo export exceeds its resource limit"))]
    Limit,
    #[snafu(display("Photo export names are unsafe or ambiguous"))]
    Names,
    #[snafu(display("Photo object integrity check failed"))]
    Integrity,
    #[snafu(display("Photo delivery was interrupted"))]
    Interrupted,
    #[snafu(display("Photo delivery persistence failed"))]
    Persistence { source: db::PersistenceError },
    #[snafu(display("Photo delivery storage failed"))]
    Storage {
        source: results_storage::ResultsStorageError,
    },
    #[snafu(display("Photo delivery file operation failed"))]
    Io { source: std::io::Error },
    #[snafu(display("Photo ZIP export failed"))]
    Zip { source: zip::result::ZipError },
    #[snafu(display("Photo delivery worker failed"))]
    Worker {
        #[snafu(source(from(tokio::task::JoinError, crate::task::TaskFailure::from)))]
        source: crate::task::TaskFailure,
    },
}
impl From<crate::db::PersistenceError> for DeliveryError {
    fn from(source: crate::db::PersistenceError) -> Self {
        Self::Persistence {
            source: source.into(),
        }
    }
}
type Result<T> = std::result::Result<T, DeliveryError>;

#[derive(serde::Serialize, specta::Type)]
pub(crate) struct PhotoRecord {
    pub account_id: String,
    pub account: String,
    pub source_id: String,
    pub submission_id: String,
    #[specta(type = specta_typescript::Number)]
    pub revision: i64,
    pub image_sha256: Sha256Digest,
    #[specta(type = String)]
    pub image_format: PhotoFormat,
    #[specta(type = specta_typescript::Number)]
    pub image_bytes: u64,
}
impl PhotoRecord {
    fn valid(&self, roster: &[PhotoTeam]) -> bool {
        roster
            .iter()
            .any(|team| team.account_id == self.account_id && team.account == self.account)
    }
}
#[derive(Default, serde::Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(crate) struct PhotoQuery {
    pub after_account_id: Option<String>,
    pub limit: Option<u16>,
}
#[derive(serde::Serialize, specta::Type)]
pub(crate) struct PhotoPage {
    pub entries: Vec<PhotoRecord>,
    pub next: Option<String>,
}

pub(crate) struct PreparedDownload {
    pub file: File,
    pub length: u64,
    pub content_type: &'static str,
    pub permit: OwnedSemaphorePermit,
}

impl PhotoComponent {
    pub(crate) fn photos_in(
        tx: &mut Transaction<'_>,
        query: &PhotoQuery,
        roster: &[PhotoTeam],
    ) -> Result<PhotoPage> {
        let limit = query.limit.unwrap_or(50);
        ensure!(
            (1..=100).contains(&limit)
                && query
                    .after_account_id
                    .as_ref()
                    .is_none_or(|id| !id.is_empty() && id.len() <= 512),
            InvalidSnafu
        );
        let mut entries = db::delivery::read(
            tx,
            query.after_account_id.as_deref(),
            None,
            i64::from(limit) + 1,
        )
        .context(PersistenceSnafu)?;
        let next = if entries.len() > usize::from(limit) {
            entries.pop();
            entries.last().map(|entry| entry.account_id.clone())
        } else {
            None
        };
        entries.retain(|entry| entry.valid(roster));
        Ok(PhotoPage { entries, next })
    }

    pub(crate) fn photo_in(
        tx: &mut Transaction<'_>,
        id: &str,
        roster: &[PhotoTeam],
    ) -> Result<PhotoRecord> {
        ensure!(!id.is_empty() && id.len() <= 512, InvalidSnafu);
        db::delivery::read(tx, None, Some(id), 1)
            .context(PersistenceSnafu)?
            .pop()
            .filter(|entry| entry.valid(roster))
            .ok_or_else(|| NotFoundSnafu.build())
    }

    pub(crate) fn export_snapshot_in(
        tx: &mut Transaction<'_>,
        roster: &[PhotoTeam],
    ) -> Result<Vec<PhotoRecord>> {
        let entries = db::delivery::read(
            tx,
            None,
            None,
            i64::try_from(natsume_photo_protocol::TEAM_LIMIT).map_err(|_| LimitSnafu.build())? + 1,
        )
        .context(PersistenceSnafu)?;
        ensure!(
            entries.len() <= natsume_photo_protocol::TEAM_LIMIT,
            LimitSnafu
        );
        Ok(entries
            .into_iter()
            .filter(|entry| entry.valid(roster))
            .collect())
    }

    pub(crate) fn reserve_export(&self) -> Result<OwnedSemaphorePermit> {
        self.export_work
            .clone()
            .try_acquire_owned()
            .map_err(|_| BusySnafu.build())
    }

    pub(crate) async fn prepare_export(
        &self,
        entries: Vec<PhotoRecord>,
        permit: OwnedSemaphorePermit,
        admission: Admission,
    ) -> Result<PreparedDownload> {
        let store = self.results.clone();
        tokio::task::spawn_blocking(move || {
            let deadline = Instant::now() + Duration::from_mins(15);
            let mut names = BTreeSet::new();
            let mut total = 0_u64;
            let mut inputs = Vec::with_capacity(entries.len());
            for entry in entries {
                check(&admission, deadline)?;
                let name = export_name(&entry)?;
                ensure!(names.insert(name.to_uppercase()), NamesSnafu);
                total = total
                    .checked_add(entry.image_bytes)
                    .ok_or_else(|| LimitSnafu.build())?;
                ensure!(total <= RESULT_IMAGES_LIMIT, LimitSnafu);
                let file = store.open_object(&entry).context(StorageSnafu)?;
                inputs.push((entry, name, file));
            }
            let file = store.export_file().context(StorageSnafu)?;
            let mut writer = zip::ZipWriter::new(BoundedFile(file));
            for (entry, name, mut source) in inputs {
                check(&admission, deadline)?;
                writer
                    .start_file(
                        name,
                        zip::write::SimpleFileOptions::default()
                            .compression_method(zip::CompressionMethod::Stored)
                            .large_file(true),
                    )
                    .context(ZipSnafu)?;
                copy(&mut source, &mut writer, &entry, &admission, deadline)?;
            }
            let mut file = writer.finish().context(ZipSnafu)?.0;
            file.sync_all().context(IoSnafu)?;
            let length = file.metadata().context(IoSnafu)?.len();
            ensure!(length <= PACKAGE_LIMIT, LimitSnafu);
            file.rewind().context(IoSnafu)?;
            check(&admission, deadline)?;
            Ok(PreparedDownload {
                file,
                length,
                content_type: "application/zip",
                permit,
            })
        })
        .await
        .context(WorkerSnafu)?
    }

    pub(crate) async fn prepare_image(
        &self,
        entry: PhotoRecord,
        admission: Admission,
    ) -> Result<PreparedDownload> {
        let permit = self
            .result_download_work
            .clone()
            .try_acquire_owned()
            .map_err(|_| BusySnafu.build())?;
        let store = self.results.clone();
        tokio::task::spawn_blocking(move || {
            let mut file = store.open_object(&entry).context(StorageSnafu)?;
            copy(
                &mut file,
                &mut std::io::sink(),
                &entry,
                &admission,
                Instant::now() + Duration::from_mins(15),
            )?;
            file.rewind().context(IoSnafu)?;
            let content_type = match entry.image_format {
                PhotoFormat::Jpeg => "image/jpeg",
                PhotoFormat::Png => "image/png",
            };
            Ok(PreparedDownload {
                file,
                length: entry.image_bytes,
                content_type,
                permit,
            })
        })
        .await
        .context(WorkerSnafu)?
    }
}

fn check(admission: &Admission, deadline: Instant) -> Result<()> {
    ensure!(
        !admission.is_stopping() && Instant::now() < deadline,
        InterruptedSnafu
    );
    Ok(())
}

fn copy(
    source: &mut File,
    target: &mut impl std::io::Write,
    entry: &PhotoRecord,
    admission: &Admission,
    deadline: Instant,
) -> Result<()> {
    ensure!(
        source.metadata().context(IoSnafu)?.len() == entry.image_bytes,
        IntegritySnafu
    );
    let mut remaining = entry.image_bytes;
    let mut buffer = vec![0_u8; 64 * 1024];
    let mut hash = Sha256::new();
    while remaining != 0 {
        check(admission, deadline)?;
        let capacity = usize::try_from(remaining.min(64 * 1024)).map_err(|_| LimitSnafu.build())?;
        let count = source.read(&mut buffer[..capacity]).context(IoSnafu)?;
        ensure!(count != 0, IntegritySnafu);
        remaining -= u64::try_from(count).map_err(|_| LimitSnafu.build())?;
        hash.update(&buffer[..count]);
        target.write_all(&buffer[..count]).context(IoSnafu)?;
    }
    ensure!(
        hex::encode(hash.finalize()) == entry.image_sha256.as_str()
            && source.metadata().context(IoSnafu)?.len() == entry.image_bytes,
        IntegritySnafu
    );
    Ok(())
}

fn export_name(entry: &PhotoRecord) -> Result<String> {
    let id = &entry.account;
    ensure!(
        !id.is_empty()
            && !id.ends_with([' ', '.'])
            && !id
                .chars()
                .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c)),
        NamesSnafu
    );
    let stem = id
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end_matches(' ')
        .to_uppercase();
    let numbered = stem
        .strip_prefix("COM")
        .or_else(|| stem.strip_prefix("LPT"))
        .is_some_and(|n| {
            matches!(
                n,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        });
    ensure!(
        !matches!(
            stem.as_str(),
            "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$" | "CONIN$" | "CONOUT$"
        ) && !numbered,
        NamesSnafu
    );
    let name = format!("{id}.{}", entry.image_format.extension());
    ensure!(
        name.len() <= 255 && name.encode_utf16().count() <= 255,
        NamesSnafu
    );
    Ok(name)
}

struct BoundedFile(File);
impl std::io::Write for BoundedFile {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let end = self
            .0
            .stream_position()?
            .checked_add(u64::try_from(buffer.len()).map_err(std::io::Error::other)?);
        if end.is_none_or(|end| end > PACKAGE_LIMIT) {
            return Err(std::io::Error::other("Photo export byte limit"));
        }
        self.0.write(buffer)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}
impl std::io::Seek for BoundedFile {
    fn seek(&mut self, position: std::io::SeekFrom) -> std::io::Result<u64> {
        self.0.seek(position)
    }
}
