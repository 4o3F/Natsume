PRAGMA foreign_keys = ON;

-- Owning Rust components validate and canonicalize all persisted values before
-- issuing SQL. SQLite owns only physical shape, nullability, relationships,
-- uniqueness, and indexes; business validation does not live in this migration.

CREATE TABLE site_identity (
    singleton INTEGER NOT NULL PRIMARY KEY,
    fleet_namespace_uuid TEXT NOT NULL UNIQUE
) STRICT;

CREATE TABLE audit_events (
    audit_event_id TEXT PRIMARY KEY,
    occurred_at_unix_ms INTEGER NOT NULL,
    actor TEXT NOT NULL,
    action_kind TEXT NOT NULL,
    resource_type TEXT NOT NULL,
    resource_id TEXT,
    result TEXT NOT NULL,
    reason_code TEXT,
    correlation_id TEXT NOT NULL,
    group_correlation_id TEXT,
    redacted_detail_json TEXT NOT NULL
) STRICT;

CREATE TABLE operator_accounts (
    operator_id TEXT PRIMARY KEY,
    username TEXT NOT NULL UNIQUE,
    role TEXT NOT NULL,
    password_hash TEXT NOT NULL
) STRICT;

CREATE TABLE operator_sessions (
    session_credential_hash BLOB PRIMARY KEY,
    operator_id TEXT NOT NULL REFERENCES operator_accounts(operator_id) ON DELETE CASCADE,
    expires_at_unix_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE seats (
    seat_id TEXT PRIMARY KEY,
    seat_code TEXT NOT NULL UNIQUE
) STRICT;

CREATE TABLE accounts (
    account_id TEXT PRIMARY KEY,
    domjudge_username TEXT NOT NULL UNIQUE,
    credential_revision INTEGER NOT NULL
) STRICT;

CREATE TABLE server_vault_records (
    account_id TEXT PRIMARY KEY REFERENCES accounts(account_id) ON DELETE CASCADE,
    nonce BLOB NOT NULL,
    ciphertext BLOB NOT NULL
) STRICT;

CREATE TABLE account_mappings (
    seat_id TEXT PRIMARY KEY REFERENCES seats(seat_id) ON DELETE CASCADE,
    account_id TEXT NOT NULL UNIQUE REFERENCES accounts(account_id) ON DELETE CASCADE
) STRICT;

CREATE TABLE pending_import_candidate (
    singleton INTEGER NOT NULL PRIMARY KEY,
    candidate_id TEXT NOT NULL UNIQUE,
    expires_at_unix_ms INTEGER NOT NULL,
    preview_token_hash BLOB NOT NULL UNIQUE,
    fingerprint_version INTEGER NOT NULL,
    candidate_fingerprint_sha256 BLOB NOT NULL,
    baseline_fingerprint_sha256 BLOB NOT NULL,
    redacted_preview_json TEXT NOT NULL,
    created_audit_event_id TEXT NOT NULL UNIQUE REFERENCES audit_events(audit_event_id)
) STRICT;

CREATE TABLE devices (
    device_id TEXT PRIMARY KEY,
    machine_hardware_id TEXT NOT NULL,
    evidence_quality TEXT NOT NULL,
    state TEXT NOT NULL,
    created_at_unix_ms INTEGER NOT NULL
) STRICT;
CREATE UNIQUE INDEX one_non_revoked_device_per_machine
    ON devices(machine_hardware_id)
    WHERE state != 'revoked';

CREATE TABLE device_control_keys (
    public_key BLOB PRIMARY KEY,
    device_id TEXT NOT NULL REFERENCES devices(device_id),
    status TEXT NOT NULL,
    activated_at_unix_ms INTEGER NOT NULL,
    activated_audit_event_id TEXT NOT NULL UNIQUE REFERENCES audit_events(audit_event_id),
    retired_at_unix_ms INTEGER,
    retired_audit_event_id TEXT UNIQUE REFERENCES audit_events(audit_event_id)
) STRICT;
CREATE UNIQUE INDEX one_current_device_control_key
    ON device_control_keys(device_id)
    WHERE status = 'current';

CREATE TABLE gateway_credentials (
    device_id TEXT PRIMARY KEY REFERENCES devices(device_id) ON DELETE CASCADE,
    credential_id TEXT NOT NULL,
    gateway_csr_der BLOB,
    gateway_leaf_der BLOB,
    -- Leaf-nearest issuer certificates concatenated as self-delimiting DER.
    -- Empty BLOB is the exact representation of a direct-issue empty chain.
    issuer_chain_der BLOB
) STRICT;
CREATE UNIQUE INDEX gateway_credentials_by_credential_id
    ON gateway_credentials(credential_id);

CREATE TABLE binding_negotiations (
    device_id TEXT PRIMARY KEY REFERENCES devices(device_id) ON DELETE CASCADE,
    negotiation_id TEXT NOT NULL,
    submission_epoch INTEGER,
    seat_code TEXT,
    evaluation_error_code TEXT
) STRICT;
CREATE UNIQUE INDEX binding_negotiations_by_negotiation_id
    ON binding_negotiations(negotiation_id);

CREATE TABLE device_bindings (
    binding_id TEXT PRIMARY KEY,
    device_id TEXT NOT NULL REFERENCES devices(device_id),
    seat_id TEXT NOT NULL REFERENCES seats(seat_id)
) STRICT;
CREATE UNIQUE INDEX one_binding_per_device
    ON device_bindings(device_id);
CREATE UNIQUE INDEX one_binding_per_seat
    ON device_bindings(seat_id);

CREATE TABLE runtime_config (
    singleton INTEGER NOT NULL PRIMARY KEY,
    domjudge_origin TEXT NOT NULL
) STRICT;

CREATE TABLE device_session_targets (
    device_id TEXT PRIMARY KEY REFERENCES devices(device_id) ON DELETE CASCADE,
    lock_state TEXT NOT NULL,
    terminate_epoch INTEGER
) STRICT;

CREATE TABLE device_home_targets (
    device_id TEXT PRIMARY KEY REFERENCES devices(device_id) ON DELETE CASCADE,
    reset_epoch INTEGER
) STRICT;
