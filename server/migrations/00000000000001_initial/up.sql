PRAGMA foreign_keys = ON;

-- One-row site identity. Machine Hardware IDs derive from fleet_namespace_uuid.
CREATE TABLE site_identity (
    -- CHECK(singleton = 1) forces exactly one row.
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    -- Public immutable fleet UUID; UUIDv5 namespace for Machine Hardware ID (ADR-0032).
    fleet_namespace_uuid TEXT NOT NULL UNIQUE
) STRICT;

-- Current contest Seat identities. Rename is REMOVED+ADDED, not an in-place code change.
CREATE TABLE seats (
    -- Canonical lowercase UUIDv7 surrogate. Stable while this Seat row exists.
    seat_id TEXT PRIMARY KEY,
    -- Venue-facing label (e.g. A-01). Unique in the current configuration; Import may replace the row.
    seat_code TEXT NOT NULL UNIQUE
) STRICT;

-- One workstation. Hardware ID is unique; this row is the durable Device identity.
CREATE TABLE devices (
    -- Canonical lowercase UUIDv7 surrogate. Not derived from hardware.
    device_id TEXT PRIMARY KEY,
    -- Derived Machine Hardware ID (ADR-0032). Unique; not an authenticator.
    machine_hardware_id TEXT NOT NULL UNIQUE,
    -- Quality of the derivation evidence: strong | medium | weak.
    hardware_identity_quality TEXT NOT NULL
        CHECK (hardware_identity_quality IN ('strong', 'medium', 'weak')),
    -- enrolled | revoked | disabled. WSS/control authority requires enrolled.
    state TEXT NOT NULL CHECK (state IN ('enrolled', 'revoked', 'disabled'))
) STRICT;

-- Append-only redacted evidence. Event-specific facts live in redacted_detail_json, not extra columns.
CREATE TABLE audit_events (
    -- Canonical lowercase UUIDv7. Fresh id may be an input to the guarded operation; stored ids cannot be replayed.
    audit_event_id TEXT PRIMARY KEY,
    -- UTC epoch milliseconds of the mutation.
    occurred_at_unix_ms INTEGER NOT NULL,
    -- Operator id or system actor (system:recovery, system:password-reset). Not a Device.
    actor TEXT NOT NULL,
    -- Closed action vocabulary (see contracts audit registry).
    action_kind TEXT NOT NULL,
    -- Affected resource class (device, import, command, ...).
    resource_type TEXT NOT NULL,
    -- Optional canonical id of that resource. No FK: parent table depends on resource_type.
    resource_id TEXT,
    -- succeeded | rejected | failed | noop.
    result TEXT NOT NULL CHECK (result IN ('succeeded', 'rejected', 'failed', 'noop')),
    -- Optional registered reason for rejected/noop. Empty/NULL when not applicable.
    reason_code TEXT,
    -- Per-request correlation UUIDv7. Always present.
    correlation_id TEXT NOT NULL,
    -- Optional bulk grouping UUIDv7 (batch Command). Not ordering or lifecycle.
    group_correlation_id TEXT,
    -- Typed redacted object: counts, binding_id, revisions. Never secrets or raw CSV.
    redacted_detail_json TEXT NOT NULL
        CHECK (json_valid(redacted_detail_json) AND json_type(redacted_detail_json) = 'object')
) STRICT;

-- Control-plane human operators. Not contest DOMjudge accounts.
CREATE TABLE operator_accounts (
    -- Canonical lowercase UUIDv7 surrogate.
    operator_id TEXT PRIMARY KEY,
    -- Login handle typed at the panel. Unique. Not a contest account username.
    username TEXT NOT NULL UNIQUE,
    -- Closed role: admin | viewer. Not a permission bitset.
    role TEXT NOT NULL CHECK (role IN ('admin', 'viewer')),
    -- Password hash with work factor. Plaintext never stored.
    password_hash TEXT NOT NULL
) STRICT;

