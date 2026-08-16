PRAGMA foreign_keys = ON;

CREATE TABLE site_identity (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    fleet_namespace_uuid TEXT NOT NULL UNIQUE
) STRICT;

CREATE TABLE revision_counters (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    configuration_revision INTEGER NOT NULL CHECK (configuration_revision >= 0),
    binding_revision INTEGER NOT NULL CHECK (binding_revision >= 0)
) STRICT;

CREATE TABLE server_vault_records (
    vault_record_id TEXT PRIMARY KEY,
    record_type TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    nonce BLOB NOT NULL CHECK (length(nonce) > 0),
    ciphertext BLOB NOT NULL CHECK (length(ciphertext) > 0),
    UNIQUE(record_type, subject_id)
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
    state TEXT NOT NULL CHECK (state IN ('enrolled', 'revoked', 'disabled'))
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
    state TEXT NOT NULL
        CHECK (state IN ('pending', 'approved', 'rejected', 'issued', 'expired', 'conflict')),
    resolution TEXT CHECK (resolution IN ('create_device', 'replace_device_credentials')),
    resolved_device_pk TEXT REFERENCES devices(device_pk),
    issuance_audit_event_id TEXT UNIQUE REFERENCES audit_events(audit_event_id),
    created_at TEXT NOT NULL,
    CHECK (
        state != 'issued'
        OR (
            resolution IS NOT NULL
            AND resolved_device_pk IS NOT NULL
            AND issuance_audit_event_id IS NOT NULL
        )
    ),
    CHECK (issuance_audit_event_id IS NULL OR state = 'issued'),
    FOREIGN KEY (resolved_device_pk, machine_hardware_id)
        REFERENCES devices(device_pk, machine_hardware_id)
) STRICT;
CREATE UNIQUE INDEX one_live_enrollment_per_machine_and_gateway_spki
    ON enrollment_requests(machine_hardware_id, gateway_spki_sha256)
    WHERE state IN ('pending', 'approved');

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

CREATE TABLE pending_import_candidate (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    candidate_id TEXT NOT NULL UNIQUE,
    expires_at TEXT NOT NULL,
    baseline_configuration_revision INTEGER NOT NULL CHECK (baseline_configuration_revision >= 0),
    baseline_binding_revision INTEGER NOT NULL CHECK (baseline_binding_revision >= 0),
    preview_token_hash BLOB NOT NULL UNIQUE CHECK (length(preview_token_hash) = 32),
    payload_vault_record_id TEXT NOT NULL UNIQUE
        REFERENCES server_vault_records(vault_record_id),
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
INSERT INTO revision_counters(singleton, configuration_revision, binding_revision)
VALUES (1, 0, 0);

INSERT INTO provisioning_window(singleton, state, revision, last_audit_event_id)
VALUES (1, 'closed', 0, NULL);
