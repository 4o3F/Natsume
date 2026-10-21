CREATE TABLE print_sources (
    source_id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    token_hash BLOB NOT NULL,
    revoked INTEGER NOT NULL
) STRICT;

-- Account identity and display facts intentionally survive roster removal.
CREATE TABLE print_requests (
    source_id TEXT NOT NULL REFERENCES print_sources(source_id),
    request_id TEXT NOT NULL,
    account_id TEXT NOT NULL,
    account TEXT NOT NULL,
    team_name TEXT NOT NULL,
    seat_label TEXT,
    original_filename TEXT NOT NULL,
    source_sha256 TEXT NOT NULL,
    generation_failure TEXT,
    PRIMARY KEY (source_id, request_id)
) STRICT;

-- AUTOINCREMENT reserves committed sequence numbers even after explicit maintenance.
CREATE TABLE print_documents (
    ready_sequence INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
    source_id TEXT NOT NULL,
    request_id TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    bytes INTEGER NOT NULL,
    pages INTEGER NOT NULL,
    ahead_tasks INTEGER NOT NULL,
    ahead_pages INTEGER NOT NULL,
    UNIQUE (source_id, request_id),
    FOREIGN KEY (source_id, request_id) REFERENCES print_requests(source_id, request_id)
) STRICT;
