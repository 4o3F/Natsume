ALTER TABLE print_jobs ADD COLUMN requeue_count INTEGER NOT NULL DEFAULT 0 CHECK (requeue_count >= 0);

-- Keep attempt numbers within each three-attempt round for existing clients.
CREATE TABLE print_attempts_new (
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
    requeue_count INTEGER NOT NULL DEFAULT 0 CHECK (requeue_count >= 0),
    CHECK ((outcome IS NULL) = (finished_at_unix_ms IS NULL)),
    CHECK (outcome IS NOT 'printed' OR job_id IS NOT NULL),
    UNIQUE (ready_sequence, requeue_count, attempt_number)
) STRICT;

INSERT INTO print_attempts_new (
    attempt_id, ready_sequence, printer_id, client_id, attempt_number, queue_name,
    claimed_at_unix_ms, prepare_expires_at_unix_ms, lease_expires_at_unix_ms,
    last_contact_at_unix_ms, last_observed_at_unix_ms, job_id, finished_at_unix_ms, outcome
)
SELECT attempt_id, ready_sequence, printer_id, client_id, attempt_number, queue_name,
       claimed_at_unix_ms, prepare_expires_at_unix_ms, lease_expires_at_unix_ms,
       last_contact_at_unix_ms, last_observed_at_unix_ms, job_id, finished_at_unix_ms, outcome
FROM print_attempts;

-- Rebuild the referencing table without disabling foreign-key enforcement.
CREATE TABLE print_claims_new (
    client_id TEXT NOT NULL,
    request_id TEXT NOT NULL,
    queue_name TEXT NOT NULL,
    attempt_id TEXT UNIQUE REFERENCES print_attempts_new(attempt_id),
    PRIMARY KEY (client_id, request_id)
) STRICT;
INSERT INTO print_claims_new SELECT client_id, request_id, queue_name, attempt_id FROM print_claims;

DROP TABLE print_claims;
DROP TABLE print_attempts;
ALTER TABLE print_attempts_new RENAME TO print_attempts;
ALTER TABLE print_claims_new RENAME TO print_claims;

CREATE UNIQUE INDEX print_attempts_active_job ON print_attempts(ready_sequence) WHERE outcome IS NULL;
CREATE UNIQUE INDEX print_attempts_active_printer ON print_attempts(printer_id) WHERE outcome IS NULL;
CREATE UNIQUE INDEX print_attempts_active_client ON print_attempts(client_id) WHERE outcome IS NULL;
CREATE INDEX print_attempts_expiry ON print_attempts(lease_expires_at_unix_ms) WHERE outcome IS NULL;