-- Opaque operator sessions. Logout and password reset delete rows; no sliding renewal.
CREATE TABLE operator_sessions (
    -- SHA-256 of the cookie secret. The secret itself is never stored.
    session_credential_hash BLOB PRIMARY KEY CHECK (length(session_credential_hash) = 32),
    -- Owning operator. Password reset deletes every row for this id.
    operator_id TEXT NOT NULL REFERENCES operator_accounts(operator_id),
    -- Absolute UTC epoch-ms expiry. No last-activity column, no sliding TTL.
    expires_at_unix_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE accounts (
    -- Canonical lowercase UUIDv7. Parent of the password vault row.
    account_id TEXT PRIMARY KEY,
    -- DOMjudge login name in the current confirmed configuration.
    domjudge_username TEXT NOT NULL UNIQUE,
    -- Current secret generation for SYNC_SECRET fencing. Every successful Import Commit increments it.
    credential_revision INTEGER NOT NULL CHECK (credential_revision >= 1)
) STRICT;

-- Current-fact AEAD store for Account passwords. Insert account first, then this row.
CREATE TABLE server_vault_records (
    -- FK and PK: one ciphertext per Account. Cascades when the Account is deleted.
    account_id TEXT PRIMARY KEY REFERENCES accounts(account_id) ON DELETE CASCADE,
    -- AEAD nonce (XChaCha20-Poly1305: 24 bytes in the vault implementation).
    nonce BLOB NOT NULL CHECK (length(nonce) > 0),
    -- AEAD ciphertext of the current DOMjudge password; plaintext never stored.
    ciphertext BLOB NOT NULL CHECK (length(ciphertext) > 0)
) STRICT;

-- Current Seat→Account mapping. No row means the Seat currently has no Account.
CREATE TABLE account_mappings (
    -- One Account per Seat. Seat deletion must remove this row in the same Import transaction.
    seat_id TEXT PRIMARY KEY REFERENCES seats(seat_id),
    -- One Seat per Account.
    account_id TEXT NOT NULL UNIQUE REFERENCES accounts(account_id)
) STRICT;

-- Current Seat↔Device occupancy. Unbind deletes the row; a later bind mints a new binding_id.
CREATE TABLE device_bindings (
    -- One Device per Seat.
    seat_id TEXT PRIMARY KEY REFERENCES seats(seat_id),
    -- One Seat per Device.
    device_id TEXT NOT NULL UNIQUE REFERENCES devices(device_id),
    -- Canonical lowercase UUIDv7 occupancy stamp. Fresh on every bind; not derived from seats.
    -- Frozen into SYNC_SECRET / Target; old commands fail if this id changed or the row is gone.
    binding_id TEXT NOT NULL UNIQUE
) STRICT;

-- Latest Device-reported snapshot. One row per Device; not a history table.
-- Wire fields plus Server receive-time. Empty proto bytes/strings map to NULL here.
CREATE TABLE observed_device_states (
    -- Owning Device. One current snapshot.
    device_id TEXT PRIMARY KEY REFERENCES devices(device_id),
    -- SHA-256 of the last successfully applied SyncState assignment; same 32 bytes
    -- as SyncState.canonical_hash. NULL until first successful apply.
    applied_hash BLOB CHECK (applied_hash IS NULL OR length(applied_hash) = 32),
    -- Occupancy stamp last installed on Device; canonical lowercase hyphenated UUIDv7.
    -- NULL = never installed.
    installed_binding_id TEXT,
    -- accounts.credential_revision last written by SYNC_SECRET. NULL = never installed.
    installed_credential_revision INTEGER CHECK (
        installed_credential_revision IS NULL OR installed_credential_revision >= 0
    ),
    -- Local DOMjudge password file: absent | installed | failed.
    -- Freshness is installed_credential_revision vs accounts.credential_revision, not a stale variant.
    credential_state TEXT NOT NULL CHECK (credential_state IN ('absent', 'installed', 'failed')),
    -- Caddy data plane: absent | blocked | restoring | ready | upstream_unhealthy | recovery_required.
    gateway_state TEXT NOT NULL CHECK (gateway_state IN (
        'absent', 'blocked', 'restoring', 'ready', 'upstream_unhealthy', 'recovery_required'
    )),
    -- SHA-256 of the raw SPKI DER of the Gateway cert Caddy is actually using. NULL if none.
    gateway_certificate_fingerprint BLOB CHECK (
        gateway_certificate_fingerprint IS NULL OR length(gateway_certificate_fingerprint) = 32
    ),
    -- none | starting | active | locked | terminating | error.
    session_state TEXT NOT NULL CHECK (session_state IN (
        'none', 'starting', 'active', 'locked', 'terminating', 'error'
    )),
    -- Server receive-time UTC epoch-ms. Not a Device-authored field.
    observed_at_unix_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE provisioning_window (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    state TEXT NOT NULL CHECK (state IN ('closed', 'open')),
    revision INTEGER NOT NULL CHECK (revision >= 0),
    last_audit_event_id TEXT UNIQUE REFERENCES audit_events(audit_event_id),
    CHECK (
        (revision = 0 AND state = 'closed' AND last_audit_event_id IS NULL)
        OR (revision > 0 AND last_audit_event_id IS NOT NULL)
    )
) STRICT;

CREATE TABLE enrollment_requests (
    enrollment_request_id TEXT PRIMARY KEY,
    machine_hardware_id TEXT NOT NULL,
    hardware_identity_quality TEXT NOT NULL
        CHECK (hardware_identity_quality IN ('strong', 'medium', 'weak')),
    gateway_csr_der BLOB NOT NULL CHECK (length(gateway_csr_der) > 0),
    gateway_spki_sha256 BLOB NOT NULL CHECK (length(gateway_spki_sha256) = 32),
    client_version TEXT NOT NULL,
    protocol_version INTEGER NOT NULL CHECK (protocol_version BETWEEN 0 AND 4294967295),
    source_ip TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN (
        'pending', 'approved', 'rejected', 'issued', 'expired', 'conflict',
        'pending_approval', 'awaiting_credential_ack', 'active'
    )),
    resolution TEXT CHECK (resolution IN ('create_device', 'replace_device_credentials')),
    resolved_device_id TEXT REFERENCES devices(device_id),
    issuance_audit_event_id TEXT UNIQUE REFERENCES audit_events(audit_event_id),
    created_at_unix_ms INTEGER NOT NULL,
    control_intent TEXT CHECK (
        control_intent IS NULL
        OR control_intent IN ('first', 'replace', 'recover', 'refresh')
    ),
    proposed_device_id TEXT,
    proposed_control_key_id BLOB
        CHECK (proposed_control_key_id IS NULL OR length(proposed_control_key_id) = 32),
    proposed_control_public_key BLOB
        CHECK (proposed_control_public_key IS NULL OR length(proposed_control_public_key) = 32),
    control_key_generation INTEGER
        CHECK (control_key_generation IS NULL OR control_key_generation >= 1),
    canonical_client_init BLOB
        CHECK (canonical_client_init IS NULL OR length(canonical_client_init) > 0),
    request_fingerprint_version INTEGER
        CHECK (request_fingerprint_version IS NULL OR request_fingerprint_version >= 1),
    request_fingerprint_sha256 BLOB
        CHECK (request_fingerprint_sha256 IS NULL OR length(request_fingerprint_sha256) = 32),
    baseline_authority_revision INTEGER
        CHECK (baseline_authority_revision IS NULL OR baseline_authority_revision >= 1),
    expected_active_control_key_id BLOB CHECK (
        expected_active_control_key_id IS NULL
        OR length(expected_active_control_key_id) = 32
    ),
    activation_deadline_unix_ms INTEGER,
    approval_audit_event_id TEXT UNIQUE REFERENCES audit_events(audit_event_id),
    CHECK (
        state != 'issued'
        OR (
            resolution IS NOT NULL
            AND resolved_device_id IS NOT NULL
            AND issuance_audit_event_id IS NOT NULL
        )
    ),
    CHECK (issuance_audit_event_id IS NULL OR state = 'issued'),
    CHECK (
        (
            control_intent IS NULL
            AND state IN ('pending', 'approved', 'rejected', 'issued', 'expired', 'conflict')
        )
        OR (
            control_intent IS NOT NULL
            AND state IN (
                'pending_approval', 'awaiting_credential_ack', 'active',
                'rejected', 'expired', 'conflict'
            )
        )
    ),
    CHECK (
        (
            proposed_control_key_id IS NULL
            AND proposed_control_public_key IS NULL
            AND control_key_generation IS NULL
        )
        OR (
            proposed_control_key_id IS NOT NULL
            AND proposed_control_public_key IS NOT NULL
            AND control_key_generation IS NOT NULL
        )
    ),
    CHECK (
        (
            request_fingerprint_version IS NULL
            AND request_fingerprint_sha256 IS NULL
        )
        OR (
            request_fingerprint_version IS NOT NULL
            AND request_fingerprint_sha256 IS NOT NULL
        )
    )
) STRICT;
CREATE UNIQUE INDEX one_live_enrollment_per_machine_and_gateway_spki
    ON enrollment_requests(machine_hardware_id, gateway_spki_sha256)
    WHERE state IN ('pending', 'approved');
CREATE UNIQUE INDEX one_live_control_enrollment_per_machine
    ON enrollment_requests(machine_hardware_id)
    WHERE state IN ('pending_approval', 'awaiting_credential_ack')
        AND control_intent IS NOT NULL;
CREATE UNIQUE INDEX one_live_control_enrollment_per_resolved_device
    ON enrollment_requests(resolved_device_id)
    WHERE resolved_device_id IS NOT NULL
        AND state IN ('pending_approval', 'awaiting_credential_ack')
        AND control_intent IS NOT NULL;

CREATE TABLE device_control_keys (
    public_key BLOB PRIMARY KEY NOT NULL UNIQUE CHECK (length(public_key) = 32),
    algorithm TEXT NOT NULL CHECK (algorithm = 'ed25519'),
    device_id TEXT NOT NULL REFERENCES devices(device_id),
    key_generation INTEGER NOT NULL CHECK (key_generation >= 1),
    status TEXT NOT NULL CHECK (status IN ('active', 'superseded', 'revoked')),
    originating_enrollment_request_id TEXT NOT NULL
        REFERENCES enrollment_requests(enrollment_request_id),
    activated_audit_event_id TEXT NOT NULL UNIQUE REFERENCES audit_events(audit_event_id),
    retired_audit_event_id TEXT UNIQUE REFERENCES audit_events(audit_event_id),
    activated_revision INTEGER NOT NULL CHECK (activated_revision >= 1),
    retired_revision INTEGER
        CHECK (retired_revision IS NULL OR retired_revision >= activated_revision),
    UNIQUE(device_id, key_generation),
    CHECK (
        (
            status = 'active'
            AND retired_audit_event_id IS NULL
            AND retired_revision IS NULL
        )
        OR (
            status IN ('superseded', 'revoked')
            AND retired_audit_event_id IS NOT NULL
            AND retired_revision IS NOT NULL
        )
    )
) STRICT;
CREATE UNIQUE INDEX one_active_device_control_key
    ON device_control_keys(device_id)
    WHERE status = 'active';

CREATE TABLE credential_bundles (
    issuance_id TEXT PRIMARY KEY,
    enrollment_request_id TEXT NOT NULL UNIQUE
        REFERENCES enrollment_requests(enrollment_request_id),
    device_id TEXT,
    format_version INTEGER NOT NULL CHECK (format_version >= 1),
    canonical_bundle_bytes BLOB NOT NULL CHECK (length(canonical_bundle_bytes) > 0),
    bundle_sha256 BLOB NOT NULL CHECK (length(bundle_sha256) = 32),
    activation_deadline_unix_ms INTEGER NOT NULL,
    created_at_unix_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE device_tokens (
    device_id TEXT PRIMARY KEY REFERENCES devices(device_id),
    enrollment_request_id TEXT NOT NULL UNIQUE REFERENCES enrollment_requests(enrollment_request_id),
    token_hash BLOB NOT NULL UNIQUE CHECK (length(token_hash) = 32)
) STRICT;

CREATE TABLE gateway_certificates (
    certificate_id TEXT PRIMARY KEY,
    device_id TEXT NOT NULL REFERENCES devices(device_id),
    enrollment_request_id TEXT NOT NULL UNIQUE REFERENCES enrollment_requests(enrollment_request_id),
    serial TEXT NOT NULL UNIQUE,
    spki_sha256 BLOB NOT NULL CHECK (length(spki_sha256) = 32),
    not_after_unix_ms INTEGER NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('active', 'revoked', 'expired', 'retired'))
) STRICT;
CREATE UNIQUE INDEX one_active_gateway_certificate
    ON gateway_certificates(device_id)
    WHERE status = 'active';

-- Singleton non-secret import draft. Passwords are not persisted between preview and commit.
CREATE TABLE pending_import_candidate (
    -- CHECK(singleton = 1) forces at most one pending candidate.
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    -- Canonical lowercase UUIDv7 for this candidate.
    candidate_id TEXT NOT NULL UNIQUE,
    -- Absolute UTC epoch-ms expiry of this pending draft.
    expires_at_unix_ms INTEGER NOT NULL,
    -- SHA-256 of the opaque preview token; token itself is not stored.
    preview_token_hash BLOB NOT NULL UNIQUE CHECK (length(preview_token_hash) = 32),
    -- Version of the non-secret seat+account fingerprint algorithm.
    nonsecret_fingerprint_version INTEGER NOT NULL CHECK (nonsecret_fingerprint_version >= 1),
    -- SHA-256 of the canonical non-secret candidate (seat_code + username). Commit CSV must match.
    nonsecret_fingerprint_sha256 BLOB NOT NULL CHECK (length(nonsecret_fingerprint_sha256) = 32),
    -- Server-authoritative redacted diff JSON; no passwords.
    redacted_preview_json TEXT NOT NULL
        CHECK (json_valid(redacted_preview_json) AND json_type(redacted_preview_json) = 'object')
) STRICT;

CREATE TABLE commands (
    command_id TEXT PRIMARY KEY,
    device_id TEXT NOT NULL REFERENCES devices(device_id),
    kind TEXT NOT NULL CHECK (kind IN (
        'sync_state', 'sync_secret', 'open_binding_prompt', 'lock_session',
        'unlock_session', 'terminate_session', 'reset_home'
    )),
    state TEXT NOT NULL CHECK (state IN (
        'created', 'received', 'running', 'succeeded', 'failed', 'cancelled',
        'expired', 'manual_intervention_required'
    )),
    request_fingerprint_version INTEGER NOT NULL CHECK (request_fingerprint_version >= 1),
    request_fingerprint_sha256 BLOB NOT NULL CHECK (length(request_fingerprint_sha256) = 32),
    group_correlation_id TEXT,
    payload_version INTEGER NOT NULL CHECK (payload_version >= 1),
    frozen_payload_json TEXT NOT NULL
        CHECK (json_valid(frozen_payload_json) AND json_type(frozen_payload_json) = 'object'),
    created_at_unix_ms INTEGER NOT NULL,
    deadline_at_unix_ms INTEGER,
    terminal_error_code TEXT,
    redacted_terminal_result_json TEXT
        CHECK (
            redacted_terminal_result_json IS NULL
            OR (
                json_valid(redacted_terminal_result_json)
                AND json_type(redacted_terminal_result_json) = 'object'
            )
        ),
    created_audit_event_id TEXT NOT NULL UNIQUE REFERENCES audit_events(audit_event_id)
) STRICT;
CREATE INDEX commands_device_id_state_index ON commands(device_id, state);

INSERT INTO provisioning_window(singleton, state, revision, last_audit_event_id)
VALUES (1, 'closed', 0, NULL);
