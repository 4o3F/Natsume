CREATE TABLE photo_workspace (
    singleton INTEGER NOT NULL PRIMARY KEY,
    format_version INTEGER NOT NULL,
    ready INTEGER NOT NULL
);

INSERT INTO photo_workspace (singleton, format_version, ready) VALUES (1, 2, 0);

CREATE TABLE photo_roster (
    account_id TEXT NOT NULL PRIMARY KEY,
    account TEXT NOT NULL UNIQUE,
    organization_id BIGINT NOT NULL,
    school_name_zh TEXT NOT NULL,
    school_name_en TEXT NOT NULL,
    team_name_zh TEXT NOT NULL,
    team_name_en TEXT NOT NULL
);
