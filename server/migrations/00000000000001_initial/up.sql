PRAGMA foreign_keys = ON;

-- One-row site identity. Machine Hardware IDs derive from fleet_namespace_uuid.
CREATE TABLE site_identity (
    -- CHECK(singleton = 1) forces exactly one row.
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    -- Public immutable fleet UUID; UUIDv5 namespace for Machine Hardware ID (ADR-0032).
    fleet_namespace_uuid TEXT NOT NULL UNIQUE
) STRICT;

-- Current-fact AEAD store for Account passwords only. No import staging, no history rows.
CREATE TABLE server_vault_records (
    -- Canonical lowercase UUIDv7 identifier for this vault row.
    vault_record_id TEXT PRIMARY KEY,
    -- Matches accounts.account_id. No FK: accounts references this row (insert vault first).
    account_id TEXT NOT NULL UNIQUE,
    -- AEAD nonce (XChaCha20-Poly1305: 24 bytes in the vault implementation).
    nonce BLOB NOT NULL CHECK (length(nonce) > 0),
    -- AEAD ciphertext of the current DOMjudge password; plaintext never stored.
    ciphertext BLOB NOT NULL CHECK (length(ciphertext) > 0)
) STRICT;

CREATE TABLE seats (
    seat_id TEXT PRIMARY KEY,
    seat_code TEXT NOT NULL UNIQUE
) STRICT;

CREATE TABLE devices (
    device_pk TEXT PRIMARY KEY,
    machine_hardware_id TEXT NOT NULL UNIQUE,
    hardware_identity_quality TEXT NOT NULL
        CHECK (hardware_identity_quality IN ('strong', 'medium', 'weak')),
    state TEXT NOT NULL CHECK (state IN ('enrolled', 'revoked', 'disabled')),
    control_authority_revision INTEGER
        CHECK (control_authority_revision IS NULL OR control_authority_revision >= 1)
) STRICT;
CREATE UNIQUE INDEX device_pk_machine_hardware_identity
    ON devices(device_pk, machine_hardware_id);

CREATE TABLE audit_events (
    audit_event_id TEXT PRIMARY KEY,
    occurred_at TEXT NOT NULL,
    actor TEXT NOT NULL,
    action_kind TEXT NOT NULL,
    resource_type TEXT NOT NULL,
    resource_id TEXT,
    result TEXT NOT NULL CHECK (result IN ('succeeded', 'rejected', 'failed', 'noop')),
    reason_code TEXT,
    correlation_id TEXT NOT NULL,
    group_correlation_id TEXT,
    redacted_detail_json TEXT NOT NULL
        CHECK (json_valid(redacted_detail_json) AND json_type(redacted_detail_json) = 'object')
) STRICT;
CREATE TABLE operator_accounts (
    operator_id TEXT PRIMARY KEY,
    login_name TEXT NOT NULL UNIQUE,
    role TEXT NOT NULL CHECK (role IN ('admin', 'viewer')),
    password_hash TEXT NOT NULL
) STRICT;

CREATE TABLE operator_sessions (
    session_credential_hash BLOB PRIMARY KEY CHECK (length(session_credential_hash) = 32),
    operator_id TEXT NOT NULL REFERENCES operator_accounts(operator_id),
    expires_at TEXT NOT NULL CHECK (
        strftime('%Y-%m-%dT%H:%M:%fZ', expires_at) IS NOT NULL
        AND expires_at = strftime('%Y-%m-%dT%H:%M:%fZ', expires_at)
    )
) STRICT;

CREATE TABLE accounts (
    account_id TEXT PRIMARY KEY,
    domjudge_username TEXT NOT NULL UNIQUE,
    credential_vault_record_id TEXT NOT NULL UNIQUE
        REFERENCES server_vault_records(vault_record_id),
    credential_revision INTEGER NOT NULL CHECK (credential_revision >= 1)
) STRICT;

CREATE TABLE account_mappings (
    seat_id TEXT PRIMARY KEY REFERENCES seats(seat_id),
    account_id TEXT NOT NULL UNIQUE REFERENCES accounts(account_id)
) STRICT;

CREATE TABLE device_bindings (
    seat_id TEXT PRIMARY KEY REFERENCES seats(seat_id),
    device_pk TEXT NOT NULL UNIQUE REFERENCES devices(device_pk),
    binding_revision INTEGER NOT NULL CHECK (binding_revision > 0)
) STRICT;

