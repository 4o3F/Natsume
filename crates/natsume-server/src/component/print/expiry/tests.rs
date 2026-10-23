use super::*;
use crate::db::DatabaseConfig;

fn must<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
    result.unwrap_or_else(|error| panic!("expiry fixture failed: {error:?}"))
}

async fn driver() -> (tempfile::TempDir, Driver) {
    let directory = must(tempfile::tempdir());
    let database = must(
        Database::connect_and_migrate(&DatabaseConfig::new(
            directory.path().join("expiry.sqlite3"),
            true,
        ))
        .await,
    );
    (directory, Driver::new(database, Arc::new(Inbox::default())))
}

#[test]
fn unavailable_inbox_is_a_typed_failure_for_both_publisher_and_consumer() {
    let inbox = Arc::new(Inbox::default());
    let worker = Arc::clone(&inbox);
    let result = std::thread::spawn(move || {
        let _held = must(worker.pending.lock());
        panic!("injected inbox failure");
    })
    .join();
    assert!(result.is_err());
    assert!(matches!(
        inbox.schedule(CheckRequest {
            attempt_id: "attempt".to_owned(),
            check_at: Instant::now()
        }),
        Err(ExpiryError::InboxUnavailable)
    ));
    assert!(matches!(inbox.take(), Err(ExpiryError::InboxUnavailable)));
}

#[tokio::test]
async fn later_duplicate_cannot_postpone_an_existing_reminder() {
    let (_directory, mut driver) = driver().await;
    let now = Instant::now();
    for seconds in [10, 20, 5, 30] {
        driver.install(CheckRequest {
            attempt_id: "attempt".to_owned(),
            check_at: now + Duration::from_secs(seconds),
        });
    }
    assert_eq!(driver.wheel.len(), 1);
    assert_eq!(driver.scheduled["attempt"].at, now + Duration::from_secs(5));
}

#[tokio::test]
async fn expired_keys_are_removed_before_reuse_and_pending_checks_keep_ownership() {
    let (_directory, mut driver) = driver().await;
    driver.install(CheckRequest {
        attempt_id: "old".to_owned(),
        check_at: Instant::now(),
    });
    let expired = poll_fn(|cx| driver.wheel.poll_expired(cx))
        .await
        .unwrap_or_else(|| panic!("missing reminder"));
    driver.enqueue(expired);
    assert!(!driver.scheduled.contains_key("old"));
    driver.install(CheckRequest {
        attempt_id: "new".to_owned(),
        check_at: Instant::now() + Duration::from_mins(1),
    });
    driver.install(CheckRequest {
        attempt_id: "old".to_owned(),
        check_at: Instant::now(),
    });
    assert!(!driver.scheduled.contains_key("old"));
    assert!(driver.pending.contains("old"));
    driver.pending.remove("old");
    driver.install(CheckRequest {
        attempt_id: "old".to_owned(),
        check_at: Instant::now() + Duration::from_mins(1),
    });
    assert_ne!(driver.scheduled["old"].key, driver.scheduled["new"].key);
    assert_eq!(driver.wheel.len(), 2);
}
