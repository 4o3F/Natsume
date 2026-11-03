//! Coordinates Contest identity facts and Photo mutations in one database snapshot.
use crate::{
    component::{
        contest::{ContestComponent, ContestError},
        photo::{
            AcceptanceError, ImportPreview, PendingPhotoPage, PendingPhotoQuery, PhotoComponent,
            PhotoDecision,
        },
    },
    db::{Database, TransactionError},
};
use snafu::{ResultExt as _, Snafu};
use uuid::Uuid;

#[derive(Debug, Snafu)]
pub(crate) enum Error {
    #[snafu(display("Photo roster could not be read"))]
    Roster { source: ContestError },
    #[snafu(display("Photo acceptance failed"))]
    Photo { source: AcceptanceError },
}
impl From<crate::db::PersistenceError> for Error {
    fn from(source: crate::db::PersistenceError) -> Self {
        Self::Photo {
            source: source.into(),
        }
    }
}

#[derive(Clone)]
pub(crate) struct AcceptPhotoResults {
    database: Database,
}
impl AcceptPhotoResults {
    pub(crate) async fn pending(
        &self,
        query: PendingPhotoQuery,
    ) -> Result<PendingPhotoPage, Error> {
        self.database
            .read(move |tx| {
                let roster = ContestComponent::photo_roster_in(tx)
                    .context(RosterSnafu)?
                    .into();
                PhotoComponent::pending_in(tx, query, &roster).context(PhotoSnafu)
            })
            .await
            .map_err(TransactionError::into_error)
    }

    pub(crate) const fn new(database: Database) -> Self {
        Self { database }
    }

    pub(crate) async fn preview(&self, id: Uuid) -> Result<ImportPreview, Error> {
        self.database
            .read(move |tx| {
                let roster = ContestComponent::photo_roster_in(tx)
                    .context(RosterSnafu)?
                    .into();
                PhotoComponent::preview_in(tx, id, &roster).context(PhotoSnafu)
            })
            .await
            .map_err(TransactionError::into_error)
    }
    pub(crate) async fn commit(&self, id: Uuid) -> Result<ImportPreview, Error> {
        self.database
            .write(move |tx| {
                let roster = ContestComponent::photo_roster_in(tx)
                    .context(RosterSnafu)?
                    .into();
                PhotoComponent::commit_in(tx, id, &roster).context(PhotoSnafu)
            })
            .await
            .map_err(TransactionError::into_error)
    }
    pub(crate) async fn decide(
        &self,
        id: Uuid,
        decision: PhotoDecision,
    ) -> Result<ImportPreview, Error> {
        self.database
            .write(move |tx| {
                let roster = ContestComponent::photo_roster_in(tx)
                    .context(RosterSnafu)?
                    .into();
                PhotoComponent::decide_in(tx, id, decision, &roster).context(PhotoSnafu)
            })
            .await
            .map_err(TransactionError::into_error)
    }
}
