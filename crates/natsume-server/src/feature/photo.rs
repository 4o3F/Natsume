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
}

impl PhotoFeature {
    pub(crate) async fn load(
        database: Database,
        directory: PathBuf,
        results_directory: PathBuf,
        contest: Arc<ContestComponent>,
    ) -> Result<Self, PhotoStorageError> {
        let photo = tokio::task::spawn_blocking(move || {
            PhotoComponent::load(database, &directory, &results_directory)
        })
        .await
        .map_err(|source| PhotoStorageError::Worker { source })??;
        Ok(Self {
            initialization: PreparePhotoInitialization::new(Arc::new(photo), contest),
        })
    }

    #[cfg(test)]
    pub(crate) fn for_test(
        database: Database,
        directory: &std::path::Path,
        results_directory: &std::path::Path,
        contest: Arc<ContestComponent>,
    ) -> Result<Self, PhotoStorageError> {
        Ok(Self {
            initialization: PreparePhotoInitialization::new(
                Arc::new(PhotoComponent::load(
                    database,
                    directory,
                    results_directory,
                )?),
                contest,
            ),
        })
    }

    #[cfg(test)]
    pub(crate) const fn component(&self) -> &Arc<PhotoComponent> {
        self.initialization.photo()
    }

    pub(crate) fn routes(&self) -> AccessRoutes {
        let mut operator = bind_routes(photo::operator_routes(), &self.initialization);
        operator.extend(bind_routes(
            photo::imports::routes(),
            self.initialization.photo(),
        ));
        AccessRoutes {
            public: bind_routes(photo::enrollment_routes(), &self.initialization),
            operator,
            ..AccessRoutes::new()
        }
    }
}
