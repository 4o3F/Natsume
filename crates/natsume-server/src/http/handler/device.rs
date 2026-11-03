use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use crate::component::device::DeviceId;

use super::super::error::ApiError;

pub(crate) mod binding;
pub(crate) mod convergence;
pub(crate) mod home;
pub(crate) mod lifecycle;
pub(crate) mod session;

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
