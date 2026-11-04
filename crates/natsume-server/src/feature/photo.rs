use crate::{
    component::{
        contest::ContestComponent,
        photo::{PhotoComponent, PhotoStorageError},
    },
    db::Database,
    http::{AccessRoutes, bind_routes, handler::photo},
    lifecycle::ServerLifecycle,
};
use std::{path::PathBuf, sync::Arc};

pub(crate) struct PhotoFeature {
    photo: Arc<PhotoComponent>,
}
impl PhotoFeature {
    pub(crate) async fn load(
        database: Database,
        directory: PathBuf,
        results_directory: PathBuf,
        contest: Arc<ContestComponent>,
        lifecycle: ServerLifecycle,
    ) -> Result<Self, PhotoStorageError> {
        let photo = tokio::task::spawn_blocking(move || {
            PhotoComponent::load(database, &directory, &results_directory, contest, lifecycle)
        })
        .await
        .map_err(|source| PhotoStorageError::Worker {
            source: source.into(),
        })??;
        Ok(Self {
            photo: Arc::new(photo),
        })
    }
    #[cfg(test)]
    pub(crate) fn for_test(
        database: Database,
        directory: &std::path::Path,
        results_directory: &std::path::Path,
        contest: Arc<ContestComponent>,
        lifecycle: ServerLifecycle,
    ) -> Result<Self, PhotoStorageError> {
        Ok(Self {
            photo: Arc::new(PhotoComponent::load(
                database,
                directory,
                results_directory,
                contest,
                lifecycle,
            )?),
        })
    }
    pub(crate) const fn component(&self) -> &Arc<PhotoComponent> {
        &self.photo
    }
    pub(crate) fn routes(&self) -> AccessRoutes {
        let mut operator = bind_routes(photo::operator_routes(), &self.photo);
        operator.extend(bind_routes(photo::imports::routes(), &self.photo));
        operator.extend(bind_routes(photo::acceptance::routes(), &self.photo));
        operator.extend(bind_routes(photo::delivery::routes(), &self.photo));
        AccessRoutes {
            public: bind_routes(photo::enrollment_routes(), &self.photo),
            operator,
            ..AccessRoutes::new()
        }
    }
}
