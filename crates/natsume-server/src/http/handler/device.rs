use crate::http::Routes;

use axum::{
    response::{IntoResponse, Response},
    routing::{delete, get, patch},
};
use serde::Deserialize;

use crate::component::device::DeviceId;

use super::super::{error::ApiError, middleware};

pub(crate) mod binding;
pub(crate) mod convergence;
pub(crate) mod home;
pub(crate) mod lifecycle;
pub(crate) mod session;

use binding::delete_device_binding;
use convergence::get_device_convergence;
use home::get_home;
use lifecycle::{get_device, list_devices, update_device};
use session::get_session_control;

pub(in crate::http) fn operator_routes() -> Routes {
    vec![
        ("/devices", get(list_devices)),
        (
            "/devices/{device_id}",
            get(get_device).merge(middleware::require_admin(patch(update_device))),
        ),
        (
            "/devices/{device_id}/binding",
            middleware::require_admin(delete(delete_device_binding)),
        ),
        (
            "/devices/{device_id}/session-control",
            get(get_session_control),
        ),
        ("/devices/{device_id}/home", get(get_home)),
        (
            "/devices/{device_id}/convergence",
            get(get_device_convergence),
        ),
    ]
}

#[derive(Deserialize)]
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
