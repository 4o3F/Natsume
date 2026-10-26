CREATE TABLE photo_workspace (
    singleton INTEGER NOT NULL PRIMARY KEY CHECK (singleton = 1),
    format_version INTEGER NOT NULL,
    ready BOOLEAN NOT NULL CHECK (ready IN (0, 1))
);

INSERT INTO photo_workspace (singleton, format_version, ready) VALUES (1, 1, 0);

CREATE TABLE photo_roster (
    account_id TEXT NOT NULL PRIMARY KEY,
    account TEXT NOT NULL UNIQUE,
    organization_id BIGINT NOT NULL,
    school_name_zh TEXT NOT NULL,
    school_name_en TEXT NOT NULL,
    team_name_zh TEXT NOT NULL,
    team_name_en TEXT NOT NULL
);
