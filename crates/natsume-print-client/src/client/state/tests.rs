use std::fmt::Debug;

use super::*;

fn must<T, E: Debug>(result: std::result::Result<T, E>) -> T {
    result.unwrap_or_else(|error| panic!("state fixture failed: {error:?}"))
}

#[test]
fn a_failed_write_does_not_advance_the_in_memory_or_saved_phase() {
    let directory = must(tempfile::tempdir());
    let config = Config {
        server_url: "https://example.test".to_owned(),
        queue_name: "Queue".to_owned(),
        state_directory: directory.path().join("state"),
        ca_certificate: None,
    };
    let mut store = must(Store::open(&config));
    let sequence = store.sequence;
    store.journal = must(File::open(config.state_directory.join("state.jsonl")));
    assert!(store.pause(false).is_err());
    assert_eq!(store.sequence, sequence);
    assert!(store.state.paused);
    drop(store);
    assert!(must(Store::open(&config)).state.paused);
}
