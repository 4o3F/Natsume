CREATE TABLE photo_import_jobs (
    import_id TEXT NOT NULL PRIMARY KEY,
    state TEXT NOT NULL,
    uploaded_bytes INTEGER NOT NULL DEFAULT 0,
    verified_images INTEGER NOT NULL DEFAULT 0,
    manifest_id TEXT,
    manifest_sha256 TEXT,
    manifest_json TEXT,
    failure_code TEXT,
    created_at_unix_ms INTEGER NOT NULL,
    updated_at_unix_ms INTEGER NOT NULL
) STRICT;

CREATE INDEX photo_import_jobs_by_manifest ON photo_import_jobs(manifest_id);

CREATE TABLE photo_submissions (
    source_id TEXT NOT NULL REFERENCES photo_sources(source_id),
    submission_id TEXT NOT NULL,
    result_id TEXT NOT NULL,
    import_id TEXT NOT NULL REFERENCES photo_import_jobs(import_id),
    account_id TEXT NOT NULL,
    account TEXT NOT NULL,
    assigned_account_id TEXT,
    image_sha256 TEXT NOT NULL,
    image_format TEXT NOT NULL,
    image_bytes INTEGER NOT NULL,
    state TEXT NOT NULL,
    revision INTEGER NOT NULL DEFAULT 1,
    PRIMARY KEY (source_id, submission_id),
    UNIQUE (source_id, result_id)
) STRICT;

CREATE UNIQUE INDEX photo_submissions_active_assignment
    ON photo_submissions(assigned_account_id) WHERE state = 'active';
