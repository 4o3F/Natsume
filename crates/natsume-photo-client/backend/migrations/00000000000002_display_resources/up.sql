ALTER TABLE photo_roster ADD COLUMN logo_path TEXT;
ALTER TABLE photo_roster ADD COLUMN logo_sha256 TEXT
    CHECK ((logo_path IS NULL) = (logo_sha256 IS NULL));
ALTER TABLE photo_workspace ADD COLUMN default_image_path TEXT;
ALTER TABLE photo_workspace ADD COLUMN default_image_sha256 TEXT
    CHECK ((default_image_path IS NULL) = (default_image_sha256 IS NULL));
