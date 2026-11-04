//! Current identity projections and bounded, disk-backed image/ZIP delivery.
use super::{PhotoComponent, PhotoRoster, db, results_storage};
use crate::{
    component::contest::ContestComponent,
    db::{Transaction, TransactionError},
    lifecycle::Admission,
};
use natsume_photo_protocol::{
    Sha256Digest, SourceId,
    results::{PACKAGE_LIMIT, PhotoFormat, RESULT_IMAGES_LIMIT, SubmissionId},
};
use snafu::{OptionExt as _, ResultExt as _, Snafu, ensure};
use std::{collections::BTreeSet, fs::File, io::Seek as _, time::Instant};
use tokio::sync::OwnedSemaphorePermit;

mod pyrite;

#[derive(Debug, Snafu)]
pub(crate) enum DeliveryError {
    #[snafu(display("Photo roster could not be read"))]
    Roster {
        source: crate::component::contest::ContestError,
    },
    #[snafu(display("Server is stopping"))]
    Stopping,
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
    #[snafu(display("Photo export image conversion failed"))]
    Image { source: image::ImageError },
    #[snafu(display("Photo export roster encoding failed"))]
    Json { source: serde_json::Error },
    #[snafu(display("Photo export logo preparation failed"))]
    Logo {
        source: crate::component::contest::PhotoLogoError,
    },
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
    pub source_id: SourceId,
    pub submission_id: SubmissionId,
    #[specta(type = specta_typescript::Number)]
    pub revision: i64,
    pub image_sha256: Sha256Digest,
    #[specta(type = String)]
    pub image_format: PhotoFormat,
    #[specta(type = specta_typescript::Number)]
    pub image_bytes: u64,
}
pub(crate) struct PreparedDownload {
    pub file: File,
    pub length: u64,
    pub content_type: &'static str,
    pub permit: OwnedSemaphorePermit,
}

impl PhotoComponent {
    fn photos_in(tx: &mut Transaction<'_>, roster: &PhotoRoster) -> Result<Vec<PhotoRecord>> {
        db::delivery::current_photos(tx, roster, None).context(PersistenceSnafu)
    }

    fn photo_in(tx: &mut Transaction<'_>, id: &str, roster: &PhotoRoster) -> Result<PhotoRecord> {
        ensure!(
            !id.is_empty() && id.len() <= super::limits::ACCOUNT_ID_BYTES,
            InvalidSnafu
        );
        db::delivery::current_photo(tx, roster, id)
            .context(PersistenceSnafu)?
            .ok_or_else(|| NotFoundSnafu.build())
    }

    fn export_snapshot_in(
        tx: &mut Transaction<'_>,
        roster: &PhotoRoster,
    ) -> Result<Vec<PhotoRecord>> {
        let entries = db::delivery::current_photos(
            tx,
            roster,
            Some(
                i64::try_from(natsume_photo_protocol::TEAM_LIMIT)
                    .map_err(|_| LimitSnafu.build())?
                    + 1,
            ),
        )
        .context(PersistenceSnafu)?;
        ensure!(
            entries.len() <= natsume_photo_protocol::TEAM_LIMIT,
            LimitSnafu
        );
        Ok(entries)
    }

    pub(crate) fn reserve_export(&self) -> Result<OwnedSemaphorePermit> {
        self.export_slots
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
            let deadline = Instant::now() + super::limits::OPERATION_TIMEOUT;
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
                start_entry(&mut writer, &name, &admission, deadline)?;
                copy(&mut source, &mut writer, &entry, &admission, deadline)?;
            }
            finish_export(writer, permit, &admission, deadline)
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
            .photo_download_slots
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
                Instant::now() + super::limits::OPERATION_TIMEOUT,
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
    let mut copied = 0;
    super::transfer::copy_verified(
        source,
        target,
        super::transfer::Expectation::Image {
            bytes: entry.image_bytes,
            sha256: &entry.image_sha256,
        },
        &mut copied,
        || !admission.is_stopping() && Instant::now() < deadline,
    )?;
    Ok(())
}

