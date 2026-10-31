CREATE TABLE photo_team_results (
    account_id TEXT NOT NULL PRIMARY KEY,
    confirmation_id TEXT NOT NULL UNIQUE,
    result_id TEXT NOT NULL UNIQUE,
    submission_id TEXT NOT NULL UNIQUE,
    image_path TEXT NOT NULL,
    image_sha256 TEXT NOT NULL,
    image_bytes BIGINT NOT NULL,
    image_format TEXT NOT NULL,
    width INTEGER NOT NULL,
    height INTEGER NOT NULL,
    revision BIGINT NOT NULL
);
