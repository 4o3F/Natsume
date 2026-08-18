use std::{
    collections::HashMap,
    net::IpAddr,
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant},
};

use axum::{
    Extension,
    extract::{Request, State},
    http::{HeaderMap, StatusCode, header},
    middleware::Next,
    response::{IntoResponse as _, Response},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use natsume_device_protocol::is_valid_device_token;
use sha2::{Digest as _, Sha256};
use subtle::ConstantTimeEq as _;
use zeroize::Zeroize as _;

use crate::{
    application::device::{DeviceId, credentials},
    audit::CorrelationId,
    tls::ClientAddress,
};

use super::{
    super::{AppState, error::ApiError},
    WSS_AUTH_FAILURE_WINDOW_SECONDS, WSS_AUTH_FAILURES_PER_WINDOW,
};

#[derive(Clone)]
pub(in crate::http) struct DeviceControlAuthFailureLimiter {
    failures: Arc<Mutex<HashMap<IpAddr, (Instant, u32)>>>,
}

impl DeviceControlAuthFailureLimiter {
    pub(in crate::http) fn new() -> Self {
        Self {
            failures: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub(super) fn is_limited(&self, address: IpAddr) -> bool {
        let now = Instant::now();
        let mut failures = self.lock_failures();
        prune_auth_failures(&mut failures, now);
        failures
            .get(&address)
            .is_some_and(|(_, count)| *count >= WSS_AUTH_FAILURES_PER_WINDOW)
    }

    pub(super) fn record_failure(&self, address: IpAddr) {
        let now = Instant::now();
        let mut failures = self.lock_failures();
        prune_auth_failures(&mut failures, now);
        failures
            .entry(address)
            .and_modify(|(_, count)| *count = count.saturating_add(1))
            .or_insert((now, 1));
    }

    fn lock_failures(&self) -> MutexGuard<'_, HashMap<IpAddr, (Instant, u32)>> {
        self.failures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

pub(super) fn prune_auth_failures(failures: &mut HashMap<IpAddr, (Instant, u32)>, now: Instant) {
    let window = Duration::from_secs(WSS_AUTH_FAILURE_WINDOW_SECONDS);
    failures.retain(|_, (window_start, _)| now.duration_since(*window_start) < window);
}

#[derive(Clone)]
pub(super) struct AuthenticatedDevice {
    pub(super) device_pk: DeviceId,
    pub(super) machine_hardware_id: String,
}

pub(super) async fn authenticate_device_control(
    State(state): State<AppState>,
    Extension(correlation_id): Extension<CorrelationId>,
    remote_address: ClientAddress,
    headers: HeaderMap,
    mut request: Request,
    next: Next,
) -> Response {
    let source_ip = remote_address.ip();
    if state.device_control_auth_failures.is_limited(source_ip) {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }

    let Some(mut token) = parse_bearer_token(&headers) else {
        state.device_control_auth_failures.record_failure(source_ip);
        return ApiError::authentication_failed(
            "device_control_authentication_failed",
            correlation_id,
        )
        .into_response();
    };
    let token_hash: [u8; 32] = Sha256::digest(token).into();
    token.zeroize();
    let lookup_hash = token_hash;
    let lookup = credentials::device_token_authentication_facts(&state.database, lookup_hash).await;
    let row = match lookup {
        Ok(Some(row)) => row,
        Ok(None) => {
            state.device_control_auth_failures.record_failure(source_ip);
            return ApiError::authentication_failed(
                "device_control_authentication_failed",
                correlation_id,
            )
            .into_response();
        }
        Err(error) => {
            return ApiError::from_device(error, correlation_id).into_response();
        }
    };
    if !bool::from(row.token_hash.ct_eq(&token_hash)) {
        state.device_control_auth_failures.record_failure(source_ip);
        return ApiError::authentication_failed(
            "device_control_authentication_failed",
            correlation_id,
        )
        .into_response();
    }
    request.extensions_mut().insert(AuthenticatedDevice {
        device_pk: row.device_pk,
        machine_hardware_id: row.machine_hardware_id.hyphenated().to_string(),
    });
    next.run(request).await
}

pub(super) fn parse_bearer_token(headers: &HeaderMap) -> Option<[u8; 32]> {
    let mut values = headers.get_all(header::AUTHORIZATION).iter();
    let value = values.next()?;
    if values.next().is_some() {
        return None;
    }
    let encoded = value.to_str().ok()?.strip_prefix("Bearer ")?;
    let bytes = encoded.as_bytes();
    if !is_valid_device_token(bytes) {
        return None;
    }
    let mut decoded = [0_u8; 32];
    let decoded_len = URL_SAFE_NO_PAD
        .decode_slice_unchecked(encoded, &mut decoded)
        .ok()?;
    (decoded_len == decoded.len()).then_some(decoded)
}
