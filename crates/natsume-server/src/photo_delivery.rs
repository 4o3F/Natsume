//! Coordinates one Contest/Photo snapshot; file work starts after the transaction ends.
use crate::{
    component::{
        contest::{ContestComponent, ContestError},
        photo::{DeliveryError, PhotoComponent, PhotoRecord, PhotoRoster, PreparedDownload},
    },
    db::{Database, Transaction, TransactionError},
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
    contest: Arc<ContestComponent>,
    lifecycle: ServerLifecycle,
}
impl DeliverPhotoResults {
    pub(crate) const fn new(
        database: Database,
        photo: Arc<PhotoComponent>,
        contest: Arc<ContestComponent>,
        lifecycle: ServerLifecycle,
    ) -> Self {
        Self {
            database,
            photo,
            contest,
            lifecycle,
        }
    }
    pub(crate) async fn list(&self) -> Result<Vec<PhotoRecord>, Error> {
        self.database
            .read(move |tx| {
                let roster = load_roster(tx)?;
                PhotoComponent::photos_in(tx, &roster).context(PhotoSnafu)
            })
            .await
            .map_err(TransactionError::into_error)
    }
    pub(crate) async fn image(&self, id: String) -> Result<(PreparedDownload, Admission), Error> {
        let admission = self.lifecycle.admit().context(StoppingSnafu)?;
        let record = self
            .database
            .read(move |tx| {
                let roster = load_roster(tx)?;
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
                let roster = load_roster(tx)?;
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

    pub(crate) async fn export_pyrite(&self) -> Result<(PreparedDownload, Admission), Error> {
        let permit = self.photo.reserve_export().context(PhotoSnafu)?;
        let admission = self.lifecycle.admit().context(StoppingSnafu)?;
        let (teams, entries) = self
            .database
            .read(move |tx| {
                let teams = ContestComponent::photo_roster_in(tx).context(RosterSnafu)?;
                let roster = PhotoRoster::from(teams.as_slice());
                let entries =
                    PhotoComponent::export_snapshot_in(tx, &roster).context(PhotoSnafu)?;
                Ok::<_, Error>((teams, entries))
            })
            .await
            .map_err(TransactionError::into_error)?;
        let logos = tokio::select! {
            () = admission.stopping() => return Err(Error::Stopping),
            logos = self.contest.photo_logos() => logos.map_err(|source| DeliveryError::Logo { source }).context(PhotoSnafu)?,
        };
        let ready = self
            .photo
            .prepare_pyrite_export(teams, entries, logos, permit, admission.clone())
            .await
            .context(PhotoSnafu)?;
        Ok((ready, admission))
    }
}

fn load_roster(tx: &mut Transaction<'_>) -> Result<PhotoRoster, Error> {
    ContestComponent::photo_roster_in(tx)
        .context(RosterSnafu)
        .map(PhotoRoster::from)
}
