-- Existing devices, bindings, accounts and encrypted credentials are preserved.
-- A complete XLSX import supplies the previously unavailable team metadata.
CREATE TABLE organizations (
    organization_id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
    name_key TEXT NOT NULL UNIQUE,
    name_zh TEXT NOT NULL,
    name_en TEXT NOT NULL,
    country TEXT NOT NULL
) STRICT;

CREATE TABLE teams (
    account_id TEXT PRIMARY KEY REFERENCES accounts(account_id) ON DELETE CASCADE,
    organization_id INTEGER NOT NULL REFERENCES organizations(organization_id),
    name_zh TEXT NOT NULL,
    name_en TEXT NOT NULL,
    category TEXT NOT NULL
) STRICT;
CREATE INDEX teams_by_organization ON teams(organization_id);

-- Old previews do not describe the new roster contract and must be uploaded again.
DELETE FROM pending_import_candidate;
