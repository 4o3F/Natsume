use std::{fs, os::unix::fs::PermissionsExt as _, path::PathBuf};

use tempfile::{TempDir, tempdir};

use super::*;

const COMMAND_ID: &str = "018f0e2e-8c1d-7c5e-8b12-3456789abcde";

fn test_directory() -> TempDir {
    match tempdir() {
        Ok(directory) => directory,
        Err(error) => panic!("journal test directory must be created: {error}"),
    }
}

fn journal(directory: &TempDir) -> Journal {
    let path = directory.path().join("journal");
    match Journal::open(path) {
        Ok(journal) => journal,
        Err(error) => panic!("journal fixture must open: {error}"),
    }
}

fn record(journal: &Journal, bytes: &[u8]) -> JournalOutcome {
    match journal.record(COMMAND_ID, bytes) {
        Ok(outcome) => outcome,
        Err(error) => panic!("journal record must succeed: {error}"),
    }
}

#[test]
fn first_delivery_is_recorded_owner_only() {
    let directory = test_directory();
    let journal = journal(&directory);
    let frame = b"exact protobuf frame";

    let directory_metadata = match fs::metadata(&journal.directory) {
        Ok(metadata) => metadata,
        Err(error) => panic!("journal directory metadata must be readable: {error}"),
    };
    assert_eq!(
        directory_metadata.permissions().mode() & 0o777,
        JOURNAL_DIRECTORY_MODE
    );

    assert_eq!(record(&journal, frame), JournalOutcome::Recorded);

    let path = journal.directory.join(format!("{COMMAND_ID}.frame"));
    let contents = match fs::read(&path) {
        Ok(contents) => contents,
        Err(error) => panic!("journal frame must be readable: {error}"),
    };
    assert_eq!(contents, frame);
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) => panic!("journal frame metadata must be readable: {error}"),
    };
    assert_eq!(metadata.permissions().mode() & 0o777, JOURNAL_FRAME_MODE);
}

#[test]
fn byte_identical_redelivery_is_already_recorded() {
    let directory = test_directory();
    let journal = journal(&directory);
    let frame = b"same received frame";

    assert_eq!(record(&journal, frame), JournalOutcome::Recorded);
    assert_eq!(record(&journal, frame), JournalOutcome::AlreadyRecorded);
    let entries = match fs::read_dir(&journal.directory) {
        Ok(entries) => entries.count(),
        Err(error) => panic!("journal directory must be readable: {error}"),
    };
    assert_eq!(entries, 1);
}

#[test]
fn same_id_with_different_bytes_is_a_conflict() {
    let directory = test_directory();
    let journal = journal(&directory);

    assert_eq!(record(&journal, b"first frame"), JournalOutcome::Recorded);
    assert_eq!(
        record(&journal, b"different frame"),
        JournalOutcome::Conflict
    );
    let contents = match fs::read(journal.directory.join(format!("{COMMAND_ID}.frame"))) {
        Ok(contents) => contents,
        Err(error) => panic!("original journal frame must be readable: {error}"),
    };
    assert_eq!(contents, b"first frame");
}

#[test]
fn noncanonical_ids_are_rejected_before_filesystem_access() {
    let directory = test_directory();
    let inaccessible_parent = directory.path().join("must-not-be-created");
    let journal = Journal {
        directory: inaccessible_parent.join(PathBuf::from("journal")),
    };
    let uppercase = COMMAND_ID.to_uppercase();

    for command_id in [
        "../escape",
        uppercase.as_str(),
        "550e8400-e29b-41d4-a716-446655440000",
    ] {
        assert!(matches!(
            journal.record(command_id, b"remote bytes"),
            Err(JournalError::InvalidCommandId)
        ));
    }
    assert!(!inaccessible_parent.exists());
}
