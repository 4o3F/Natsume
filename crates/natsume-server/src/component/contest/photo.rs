//! Non-secret Photo projection. Contest retains roster and logo ownership.
use super::{ContestComponent, ContestError, db, logo_io};
use crate::db::TransactionError;
pub(crate) use natsume_photo_protocol::PhotoTeam;
#[derive(Debug, snafu::Snafu)]
pub(crate) enum PhotoLogoError {
    #[snafu(display("Photo image worker is unavailable"))]
    Unavailable { source: tokio::sync::AcquireError },
    #[snafu(display("Photo image worker failed"))]
    Worker { source: tokio::task::JoinError },
    #[snafu(display("Photo logo directory failed"))]
    Directory { source: std::io::Error },
    #[snafu(display("Photo logo decoding failed"))]
    Image { source: super::logo_io::LogoError },
}

impl ContestComponent {
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
            let directory = match logo_io::LogoDirectory::open(&directory) {
                Ok(directory) => Some(directory),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(source) => return Err(PhotoLogoError::Directory { source }),
            };
            let matched = directory
                .as_ref()
                .map_or(logo_io::LogoMatch::Missing, |directory| {
                    directory.resolve(&names.0, &names.1)
                });
            match matched {
                logo_io::LogoMatch::Missing | logo_io::LogoMatch::Ambiguous(_) => Ok(None),
                logo_io::LogoMatch::Unique(path) => logo_io::read_logo(&path)
                    .and_then(logo_io::DecodedLogo::encode_photo_png)
                    .map(Some)
                    .map_err(|source| PhotoLogoError::Image { source }),
            }
        })
        .await
        .map_err(|source| PhotoLogoError::Worker { source })?
    }
}
