use crate::http::Routes;

use super::super::{AppState, middleware};

pub(crate) mod account;
pub(crate) mod binding;
pub(crate) mod organization;
pub(crate) mod seat;

pub(in crate::http) fn operator_routes() -> Routes {
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

pub(in crate::http) fn public_routes() -> Routes {
    organization::public_routes()
}
