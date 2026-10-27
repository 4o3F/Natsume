-- The previous schema cannot represent manual rounds; refuse to discard their history.
CREATE TEMP TABLE print_requeue_downgrade_guard (value INTEGER NOT NULL CHECK (value = 0));
INSERT INTO print_requeue_downgrade_guard SELECT requeue_count FROM print_jobs;
INSERT INTO print_requeue_downgrade_guard SELECT requeue_count FROM print_attempts;
DROP TABLE print_requeue_downgrade_guard;

CREATE TABLE print_attempts_old (
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

INSERT INTO print_attempts_old
SELECT attempt_id, ready_sequence, printer_id, client_id, attempt_number, queue_name,
       claimed_at_unix_ms, prepare_expires_at_unix_ms, lease_expires_at_unix_ms,
       last_contact_at_unix_ms, last_observed_at_unix_ms, job_id, finished_at_unix_ms, outcome
FROM print_attempts;

CREATE TABLE print_claims_old (
    client_id TEXT NOT NULL,
    request_id TEXT NOT NULL,
    queue_name TEXT NOT NULL,
    attempt_id TEXT UNIQUE REFERENCES print_attempts_old(attempt_id),
    PRIMARY KEY (client_id, request_id)
) STRICT;
INSERT INTO print_claims_old SELECT client_id, request_id, queue_name, attempt_id FROM print_claims;

DROP TABLE print_claims;
DROP TABLE print_attempts;
ALTER TABLE print_attempts_old RENAME TO print_attempts;
ALTER TABLE print_claims_old RENAME TO print_claims;
ALTER TABLE print_jobs DROP COLUMN requeue_count;

CREATE UNIQUE INDEX print_attempts_active_job ON print_attempts(ready_sequence) WHERE outcome IS NULL;
CREATE UNIQUE INDEX print_attempts_active_printer ON print_attempts(printer_id) WHERE outcome IS NULL;
CREATE UNIQUE INDEX print_attempts_active_client ON print_attempts(client_id) WHERE outcome IS NULL;
CREATE INDEX print_attempts_expiry ON print_attempts(lease_expires_at_unix_ms) WHERE outcome IS NULL;
