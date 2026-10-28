CREATE TABLE photo_server_metadata (
    singleton INTEGER NOT NULL PRIMARY KEY,
    dataset_id TEXT NOT NULL UNIQUE
) STRICT;

CREATE TABLE photo_sources (
    source_id TEXT NOT NULL PRIMARY KEY,
    name TEXT NOT NULL,
    state TEXT NOT NULL,
    enrollment_id TEXT UNIQUE,
    public_key TEXT,
    confirmed INTEGER NOT NULL,
    pairing_code_hash BLOB UNIQUE,
    pairing_expires_at_unix_ms INTEGER,
    token_hash BLOB,
    token_expires_at_unix_ms INTEGER,
    created_at_unix_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE photo_initializations (
    initialization_id TEXT NOT NULL PRIMARY KEY,
    source_id TEXT NOT NULL UNIQUE REFERENCES photo_sources(source_id),
    manifest_json TEXT NOT NULL,
    manifest_sha256 TEXT NOT NULL,
    completion_request_id TEXT,
    completed_at_unix_ms INTEGER
) STRICT;
