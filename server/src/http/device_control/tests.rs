use axum::http::{HeaderMap, header};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};

use super::{
    WSS_AUTH_FAILURES_PER_WINDOW,
    auth::{DeviceControlAuthFailureLimiter, parse_bearer_token},
    registry::{DeviceConnectionRegistry, DisplacedConnection, RegisteredConnection},
    session::is_known_stable_error_code,
};

#[test]
fn bearer_token_parser_accepts_only_the_canonical_32_byte_shape() {
    let token = [0x42_u8; 32];
    let encoded = URL_SAFE_NO_PAD.encode(token);
    let mut headers = HeaderMap::new();
    headers.insert(
        header::AUTHORIZATION,
        format!("Bearer {encoded}")
            .parse()
            .unwrap_or_else(|_| panic!("test bearer header must parse")),
    );
    assert_eq!(parse_bearer_token(&headers), Some(token));

    for malformed in [
        format!("bearer {encoded}"),
        format!("Bearer {encoded}="),
        "Bearer AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAB".to_owned(),
        "Bearer !AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_owned(),
    ] {
        headers.insert(
            header::AUTHORIZATION,
            malformed
                .parse()
                .unwrap_or_else(|_| panic!("test malformed header must parse")),
        );
        assert_eq!(parse_bearer_token(&headers), None);
    }
}

#[tokio::test]
async fn registry_replacement_and_epoch_checked_cleanup_preserve_the_new_slot() {
    let registry = DeviceConnectionRegistry::new();
    let RegisteredConnection {
        registration: old_registration,
        eviction: mut old_eviction,
        dispatch: _old_dispatch,
        displaced: old_displaced,
    } = registry.register("device-1".to_owned(), 1);
    assert_eq!(old_displaced, DisplacedConnection::None);
    let RegisteredConnection {
        registration: new_registration,
        eviction: _new_eviction,
        dispatch: new_dispatch,
        displaced: new_displaced,
    } = registry.register("device-1".to_owned(), 2);
    assert_eq!(new_displaced, DisplacedConnection::Evicted);
    assert!(*old_eviction.borrow_and_update());
    assert!(registry.notify_dispatch("device-1"));
    new_dispatch.notified().await;
    drop(old_registration);
    assert!(registry.evict("device-1"));
    assert!(!registry.evict("device-1"));
    drop(new_registration);
}

#[test]
fn failed_authentication_limiter_blocks_after_the_frozen_count() {
    let limiter = DeviceControlAuthFailureLimiter::new();
    let address = "192.0.2.10"
        .parse()
        .unwrap_or_else(|_| panic!("test address must parse"));
    for _ in 0..WSS_AUTH_FAILURES_PER_WINDOW {
        assert!(!limiter.is_limited(address));
        limiter.record_failure(address);
    }
    assert!(limiter.is_limited(address));
}

#[test]
fn command_status_error_code_accepts_only_the_shared_stable_registry() {
    assert!(is_known_stable_error_code(""));
    assert!(is_known_stable_error_code("HOME_OPERATION_FAILED"));
    assert!(!is_known_stable_error_code(
        "UNKNOWN_/var/lib/natsume/secret=hunter2"
    ));
}
