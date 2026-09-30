CREATE TABLE operator_invitations (
    invitation_id TEXT PRIMARY KEY NOT NULL,
    role TEXT NOT NULL,
    issuer_operator_id TEXT NOT NULL REFERENCES operator_accounts(operator_id) ON DELETE CASCADE,
    token_hash BLOB NOT NULL UNIQUE,
    created_at_unix_ms INTEGER NOT NULL,
    expires_at_unix_ms INTEGER NOT NULL
) STRICT;
CREATE INDEX operator_invitations_by_issuer
    ON operator_invitations(issuer_operator_id);

CREATE TABLE operator_password_resets (
    operator_id TEXT PRIMARY KEY NOT NULL REFERENCES operator_accounts(operator_id) ON DELETE CASCADE,
    reset_id TEXT NOT NULL UNIQUE,
    issuer_operator_id TEXT NOT NULL REFERENCES operator_accounts(operator_id) ON DELETE CASCADE,
    token_hash BLOB NOT NULL UNIQUE,
    created_at_unix_ms INTEGER NOT NULL,
    expires_at_unix_ms INTEGER NOT NULL
) STRICT;
CREATE INDEX operator_password_resets_by_issuer
    ON operator_password_resets(issuer_operator_id);
