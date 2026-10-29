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
    in_progress_tasks INTEGER NOT NULL DEFAULT 0,
    UNIQUE (source_id, request_id),
    FOREIGN KEY (source_id, request_id) REFERENCES print_requests(source_id, request_id)
) STRICT;

CREATE TABLE print_printers (
    printer_id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    pairing_code_hash BLOB UNIQUE,
    pairing_expires_at_unix_ms INTEGER,
    client_id TEXT UNIQUE,
    client_token_hash BLOB,
    client_confirmed INTEGER NOT NULL DEFAULT 0 CHECK (client_confirmed IN (0, 1)),
    CHECK ((pairing_code_hash IS NULL) = (pairing_expires_at_unix_ms IS NULL)),
    CHECK ((client_id IS NULL) = (client_token_hash IS NULL)),
    CHECK (client_id IS NOT NULL OR client_confirmed = 0),
    CHECK (client_id IS NULL OR pairing_code_hash IS NULL)
) STRICT;

-- Admission snapshots stay immutable; execution has its own durable lifecycle.
CREATE TABLE print_jobs (
    ready_sequence INTEGER PRIMARY KEY NOT NULL REFERENCES print_documents(ready_sequence),
    state TEXT NOT NULL CHECK (state IN ('queued', 'active', 'waiting', 'completed', 'exhausted')),
    attempts_used INTEGER NOT NULL CHECK (attempts_used BETWEEN 0 AND 3),
    available_at_unix_ms INTEGER NOT NULL,
    queue_order INTEGER NOT NULL UNIQUE,
    last_outcome TEXT CHECK (last_outcome IN ('printed', 'failed', 'unknown'))
) STRICT;

CREATE INDEX print_jobs_available ON print_jobs(state, available_at_unix_ms, queue_order);

CREATE TABLE print_attempts (
    attempt_id TEXT PRIMARY KEY NOT NULL,
    ready_sequence INTEGER NOT NULL REFERENCES print_jobs(ready_sequence),
    printer_id TEXT NOT NULL REFERENCES print_printers(printer_id),
    client_id TEXT NOT NULL,
    attempt_number INTEGER NOT NULL CHECK (attempt_number BETWEEN 1 AND 3),
    queue_name TEXT NOT NULL,
    claimed_at_unix_ms INTEGER NOT NULL,
    prepare_expires_at_unix_ms INTEGER NOT NULL,
    lease_expires_at_unix_ms INTEGER NOT NULL,
    last_contact_at_unix_ms INTEGER NOT NULL,
    last_observed_at_unix_ms INTEGER,
    job_id INTEGER CHECK (job_id BETWEEN 1 AND 4294967295),
    finished_at_unix_ms INTEGER,
    outcome TEXT CHECK (outcome IN ('printed', 'failed', 'unknown')),
    CHECK ((outcome IS NULL) = (finished_at_unix_ms IS NULL)),
    CHECK (outcome IS NOT 'printed' OR job_id IS NOT NULL),
    UNIQUE (ready_sequence, attempt_number)
) STRICT;

CREATE UNIQUE INDEX print_attempts_active_job ON print_attempts(ready_sequence) WHERE outcome IS NULL;
CREATE UNIQUE INDEX print_attempts_active_printer ON print_attempts(printer_id) WHERE outcome IS NULL;
CREATE UNIQUE INDEX print_attempts_active_client ON print_attempts(client_id) WHERE outcome IS NULL;
CREATE INDEX print_attempts_expiry ON print_attempts(lease_expires_at_unix_ms) WHERE outcome IS NULL;

-- Empty polls are receipts too: replay cannot become a different allocation.
CREATE TABLE print_claims (
    client_id TEXT NOT NULL,
    request_id TEXT NOT NULL,
    queue_name TEXT NOT NULL,
    attempt_id TEXT UNIQUE REFERENCES print_attempts(attempt_id),
    PRIMARY KEY (client_id, request_id)
) STRICT;
