//! Non-secret Photo projection. Contest retains roster and logo ownership.
use serde::{Deserialize, Serialize};

use super::{ContestComponent, ContestError, ExportError, db, logo_io, logos};
use crate::db::TransactionError;

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(crate) struct PhotoTeam {
    pub account_id: String,
    pub account: String,
    #[specta(type = specta_typescript::Number)]
    pub organization_id: i64,
    pub school_name_zh: String,
    pub school_name_en: String,
    pub team_name_zh: String,
    pub team_name_en: String,
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
        team: &PhotoTeam,
    ) -> Result<Option<Vec<u8>>, ExportError> {
        let permit = self
            .image_work
            .clone()
            .acquire_owned()
            .await
            .map_err(|source| ExportError::WorkerUnavailable { source })?;
        let directory = self.logo_directory.clone();
        let names = (
            team.organization_id,
            team.school_name_zh.clone(),
            team.school_name_en.clone(),
        );
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let directory = logos::open_directory(&directory)?;
            let matched = directory
                .as_ref()
                .map_or(logo_io::LogoMatch::Missing, |directory| {
                    directory.resolve(&names.1, &names.2)
                });
            match matched {
                logo_io::LogoMatch::Missing | logo_io::LogoMatch::Ambiguous(_) => Ok(None),
                logo_io::LogoMatch::Unique(path) => logo_io::read_logo(&path)
                    .and_then(logo_io::DecodedLogo::encode_photo_png)
                    .map(Some)
                    .map_err(|source| ExportError::Logo {
                        organization: format!("INST-{:03}", names.0),
                        file: logos::filename(&path),
                        reason: source.to_string(),
                        source: Box::new(source),
                    }),
            }
        })
        .await
        .map_err(|source| ExportError::Worker {
            source: source.into(),
        })?
    }
}
