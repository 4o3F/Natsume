//! Coordinates Contest facts with Photo snapshot publication.
use crate::component::{
    contest::{ContestComponent, ContestError, PhotoLogoError},
    photo::{InitializationError, PhotoComponent, Preparation, SnapshotDownload},
};
use snafu::{ResultExt as _, Snafu};
use std::collections::BTreeMap;

#[derive(Debug, Snafu)]
pub(crate) enum Error {
    #[snafu(display("Photo initialization failed"))]
    Photo { source: InitializationError },
    #[snafu(display("Photo roster could not be read"))]
    Roster { source: ContestError },
    #[snafu(display("Photo logo could not be prepared"))]
    Logo { source: PhotoLogoError },
}
pub(crate) struct PhotoInitialization<'a> {
    pub photo: &'a PhotoComponent,
    pub contest: &'a ContestComponent,
}
impl PhotoInitialization<'_> {
    pub(crate) async fn run(&self, token: &str) -> Result<SnapshotDownload, Error> {
        let mut draft = match self
            .photo
            .begin_initialization(token)
            .await
            .context(PhotoSnafu)?
        {
            Preparation::Existing(snapshot) => return Ok(snapshot),
            Preparation::Build(draft) => draft,
        };
        let teams = self.contest.photo_roster().await.context(RosterSnafu)?;
        natsume_photo_protocol::validate_teams(&teams).map_err(|_| Error::Photo {
            source: if teams.is_empty() {
                InitializationError::EmptyRoster
            } else {
                InitializationError::InvalidRequest
            },
        })?;
        let schools: BTreeMap<_, _> = teams
            .iter()
            .map(|team| {
                (
                    team.organization_id,
                    (team.school_name_zh.clone(), team.school_name_en.clone()),
                )
            })
            .collect();
        draft.manifest.teams = teams;
        for (organization_id, (zh, en)) in schools {
            if let Some(bytes) = self.contest.photo_logo(&zh, &en).await.context(LogoSnafu)? {
                draft
                    .add_logo(organization_id, bytes)
                    .await
                    .context(PhotoSnafu)?;
            }
        }
        self.photo
            .publish_initialization(*draft)
            .await
            .context(PhotoSnafu)
    }
}
