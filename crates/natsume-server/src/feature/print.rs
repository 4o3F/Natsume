use std::{path::PathBuf, sync::Arc};

use crate::{
    component::print::{PrintComponent, PrintError},
    db::Database,
    http::{AccessRoutes, bind_routes, handler::print},
};

pub(crate) struct PrintFeature {
    /// The single Print instance shared by routes, authentication and background tasks.
    print: Arc<PrintComponent>,
}

impl PrintFeature {
    pub(crate) async fn load(database: Database, documents: PathBuf) -> Result<Self, PrintError> {
        Ok(Self {
            print: Arc::new(PrintComponent::load(database, documents).await?),
        })
    }

    #[cfg(test)]
    pub(crate) fn for_test(database: Database, documents: PathBuf) -> Result<Self, PrintError> {
        Ok(Self {
            print: Arc::new(PrintComponent::for_test(database, documents)?),
        })
    }

    pub(crate) const fn component(&self) -> &Arc<PrintComponent> {
        &self.print
    }

    pub(crate) fn routes(&self) -> AccessRoutes {
        AccessRoutes {
            public: bind_routes(print::public_routes(Arc::clone(&self.print)), &self.print),
            operator: bind_routes(print::operator_routes(), &self.print),
            source: bind_routes(print::source_routes(), &self.print),
            client: bind_routes(print::client_routes(), &self.print),
        }
    }
}
