//! Non-secret Photo projection. Contest retains roster and logo ownership.
use super::{ContestComponent, ContestError, db, logo_io};
use crate::db::TransactionError;
pub(crate) use natsume_photo_protocol::PhotoTeam;
#[derive(Debug, snafu::Snafu)]
pub(crate) enum PhotoLogoError {
    #[snafu(display("Photo image worker is unavailable"))]
    Unavailable { source: tokio::sync::AcquireError },
    #[snafu(display("Photo image worker failed"))]
    Worker { source: crate::task::TaskFailure },
    #[snafu(display("Photo logo directory failed"))]
    Directory { source: std::io::Error },
    #[snafu(display("Photo logo decoding failed"))]
    Image { source: super::logo_io::LogoError },
}

/// One Contest-owned directory index and image-processing slot for a batch.
pub(crate) struct PhotoLogos {
    directory: Option<logo_io::LogoDirectory>,
    _permit: tokio::sync::OwnedSemaphorePermit,
}

impl PhotoLogos {
    /// Validate bounded image content, preserving PNG bytes and converting other formats.
    pub(crate) fn png(&self, zh: &str, en: &str) -> Result<Option<Vec<u8>>, PhotoLogoError> {
        read_png(self.directory.as_ref(), zh, en)
    }
}

fn read_png(
    directory: Option<&logo_io::LogoDirectory>,
    zh: &str,
    en: &str,
) -> Result<Option<Vec<u8>>, PhotoLogoError> {
    match directory.map_or(logo_io::LogoMatch::Missing, |directory| {
        directory.resolve(zh, en)
    }) {
        logo_io::LogoMatch::Missing | logo_io::LogoMatch::Ambiguous(_) => Ok(None),
        logo_io::LogoMatch::Unique(path) => logo_io::read_logo(&path)
            .and_then(logo_io::DecodedLogo::encode_photo_png)
            .map(Some)
            .map_err(|source| PhotoLogoError::Image { source }),
    }
}

fn open_directory(
    path: &std::path::Path,
) -> Result<Option<logo_io::LogoDirectory>, PhotoLogoError> {
    match logo_io::LogoDirectory::open(path) {
        Ok(directory) => Ok(Some(directory)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(PhotoLogoError::Directory { source }),
    }
}

impl ContestComponent {
    pub(crate) async fn photo_logos(&self) -> Result<PhotoLogos, PhotoLogoError> {
        let permit = self
            .image_work
            .clone()
            .acquire_owned()
            .await
            .map_err(|source| PhotoLogoError::Unavailable { source })?;
        let directory = self.logo_directory.clone();
        tokio::task::spawn_blocking(move || {
            Ok(PhotoLogos {
                directory: open_directory(&directory)?,
                _permit: permit,
            })
        })
        .await
        .map_err(|source| PhotoLogoError::Worker {
            source: source.into(),
        })?
    }

    /// Read Photo facts inside the caller's transaction, without starting another one.
    pub(crate) fn photo_roster_in(
        tx: &mut crate::db::Transaction<'_>,
    ) -> Result<Vec<PhotoTeam>, ContestError> {
        db::photo_roster(tx).map_err(ContestError::from)
    }

    pub(crate) async fn photo_roster(&self) -> Result<Vec<PhotoTeam>, ContestError> {
        self.database
            .read(db::photo_roster)
            .await
            .map_err(TransactionError::into_error)
            .map_err(ContestError::from)
    }

    /// Owns image matching/conversion; callers receive only bounded PNG bytes.
    pub(crate) async fn photo_logo(
        &self,
        school_name_zh: &str,
        school_name_en: &str,
    ) -> Result<Option<Vec<u8>>, PhotoLogoError> {
        let permit = self
            .image_work
            .clone()
            .acquire_owned()
            .await
            .map_err(|source| PhotoLogoError::Unavailable { source })?;
        let directory = self.logo_directory.clone();
        let names = (school_name_zh.to_owned(), school_name_en.to_owned());
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let directory = open_directory(&directory)?;
            read_png(directory.as_ref(), &names.0, &names.1)
        })
        .await
        .map_err(|source| PhotoLogoError::Worker {
            source: source.into(),
        })?
    }
}
