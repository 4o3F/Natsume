use std::sync::Arc;

use time::UtcOffset;

use crate::{
    component::operator::OperatorComponent,
    device_control::LiveLeaseView,
    http::{AccessRoutes, handler::journal},
    journal::read::JournalReader,
    journal_query::JournalQuery,
};

pub(crate) struct JournalFeature {
    /// Read-only query state shared by the operator journal routes.
    query: JournalQuery,
}

impl JournalFeature {
    pub(crate) fn load(
        reader: JournalReader,
        operator: Arc<OperatorComponent>,
        live: LiveLeaseView,
        utc_offset: UtcOffset,
    ) -> Self {
        Self {
            query: JournalQuery::new(reader, operator, live, utc_offset),
        }
    }

    #[cfg(test)]
    pub(crate) const fn query(&self) -> &JournalQuery {
        &self.query
    }

    pub(crate) fn routes(&self) -> AccessRoutes {
        AccessRoutes {
            operator: journal::operator_routes()
                .into_iter()
                .map(|(path, methods)| (path, methods.with_state(self.query.clone())))
                .collect(),
            ..AccessRoutes::new()
        }
    }
}
