use axum::{
    Router,
    response::{IntoResponse, Response},
    routing::{delete, get, patch, post, put},
};
use serde::Deserialize;
use utoipa::IntoParams;

use crate::component::device::DeviceId;

use super::super::{AppState, error::ApiError, middleware};

pub(crate) mod binding;
pub(crate) mod convergence;
pub(crate) mod home;
pub(crate) mod lifecycle;
pub(crate) mod session;

use binding::delete_device_binding;
use convergence::get_device_convergence;
use home::{get_home, reset_home};
use lifecycle::{get_device, list_devices, update_device};
use session::{get_session_control, set_session_lock, terminate_session};

pub(in crate::http) fn routes(state: AppState) -> Router<AppState> {
    Router::new()
        .route(
            "/devices",
            middleware::require_operator(state.clone(), get(list_devices)),
        )
        .route(
            "/devices/{device_id}",
            middleware::require_operator(state.clone(), get(get_device)).merge(
                middleware::require_admin(state.clone(), patch(update_device)),
            ),
        )
        .route(
            "/devices/{device_id}/binding",
            middleware::require_admin(state.clone(), delete(delete_device_binding)),
        )
        .route(
            "/devices/{device_id}/session-control",
            middleware::require_operator(state.clone(), get(get_session_control)).merge(
                middleware::require_admin(state.clone(), put(set_session_lock)),
            ),
        )
        .route(
            "/devices/{device_id}/session-control/actions/terminate",
            middleware::require_admin(state.clone(), post(terminate_session)),
        )
        .route(
            "/devices/{device_id}/home",
            middleware::require_operator(state.clone(), get(get_home)),
        )
        .route(
            "/devices/{device_id}/home/actions/reset",
            middleware::require_admin(state.clone(), post(reset_home)),
        )
        .route(
            "/devices/{device_id}/convergence",
            middleware::require_operator(state, get(get_device_convergence)),
        )
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
pub(crate) struct DevicePath {
    /// Canonical lowercase hyphenated `UUIDv7` Device ID.
    device_id: String,
}

fn parse_device_id(path: &DevicePath) -> Option<DeviceId> {
    DeviceId::parse(&path.device_id)
}

fn invalid_device_id() -> Response {
    ApiError::invalid_request("device_id_not_canonical_uuid_v7").into_response()
}
