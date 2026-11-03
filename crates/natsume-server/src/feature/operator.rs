use std::sync::Arc;

use crate::{
    component::operator::OperatorComponent,
    db::Database,
    http::{
        AccessRoutes, bind_routes,
        handler::{operator, session},
    },
    journal::Journal,
};

pub(crate) struct OperatorFeature {
    /// The single Operator instance shared by routes, authentication and journal queries.
    operator: Arc<OperatorComponent>,
}

impl OperatorFeature {
    pub(crate) fn load(database: Database, journal: Journal) -> Self {
        Self {
            operator: Arc::new(OperatorComponent::new(database, journal)),
        }
    }

    pub(crate) const fn component(&self) -> &Arc<OperatorComponent> {
        &self.operator
    }

    pub(crate) fn routes(&self) -> AccessRoutes {
        let mut public = operator::public_routes(Arc::clone(&self.operator));
        public.extend(session::public_routes());
        let mut operator = operator::operator_routes();
        operator.extend(session::operator_routes());
        AccessRoutes {
            public: bind_routes(public, &self.operator),
            operator: bind_routes(operator, &self.operator),
            ..AccessRoutes::new()
        }
    }
}