impl From<super::transfer::TransferError> for DeliveryError {
    fn from(error: super::transfer::TransferError) -> Self {
        use super::transfer::TransferError as Error;
        match error {
            Error::Read { source } | Error::Write { source } => Self::Io { source },
            Error::Limit => Self::Limit,
            Error::Integrity => Self::Integrity,
            Error::Interrupted => Self::Interrupted,
        }
    }
}

fn export_name(entry: &PhotoRecord) -> Result<String> {
    account_name(&entry.account, entry.image_format.extension())
}

fn account_name(id: &str, extension: &str) -> Result<String> {
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
    let name = format!("{id}.{extension}");
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

fn start_entry(
    writer: &mut zip::ZipWriter<BoundedFile>,
    name: &str,
    admission: &Admission,
    deadline: Instant,
) -> Result<()> {
    check(admission, deadline)?;
    writer
        .start_file(
            name,
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored)
                .large_file(true),
        )
        .context(ZipSnafu)
}

fn finish_export(
    writer: zip::ZipWriter<BoundedFile>,
    permit: OwnedSemaphorePermit,
    admission: &Admission,
    deadline: Instant,
) -> Result<PreparedDownload> {
    let mut file = writer.finish().context(ZipSnafu)?.0;
    file.sync_all().context(IoSnafu)?;
    let length = file.metadata().context(IoSnafu)?.len();
    file.rewind().context(IoSnafu)?;
    check(admission, deadline)?;
    Ok(PreparedDownload {
        file,
        length,
        content_type: "application/zip",
        permit,
    })
}

impl PhotoComponent {
    pub(crate) async fn list(&self) -> Result<Vec<PhotoRecord>> {
        self.database
            .read(move |tx| {
                let roster = super::load_roster(tx).context(RosterSnafu)?;
                Self::photos_in(tx, &roster)
            })
            .await
            .map_err(TransactionError::into_error)
    }
    pub(crate) async fn image(&self, id: String) -> Result<(PreparedDownload, Admission)> {
        let admission = self.lifecycle.admit().context(StoppingSnafu)?;
        let record = self
            .database
            .read(move |tx| {
                let roster = super::load_roster(tx).context(RosterSnafu)?;
                Self::photo_in(tx, &id, &roster)
            })
            .await
            .map_err(TransactionError::into_error)?;
        let ready = self.prepare_image(record, admission.clone()).await?;
        Ok((ready, admission))
    }
    pub(crate) async fn export(&self) -> Result<(PreparedDownload, Admission)> {
        let permit = self.reserve_export()?;
        let admission = self.lifecycle.admit().context(StoppingSnafu)?;
        let snapshot = self
            .database
            .read(move |tx| {
                let roster = super::load_roster(tx).context(RosterSnafu)?;
                Self::export_snapshot_in(tx, &roster)
            })
            .await
            .map_err(TransactionError::into_error)?;
        let ready = self
            .prepare_export(snapshot, permit, admission.clone())
            .await?;
        Ok((ready, admission))
    }

    pub(crate) async fn export_pyrite(&self) -> Result<(PreparedDownload, Admission)> {
        let permit = self.reserve_export()?;
        let admission = self.lifecycle.admit().context(StoppingSnafu)?;
        let (teams, entries) = self
            .database
            .read(move |tx| {
                let teams = ContestComponent::photo_roster_in(tx).context(RosterSnafu)?;
                let roster = PhotoRoster::from(teams.as_slice());
                let entries = Self::export_snapshot_in(tx, &roster)?;
                Ok::<_, DeliveryError>((teams, entries))
            })
            .await
            .map_err(TransactionError::into_error)?;
        let logos = tokio::select! {
            () = admission.stopping() => return Err(DeliveryError::Stopping),
            logos = self.contest.photo_logos() => logos.context(LogoSnafu)?,
        };
        let ready = self
            .prepare_pyrite_export(teams, entries, logos, permit, admission.clone())
            .await?;
        Ok((ready, admission))
    }
}
