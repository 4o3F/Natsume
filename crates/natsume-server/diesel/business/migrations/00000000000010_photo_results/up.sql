CREATE TABLE photo_import_jobs (
    import_id TEXT NOT NULL PRIMARY KEY,
    state TEXT NOT NULL CHECK (state IN ('uploading', 'uploaded', 'validating', 'ready', 'committed', 'failed', 'interrupted')),
    uploaded_bytes INTEGER NOT NULL DEFAULT 0 CHECK (uploaded_bytes BETWEEN 0 AND 8589934592),
    verified_images INTEGER NOT NULL DEFAULT 0 CHECK (verified_images BETWEEN 0 AND 2000),
    source_id TEXT REFERENCES photo_sources(source_id),
    initialization_id TEXT REFERENCES photo_initializations(initialization_id),
    manifest_id TEXT,
    manifest_sha256 TEXT,
    manifest_json TEXT CHECK (length(CAST(manifest_json AS BLOB)) <= 4194304),
    failure_code TEXT,
    created_at_unix_ms INTEGER NOT NULL,
    updated_at_unix_ms INTEGER NOT NULL,
    CHECK ((manifest_id IS NULL) = (manifest_sha256 IS NULL)),
    CHECK (state NOT IN ('ready', 'committed') OR
        (source_id IS NOT NULL AND initialization_id IS NOT NULL AND manifest_id IS NOT NULL AND manifest_json IS NOT NULL))
) STRICT;

CREATE INDEX photo_import_jobs_by_manifest ON photo_import_jobs(manifest_id);

CREATE TABLE photo_objects (
    image_sha256 TEXT NOT NULL PRIMARY KEY CHECK (length(image_sha256) = 64 AND image_sha256 NOT GLOB '*[^0-9a-f]*'),
    image_format TEXT NOT NULL CHECK (image_format IN ('jpeg', 'png')),
    image_bytes INTEGER NOT NULL CHECK (image_bytes BETWEEN 1 AND 52428800),
    width INTEGER NOT NULL CHECK (width BETWEEN 1 AND 80000000),
    height INTEGER NOT NULL CHECK (height BETWEEN 1 AND 80000000),
    CHECK (width * height <= 80000000)
) STRICT;

CREATE TABLE photo_submissions (
    source_id TEXT NOT NULL REFERENCES photo_sources(source_id),
    submission_id TEXT NOT NULL,
    result_id TEXT NOT NULL,
    import_id TEXT NOT NULL REFERENCES photo_import_jobs(import_id),
    account_id TEXT NOT NULL CHECK (length(account_id) > 0),
    account TEXT NOT NULL CHECK (length(account) > 0),
    image_sha256 TEXT NOT NULL REFERENCES photo_objects(image_sha256),
    state TEXT NOT NULL CHECK (state IN ('active', 'pending_review', 'needs_association', 'rejected', 'superseded')),
    revision INTEGER NOT NULL DEFAULT 1 CHECK (revision > 0),
    PRIMARY KEY (source_id, submission_id),
    UNIQUE (source_id, result_id)
) STRICT;

CREATE INDEX photo_submissions_by_account ON photo_submissions(account_id, state);
CREATE INDEX photo_submissions_by_import ON photo_submissions(import_id);
CREATE INDEX photo_submissions_by_state ON photo_submissions(state, source_id, submission_id);

CREATE TABLE team_photos (
    account_id TEXT NOT NULL PRIMARY KEY CHECK (length(account_id) > 0),
    account TEXT NOT NULL CHECK (length(account) > 0),
    source_id TEXT,
    submission_id TEXT,
    revision INTEGER NOT NULL CHECK (revision > 0),
    CHECK ((source_id IS NULL) = (submission_id IS NULL)),
    FOREIGN KEY (source_id, submission_id) REFERENCES photo_submissions(source_id, submission_id),
    UNIQUE (source_id, submission_id)
) STRICT;
