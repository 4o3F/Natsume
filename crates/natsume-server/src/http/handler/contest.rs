use std::sync::Arc;

use crate::{component::contest::ContestComponent, http::Routes};

use super::super::{AppState, middleware};

pub(crate) mod account;
pub(crate) mod binding;
pub(crate) mod organization;
pub(crate) mod seat;

pub(crate) fn operator_routes() -> Routes<Arc<ContestComponent>> {
    [
        seat::operator_routes(),
        account::operator_routes(),
        binding::operator_routes(),
        organization::operator_routes(),
    ]
    .into_iter()
    .flatten()
    .collect()
}

pub(crate) fn public_routes() -> Routes<Arc<ContestComponent>> {
    organization::public_routes()
}
