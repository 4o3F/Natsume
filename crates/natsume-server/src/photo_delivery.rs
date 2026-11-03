//! Coordinates one Contest/Photo snapshot; file work starts after the transaction ends.
use crate::{
    component::{
        contest::{ContestComponent, ContestError},
        photo::{DeliveryError, PhotoComponent, PhotoPage, PhotoQuery, PreparedDownload},
    },
    db::{Database, TransactionError},
    lifecycle::{Admission, ServerLifecycle},
};
use snafu::{OptionExt as _, ResultExt as _, Snafu};
use std::sync::Arc;

#[derive(Debug, Snafu)]
pub(crate) enum Error {
    #[snafu(display("Server is stopping"))]
    Stopping,
    #[snafu(display("Photo roster could not be read"))]
    Roster { source: ContestError },
    #[snafu(display("Photo delivery failed"))]
    Photo { source: DeliveryError },
}
impl From<crate::db::PersistenceError> for Error {
    fn from(source: crate::db::PersistenceError) -> Self {
        Self::Photo {
            source: source.into(),
        }
    }
}
#[derive(Clone)]
pub(crate) struct DeliverPhotoResults {
    database: Database,
    photo: Arc<PhotoComponent>,
    lifecycle: ServerLifecycle,
}
impl DeliverPhotoResults {
    pub(crate) const fn new(
        database: Database,
        photo: Arc<PhotoComponent>,
        lifecycle: ServerLifecycle,
    ) -> Self {
        Self {
            database,
            photo,
            lifecycle,
        }
    }
    pub(crate) async fn list(&self, query: PhotoQuery) -> Result<PhotoPage, Error> {
        self.database
            .read(move |tx| {
                let roster = ContestComponent::photo_roster_in(tx)
                    .context(RosterSnafu)?
                    .into();
                PhotoComponent::photos_in(tx, &query, &roster).context(PhotoSnafu)
            })
            .await
            .map_err(TransactionError::into_error)
    }
    pub(crate) async fn image(&self, id: String) -> Result<(PreparedDownload, Admission), Error> {
        let admission = self.lifecycle.admit().context(StoppingSnafu)?;
        let record = self
            .database
            .read(move |tx| {
                let roster = ContestComponent::photo_roster_in(tx)
                    .context(RosterSnafu)?
                    .into();
                PhotoComponent::photo_in(tx, &id, &roster).context(PhotoSnafu)
            })
            .await
            .map_err(TransactionError::into_error)?;
        let ready = self
            .photo
            .prepare_image(record, admission.clone())
            .await
            .context(PhotoSnafu)?;
        Ok((ready, admission))
    }
    pub(crate) async fn export(&self) -> Result<(PreparedDownload, Admission), Error> {
        let permit = self.photo.reserve_export().context(PhotoSnafu)?;
        let admission = self.lifecycle.admit().context(StoppingSnafu)?;
        let snapshot = self
            .database
            .read(move |tx| {
                let roster = ContestComponent::photo_roster_in(tx)
                    .context(RosterSnafu)?
                    .into();
                PhotoComponent::export_snapshot_in(tx, &roster).context(PhotoSnafu)
            })
            .await
            .map_err(TransactionError::into_error)?;
        let ready = self
            .photo
            .prepare_export(snapshot, permit, admission.clone())
            .await
            .context(PhotoSnafu)?;
        Ok((ready, admission))
    }
}
