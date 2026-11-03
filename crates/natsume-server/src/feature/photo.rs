use std::{path::PathBuf, sync::Arc};

use crate::{
    component::{
        contest::ContestComponent,
        photo::{PhotoComponent, PhotoStorageError},
    },
    db::Database,
    http::{AccessRoutes, bind_routes, handler::photo},
    photo_initialization::PreparePhotoInitialization,
};

pub(crate) struct PhotoFeature {
    /// Shared Photo instance and Contest facts used by enrollment and initialization routes.
    initialization: PreparePhotoInitialization,
    acceptance: crate::photo_acceptance::AcceptPhotoResults,
    delivery: crate::photo_delivery::DeliverPhotoResults,
}

impl PhotoFeature {
    pub(crate) async fn load(
        database: Database,
        directory: PathBuf,
        results_directory: PathBuf,
        contest: Arc<ContestComponent>,
        lifecycle: crate::lifecycle::ServerLifecycle,
    ) -> Result<Self, PhotoStorageError> {
        let acceptance_database = database.clone();
        let photo = tokio::task::spawn_blocking(move || {
            PhotoComponent::load(database, &directory, &results_directory)
        })
        .await
        .map_err(|source| PhotoStorageError::Worker { source })??;
        let photo = Arc::new(photo);
        Ok(Self {
            delivery: crate::photo_delivery::DeliverPhotoResults::new(
                acceptance_database.clone(),
                photo.clone(),
                lifecycle,
            ),
            acceptance: crate::photo_acceptance::AcceptPhotoResults::new(acceptance_database),
            initialization: PreparePhotoInitialization::new(photo, contest),
        })
    }

    #[cfg(test)]
    pub(crate) fn for_test(
        database: Database,
        directory: &std::path::Path,
        results_directory: &std::path::Path,
        contest: Arc<ContestComponent>,
        lifecycle: crate::lifecycle::ServerLifecycle,
    ) -> Result<Self, PhotoStorageError> {
        let photo = Arc::new(PhotoComponent::load(
            database.clone(),
            directory,
            results_directory,
        )?);
        Ok(Self {
            delivery: crate::photo_delivery::DeliverPhotoResults::new(
                database.clone(),
                photo.clone(),
                lifecycle,
            ),
            acceptance: crate::photo_acceptance::AcceptPhotoResults::new(database),
            initialization: PreparePhotoInitialization::new(photo, contest),
        })
    }

    pub(crate) const fn component(&self) -> &Arc<PhotoComponent> {
        self.initialization.photo()
    }

    pub(crate) fn routes(&self) -> AccessRoutes {
        let mut operator = bind_routes(photo::operator_routes(), &self.initialization);
        operator.extend(bind_routes(
            photo::imports::routes(),
            self.initialization.photo(),
        ));
        operator.extend(bind_routes(photo::acceptance::routes(), &self.acceptance));
        operator.extend(bind_routes(photo::delivery::routes(), &self.delivery));
        AccessRoutes {
            public: bind_routes(photo::enrollment_routes(), &self.initialization),
            operator,
            ..AccessRoutes::new()
        }
    }
}