CREATE TABLE observed_device_states (
    device_pk TEXT PRIMARY KEY REFERENCES devices(device_pk),
    observed_sequence INTEGER NOT NULL CHECK (observed_sequence >= 0),
    boot_id TEXT NOT NULL,
    received_generation INTEGER NOT NULL CHECK (received_generation >= 0),
    applied_generation INTEGER NOT NULL CHECK (applied_generation >= 0),
    applied_hash BLOB CHECK (applied_hash IS NULL OR length(applied_hash) = 32),
    state_apply_status TEXT NOT NULL CHECK (state_apply_status IN (
        'idle', 'received', 'validating', 'applying', 'applied', 'failed', 'recovery_required'
    )),
    state_error_code TEXT,
    installed_binding_revision INTEGER CHECK (
        installed_binding_revision IS NULL OR installed_binding_revision >= 0
    ),
    installed_credential_revision INTEGER CHECK (
        installed_credential_revision IS NULL OR installed_credential_revision >= 0
    ),
    secret_state TEXT NOT NULL
        CHECK (secret_state IN ('absent', 'installed', 'stale', 'failed')),
    gateway_state TEXT NOT NULL CHECK (gateway_state IN (
        'absent', 'blocked', 'restoring', 'ready', 'upstream_unhealthy', 'recovery_required'
    )),
    gateway_configuration_revision INTEGER CHECK (
        gateway_configuration_revision IS NULL OR gateway_configuration_revision >= 0
    ),
    gateway_certificate_fingerprint BLOB CHECK (
        gateway_certificate_fingerprint IS NULL OR length(gateway_certificate_fingerprint) = 32
    ),
    gateway_certificate_not_after TEXT,
    session_state TEXT NOT NULL CHECK (session_state IN (
        'none', 'starting', 'active', 'locked', 'terminating', 'error'
    )),
    session_instance_id TEXT,
    session_epoch INTEGER CHECK (session_epoch IS NULL OR session_epoch >= 0),
    session_lock_state TEXT CHECK (
        session_lock_state IS NULL OR session_lock_state IN (
            'none', 'locking', 'locked', 'unlocking', 'unlocked', 'terminating', 'error'
        )
    ),
    session_lock_epoch INTEGER CHECK (session_lock_epoch IS NULL OR session_lock_epoch >= 0),
    active_lock_command_id TEXT,
    session_agent_state TEXT NOT NULL DEFAULT 'absent'
        CHECK (session_agent_state IN ('absent', 'starting', 'ready', 'degraded', 'error')),
    graphical_session_type TEXT
        CHECK (graphical_session_type IS NULL OR graphical_session_type IN ('wayland', 'x11')),
    display_backend TEXT
        CHECK (display_backend IS NULL OR display_backend IN ('wayland', 'x11')),
    ui_presentation_state TEXT NOT NULL DEFAULT 'hidden'
        CHECK (ui_presentation_state IN (
            'hidden', 'presenting', 'presented_focused', 'presented_unfocused', 'unsupported', 'failed'
        )),
    session_screen_kind TEXT NOT NULL DEFAULT 'hidden'
        CHECK (session_screen_kind IN (
            'hidden', 'idle_status', 'binding_prompt', 'binding_pending', 'binding_result',
            'recovery_status', 'lock_presentation', 'fatal_local_error'
        )),
    notifications_available INTEGER NOT NULL DEFAULT 0 CHECK (notifications_available IN (0, 1)),
    desktop_lock_supported INTEGER NOT NULL DEFAULT 0 CHECK (desktop_lock_supported IN (0, 1)),
    desktop_unlock_supported INTEGER NOT NULL DEFAULT 0 CHECK (desktop_unlock_supported IN (0, 1)),
    session_agent_error_code TEXT,
    home_state TEXT NOT NULL CHECK (home_state IN (
        'unmounted', 'ready', 'resetting', 'recovery_required', 'error'
    )),
    observed_at TEXT NOT NULL
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
    resolved_device_pk TEXT REFERENCES devices(device_pk),
    issuance_audit_event_id TEXT UNIQUE REFERENCES audit_events(audit_event_id),
    created_at TEXT NOT NULL,
    control_intent TEXT CHECK (
        control_intent IS NULL
        OR control_intent IN ('first', 'replace', 'recover', 'refresh')
    ),
    proposed_device_pk TEXT,
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
    activation_deadline TEXT,
    approval_audit_event_id TEXT UNIQUE REFERENCES audit_events(audit_event_id),
    CHECK (
        state != 'issued'
        OR (
            resolution IS NOT NULL
            AND resolved_device_pk IS NOT NULL
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
    ),
    FOREIGN KEY (resolved_device_pk, machine_hardware_id)
        REFERENCES devices(device_pk, machine_hardware_id)
) STRICT;
CREATE UNIQUE INDEX one_live_enrollment_per_machine_and_gateway_spki
    ON enrollment_requests(machine_hardware_id, gateway_spki_sha256)
    WHERE state IN ('pending', 'approved');
CREATE UNIQUE INDEX one_live_control_enrollment_per_machine
    ON enrollment_requests(machine_hardware_id)
    WHERE state IN ('pending_approval', 'awaiting_credential_ack')
        AND control_intent IS NOT NULL;
CREATE UNIQUE INDEX one_live_control_enrollment_per_resolved_device
    ON enrollment_requests(resolved_device_pk)
    WHERE resolved_device_pk IS NOT NULL
        AND state IN ('pending_approval', 'awaiting_credential_ack')
        AND control_intent IS NOT NULL;

CREATE TABLE device_control_keys (
    public_key BLOB PRIMARY KEY NOT NULL UNIQUE CHECK (length(public_key) = 32),
    algorithm TEXT NOT NULL CHECK (algorithm = 'ed25519'),
    device_pk TEXT NOT NULL REFERENCES devices(device_pk),
    key_generation INTEGER NOT NULL CHECK (key_generation >= 1),
    status TEXT NOT NULL CHECK (status IN ('active', 'superseded', 'revoked')),
    originating_enrollment_request_id TEXT NOT NULL
        REFERENCES enrollment_requests(enrollment_request_id),
    activated_audit_event_id TEXT NOT NULL UNIQUE REFERENCES audit_events(audit_event_id),
    retired_audit_event_id TEXT UNIQUE REFERENCES audit_events(audit_event_id),
    activated_revision INTEGER NOT NULL CHECK (activated_revision >= 1),
    retired_revision INTEGER
        CHECK (retired_revision IS NULL OR retired_revision >= activated_revision),
    UNIQUE(device_pk, key_generation),
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
    ON device_control_keys(device_pk)
    WHERE status = 'active';

CREATE TABLE credential_bundles (
    issuance_id TEXT PRIMARY KEY,
    enrollment_request_id TEXT NOT NULL UNIQUE
        REFERENCES enrollment_requests(enrollment_request_id),
    device_pk TEXT,
    format_version INTEGER NOT NULL CHECK (format_version >= 1),
    canonical_bundle_bytes BLOB NOT NULL CHECK (length(canonical_bundle_bytes) > 0),
    bundle_sha256 BLOB NOT NULL CHECK (length(bundle_sha256) = 32),
    activation_deadline TEXT NOT NULL,
    created_at TEXT NOT NULL
) STRICT;

CREATE TABLE device_tokens (
    device_pk TEXT PRIMARY KEY REFERENCES devices(device_pk),
    enrollment_request_id TEXT NOT NULL UNIQUE REFERENCES enrollment_requests(enrollment_request_id),
    token_hash BLOB NOT NULL UNIQUE CHECK (length(token_hash) = 32)
) STRICT;

CREATE TABLE gateway_certificates (
    certificate_id TEXT PRIMARY KEY,
    device_pk TEXT NOT NULL REFERENCES devices(device_pk),
    enrollment_request_id TEXT NOT NULL UNIQUE REFERENCES enrollment_requests(enrollment_request_id),
    serial TEXT NOT NULL UNIQUE,
    spki_sha256 BLOB NOT NULL CHECK (length(spki_sha256) = 32),
    not_after TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('active', 'revoked', 'expired', 'retired'))
) STRICT;
CREATE UNIQUE INDEX one_active_gateway_certificate
    ON gateway_certificates(device_pk)
    WHERE status = 'active';

-- Singleton non-secret import draft. Passwords are not persisted between preview and commit.
CREATE TABLE pending_import_candidate (
    -- CHECK(singleton = 1) forces at most one pending candidate.
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    -- Canonical lowercase UUIDv7 for this candidate.
    candidate_id TEXT NOT NULL UNIQUE,
    expires_at TEXT NOT NULL,
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
    device_pk TEXT NOT NULL REFERENCES devices(device_pk),
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
    created_at TEXT NOT NULL,
    deadline_at TEXT,
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
CREATE INDEX commands_device_pk_state_index ON commands(device_pk, state);

INSERT INTO provisioning_window(singleton, state, revision, last_audit_event_id)
VALUES (1, 'closed', 0, NULL);
