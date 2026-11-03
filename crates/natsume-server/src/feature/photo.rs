use std::{path::PathBuf, sync::Arc};

use crate::{
    component::{
        contest::ContestComponent,
        photo::{InitializationError, PhotoComponent},
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
        contest: Arc<ContestComponent>,
    ) -> Result<Self, InitializationError> {
        let photo = tokio::task::spawn_blocking(move || PhotoComponent::new(database, &directory))
            .await
            .map_err(|source| InitializationError::Worker { source })??;
        Ok(Self {
            initialization: PreparePhotoInitialization::new(Arc::new(photo), contest),
        })
    }

    #[cfg(test)]
    pub(crate) fn for_test(
        database: Database,
        directory: &std::path::Path,
        contest: Arc<ContestComponent>,
    ) -> Result<Self, InitializationError> {
        Ok(Self {
            initialization: PreparePhotoInitialization::new(
                Arc::new(PhotoComponent::new(database, directory)?),
                contest,
            ),
        })
    }

    #[cfg(test)]
    pub(crate) const fn component(&self) -> &Arc<PhotoComponent> {
        self.initialization.photo()
    }

    pub(crate) fn routes(&self) -> AccessRoutes {
        AccessRoutes {
            public: bind_routes(photo::enrollment_routes(), &self.initialization),
            operator: bind_routes(photo::operator_routes(), &self.initialization),
            ..AccessRoutes::new()
        }
    }
}
