PRAGMA foreign_keys = ON;

CREATE TABLE schema_metadata (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    schema_version INTEGER NOT NULL,
    initialized_at TEXT NOT NULL
) STRICT;

CREATE TABLE site_identity (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    fleet_namespace_uuid TEXT NOT NULL UNIQUE,
    control_root_sha256 BLOB NOT NULL,
    local_origin_root_sha256 BLOB NOT NULL,
    initialized_at TEXT NOT NULL
) STRICT;
CREATE TRIGGER site_identity_is_immutable_on_update
BEFORE UPDATE ON site_identity
BEGIN
    SELECT RAISE(ABORT, 'site_identity_immutable');
END;
CREATE TRIGGER site_identity_is_immutable_on_delete
BEFORE DELETE ON site_identity
BEGIN
    SELECT RAISE(ABORT, 'site_identity_immutable');
END;

CREATE TABLE instance_state (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    seat_universe_frozen_at TEXT,
    seat_universe_hash BLOB,
    initialized_at TEXT NOT NULL,
    CHECK (
        (seat_universe_frozen_at IS NULL AND seat_universe_hash IS NULL)
        OR
        (seat_universe_frozen_at IS NOT NULL AND seat_universe_hash IS NOT NULL)
    )
) STRICT;
CREATE TRIGGER frozen_seat_universe_is_immutable
BEFORE UPDATE OF seat_universe_frozen_at, seat_universe_hash ON instance_state
WHEN OLD.seat_universe_frozen_at IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'seat_universe_immutable');
END;

CREATE TABLE server_vault_records (
    vault_record_id TEXT PRIMARY KEY,
    record_type TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    key_version INTEGER NOT NULL,
    aad_version INTEGER NOT NULL,
    nonce BLOB NOT NULL,
    ciphertext BLOB NOT NULL,
    created_at TEXT NOT NULL,
    superseded_at TEXT
) STRICT;
CREATE UNIQUE INDEX server_vault_active_subject
    ON server_vault_records(record_type, subject_id)
    WHERE superseded_at IS NULL;

CREATE TABLE system_configuration_revisions (
    configuration_revision_id TEXT PRIMARY KEY,
    revision_no INTEGER NOT NULL UNIQUE,
    domjudge_upstream_url TEXT NOT NULL,
    domjudge_upstream_host_header TEXT NOT NULL,
    client_origin_hostname TEXT NOT NULL,
    browser_start_path TEXT NOT NULL,
    domjudge_login_path TEXT NOT NULL,
    gateway_certificate_profile_id TEXT NOT NULL,
    browser_policy_revision TEXT NOT NULL,
    home_template_revision TEXT NOT NULL,
    created_by TEXT NOT NULL,
    created_at TEXT NOT NULL,
    activated_at TEXT,
    deactivated_at TEXT
) STRICT;
CREATE UNIQUE INDEX one_active_system_configuration
    ON system_configuration_revisions((1))
    WHERE activated_at IS NOT NULL AND deactivated_at IS NULL;

CREATE TABLE automation_policy_revisions (
    policy_revision_id TEXT PRIMARY KEY,
    revision_no INTEGER NOT NULL UNIQUE,
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    enabled_until TEXT,
    allowed_subnets_json TEXT NOT NULL,
    max_automatic_devices INTEGER NOT NULL CHECK (max_automatic_devices >= 0),
    minimum_hardware_identity_quality TEXT NOT NULL
        CHECK (minimum_hardware_identity_quality IN ('weak', 'medium', 'strong')),
    auto_approve_enrollment INTEGER NOT NULL CHECK (auto_approve_enrollment IN (0, 1)),
    auto_approve_binding_request INTEGER NOT NULL CHECK (auto_approve_binding_request IN (0, 1)),
    auto_sync_state_after_binding INTEGER NOT NULL CHECK (auto_sync_state_after_binding IN (0, 1)),
    auto_open_binding_prompt_on_connect INTEGER NOT NULL CHECK (auto_open_binding_prompt_on_connect IN (0, 1)),
    created_by TEXT NOT NULL,
    created_at TEXT NOT NULL,
    activated_at TEXT,
    deactivated_at TEXT
) STRICT;
CREATE UNIQUE INDEX one_active_automation_policy
    ON automation_policy_revisions((1))
    WHERE activated_at IS NOT NULL AND deactivated_at IS NULL;

CREATE TABLE seats (
    seat_id TEXT PRIMARY KEY,
    label TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL,
    row_version INTEGER NOT NULL DEFAULT 1
) STRICT;
CREATE TRIGGER seat_label_is_immutable
BEFORE UPDATE OF label ON seats
BEGIN
    SELECT RAISE(ABORT, 'seat_label_immutable');
END;
CREATE TRIGGER no_new_seat_after_universe_freeze
BEFORE INSERT ON seats
WHEN (SELECT seat_universe_frozen_at FROM instance_state WHERE singleton = 1) IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'seat_universe_immutable');
END;
CREATE TRIGGER no_seat_delete_after_universe_freeze
BEFORE DELETE ON seats
WHEN (SELECT seat_universe_frozen_at FROM instance_state WHERE singleton = 1) IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'seat_universe_immutable');
END;

CREATE TABLE accounts (
    account_id TEXT PRIMARY KEY,
    domjudge_username TEXT NOT NULL UNIQUE,
    row_version INTEGER NOT NULL DEFAULT 1
) STRICT;

CREATE TABLE credential_revisions (
    credential_revision_id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES accounts(account_id),
    revision_no INTEGER NOT NULL,
    password_vault_record_id TEXT NOT NULL REFERENCES server_vault_records(vault_record_id),
    created_at TEXT NOT NULL,
    superseded_at TEXT,
    UNIQUE(account_id, revision_no)
) STRICT;
CREATE UNIQUE INDEX one_active_credential_per_account
    ON credential_revisions(account_id)
    WHERE superseded_at IS NULL;

CREATE TABLE seat_assignments (
    seat_assignment_id TEXT PRIMARY KEY,
    seat_id TEXT NOT NULL REFERENCES seats(seat_id),
    account_id TEXT REFERENCES accounts(account_id),
    revision_no INTEGER NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('active', 'superseded', 'unassigned')),
    created_by TEXT NOT NULL,
    created_at TEXT NOT NULL,
    superseded_at TEXT,
    UNIQUE(seat_id, revision_no),
    CHECK (
        (state = 'unassigned' AND account_id IS NULL)
        OR
        (state IN ('active', 'superseded') AND account_id IS NOT NULL)
    )
) STRICT;
CREATE UNIQUE INDEX one_active_assignment_per_seat
    ON seat_assignments(seat_id) WHERE state = 'active';
CREATE UNIQUE INDEX one_active_seat_per_account
    ON seat_assignments(account_id) WHERE state = 'active';

CREATE TABLE devices (
    device_pk TEXT PRIMARY KEY,
    machine_hardware_id TEXT NOT NULL UNIQUE,
    hardware_identity_quality TEXT NOT NULL
        CHECK (hardware_identity_quality IN ('strong', 'medium', 'weak')),
    enrollment_state TEXT NOT NULL
        CHECK (enrollment_state IN ('pending', 'approved', 'enrolled', 'revoked', 'disabled')),
    daemon_version TEXT,
    agent_version TEXT,
    last_source_ip TEXT,
    last_seen_at TEXT,
    disabled_at TEXT,
    row_version INTEGER NOT NULL DEFAULT 1
) STRICT;
CREATE TRIGGER machine_hardware_id_is_immutable
BEFORE UPDATE OF machine_hardware_id ON devices
BEGIN
    SELECT RAISE(ABORT, 'machine_hardware_id_immutable');
END;

CREATE TABLE device_bindings (
    device_binding_id TEXT PRIMARY KEY,
    device_pk TEXT NOT NULL REFERENCES devices(device_pk),
    seat_id TEXT NOT NULL REFERENCES seats(seat_id),
    state TEXT NOT NULL CHECK (state IN ('active', 'revoked')),
    revision_no INTEGER NOT NULL,
    request_source TEXT NOT NULL CHECK (request_source IN ('panel', 'device_prompt', 'automation')),
    created_by TEXT NOT NULL,
    created_at TEXT NOT NULL,
    revoked_at TEXT,
    UNIQUE(device_pk, revision_no)
) STRICT;
CREATE UNIQUE INDEX one_active_binding_per_device
    ON device_bindings(device_pk) WHERE state = 'active';
CREATE UNIQUE INDEX one_active_device_per_seat
    ON device_bindings(seat_id) WHERE state = 'active';

CREATE TABLE device_certificates (
    certificate_id TEXT PRIMARY KEY,
    device_pk TEXT NOT NULL REFERENCES devices(device_pk),
    serial TEXT NOT NULL UNIQUE,
    fingerprint BLOB NOT NULL UNIQUE,
    spki_sha256 BLOB NOT NULL,
    not_before TEXT NOT NULL,
    not_after TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('active', 'revoked', 'expired')),
    created_at TEXT NOT NULL,
    revoked_at TEXT
) STRICT;
CREATE UNIQUE INDEX one_active_device_certificate
    ON device_certificates(device_pk) WHERE state = 'active';


CREATE TABLE device_target_states (
    device_pk TEXT NOT NULL REFERENCES devices(device_pk),
    generation INTEGER NOT NULL,
    canonical_hash BLOB NOT NULL,
    snapshot_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY(device_pk, generation)
) STRICT;

CREATE TABLE observed_device_states (
    device_pk TEXT PRIMARY KEY REFERENCES devices(device_pk),
    observed_sequence INTEGER NOT NULL,
    boot_id TEXT NOT NULL,
    received_generation INTEGER NOT NULL,
    applied_generation INTEGER NOT NULL,
    applied_hash BLOB,
    state_apply_status TEXT NOT NULL,
    state_error_code TEXT,
    installed_assignment_revision INTEGER,
    installed_credential_revision_id TEXT,
    secret_state TEXT NOT NULL,
    gateway_state TEXT NOT NULL,
    gateway_configuration_revision_id TEXT,
    gateway_certificate_fingerprint BLOB,
    gateway_certificate_not_after TEXT,
    session_state TEXT NOT NULL,
    session_instance_id TEXT,
    session_epoch INTEGER,
    session_lock_state TEXT,
    session_lock_epoch INTEGER,
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
    home_state TEXT NOT NULL,
    observed_at TEXT NOT NULL
) STRICT;

CREATE TABLE enrollment_challenges (
    challenge_id TEXT PRIMARY KEY,
    machine_hardware_id TEXT NOT NULL,
    challenge_sha256 BLOB NOT NULL,
    expires_at TEXT NOT NULL,
    consumed_at TEXT,
    created_at TEXT NOT NULL
) STRICT;

CREATE TABLE enrollment_requests (
    enrollment_request_id TEXT PRIMARY KEY,
    challenge_id TEXT NOT NULL REFERENCES enrollment_challenges(challenge_id),
    machine_hardware_id TEXT NOT NULL,
    hardware_identity_quality TEXT NOT NULL
        CHECK (hardware_identity_quality IN ('strong', 'medium', 'weak')),
    device_csr_der BLOB NOT NULL,
    device_spki_sha256 BLOB NOT NULL,
    request_nonce_sha256 BLOB NOT NULL,
    poll_challenge_sha256 BLOB NOT NULL,
    poll_challenge_expires_at TEXT NOT NULL,
    software_version TEXT NOT NULL,
    source_ip TEXT NOT NULL,
    state TEXT NOT NULL
        CHECK (state IN ('pending', 'approved', 'rejected', 'issued', 'expired', 'conflict')),
    approval_source TEXT CHECK (approval_source IN ('manual', 'automation')),
    resolution TEXT CHECK (resolution IN ('create_device', 'rekey_existing_device')),
    resolved_device_pk TEXT REFERENCES devices(device_pk),
    created_at TEXT NOT NULL,
    decided_at TEXT
) STRICT;
CREATE UNIQUE INDEX one_live_enrollment_per_machine_and_spki
    ON enrollment_requests(machine_hardware_id, device_spki_sha256)
    WHERE state IN ('pending', 'approved');

CREATE TABLE csv_imports (
    csv_import_id TEXT PRIMARY KEY,
    state TEXT NOT NULL
        CHECK (state IN ('uploaded', 'parsed', 'previewed', 'committed', 'failed', 'expired')),
    content_sha256 BLOB NOT NULL,
    row_count INTEGER NOT NULL,
    seat_set_hash BLOB NOT NULL,
    expires_at TEXT NOT NULL,
    created_by TEXT NOT NULL,
    parse_summary_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    committed_at TEXT
) STRICT;

CREATE TABLE csv_import_rows (
    csv_import_id TEXT NOT NULL REFERENCES csv_imports(csv_import_id) ON DELETE CASCADE,
    row_number INTEGER NOT NULL,
    normalized_seat TEXT NOT NULL,
    normalized_account TEXT,
    password_vault_record_id TEXT REFERENCES server_vault_records(vault_record_id),
    planned_action TEXT NOT NULL,
    validation_error_code TEXT,
    PRIMARY KEY(csv_import_id, row_number),
    UNIQUE(csv_import_id, normalized_seat),
    CHECK (
        (normalized_account IS NULL AND password_vault_record_id IS NULL)
        OR
        (normalized_account IS NOT NULL AND password_vault_record_id IS NOT NULL)
    )
) STRICT;

CREATE TABLE operations (
    operation_id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    state TEXT NOT NULL,
    actor TEXT NOT NULL,
    reason TEXT,
    selection_digest BLOB NOT NULL,
    target_count INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    completed_at TEXT
) STRICT;

CREATE TABLE operation_targets (
    operation_target_id TEXT PRIMARY KEY,
    operation_id TEXT NOT NULL REFERENCES operations(operation_id),
    device_pk TEXT NOT NULL REFERENCES devices(device_pk),
    state TEXT NOT NULL,
    UNIQUE(operation_id, device_pk)
) STRICT;

CREATE TABLE commands (
    command_id TEXT PRIMARY KEY,
    operation_target_id TEXT NOT NULL UNIQUE REFERENCES operation_targets(operation_target_id),
    device_pk TEXT NOT NULL REFERENCES devices(device_pk),
    kind TEXT NOT NULL,
    state TEXT NOT NULL,
    payload_json TEXT,
    payload_vault_record_id TEXT REFERENCES server_vault_records(vault_record_id),
    created_at TEXT NOT NULL,
    deadline_at TEXT NOT NULL,
    terminal_result_json TEXT,
    CHECK ((payload_json IS NOT NULL) <> (payload_vault_record_id IS NOT NULL))
) STRICT;


CREATE TABLE gateway_certificates (
    certificate_id TEXT PRIMARY KEY,
    device_pk TEXT NOT NULL REFERENCES devices(device_pk),
    issued_for_command_id TEXT NOT NULL REFERENCES commands(command_id),
    configuration_revision_id TEXT NOT NULL REFERENCES system_configuration_revisions(configuration_revision_id),
    target_generation INTEGER NOT NULL,
    dns_san TEXT NOT NULL,
    certificate_profile_id TEXT NOT NULL,
    serial TEXT NOT NULL UNIQUE,
    fingerprint BLOB NOT NULL UNIQUE,
    spki_sha256 BLOB NOT NULL,
    not_before TEXT NOT NULL,
    not_after TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('active', 'revoked', 'expired', 'superseded')),
    created_at TEXT NOT NULL,
    revoked_at TEXT
) STRICT;
CREATE UNIQUE INDEX one_active_gateway_certificate
    ON gateway_certificates(device_pk)
    WHERE state = 'active';
CREATE INDEX gateway_certificate_by_command
    ON gateway_certificates(issued_for_command_id);

CREATE TABLE gateway_certificate_requests (
    gateway_certificate_request_id TEXT PRIMARY KEY,
    command_id TEXT NOT NULL UNIQUE REFERENCES commands(command_id),
    device_pk TEXT NOT NULL REFERENCES devices(device_pk),
    target_generation INTEGER NOT NULL,
    configuration_revision_id TEXT NOT NULL REFERENCES system_configuration_revisions(configuration_revision_id),
    csr_der BLOB NOT NULL,
    spki_sha256 BLOB NOT NULL,
    request_nonce_sha256 BLOB NOT NULL,
    state TEXT NOT NULL CHECK (
        state IN ('pending', 'issued', 'rejected', 'conflict', 'temporarily_unavailable', 'expired')
    ),
    issued_certificate_id TEXT REFERENCES gateway_certificates(certificate_id),
    stable_error_code TEXT,
    created_at TEXT NOT NULL,
    completed_at TEXT,
    CHECK (
        (state = 'issued' AND issued_certificate_id IS NOT NULL AND stable_error_code IS NULL)
        OR
        (state != 'issued' AND issued_certificate_id IS NULL)
    )
) STRICT;
CREATE UNIQUE INDEX gateway_certificate_request_bound_identity
    ON gateway_certificate_requests(
        command_id,
        target_generation,
        configuration_revision_id,
        spki_sha256
    );
CREATE INDEX gateway_certificate_request_by_device_state
    ON gateway_certificate_requests(device_pk, state);
CREATE TRIGGER gateway_certificate_request_requires_sync_state
BEFORE INSERT ON gateway_certificate_requests
WHEN NOT EXISTS (
    SELECT 1
    FROM commands
    WHERE commands.command_id = NEW.command_id
      AND commands.device_pk = NEW.device_pk
      AND commands.kind = 'SYNC_STATE'
)
BEGIN
    SELECT RAISE(ABORT, 'gateway_certificate_request_requires_sync_state');
END;

CREATE TABLE command_attempts (
    command_attempt_id TEXT PRIMARY KEY,
    command_id TEXT NOT NULL REFERENCES commands(command_id),
    connection_epoch INTEGER,
    attempted_at TEXT NOT NULL,
    result_code TEXT
) STRICT;

CREATE TABLE idempotency_records (
    actor TEXT NOT NULL,
    endpoint TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    request_hash BLOB NOT NULL,
    resource_type TEXT NOT NULL,
    resource_id TEXT NOT NULL,
    response_status INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    PRIMARY KEY(actor, endpoint, idempotency_key)
) STRICT;

CREATE TABLE audit_events (
    audit_event_id TEXT PRIMARY KEY,
    occurred_at TEXT NOT NULL,
    actor TEXT NOT NULL,
    action_kind TEXT NOT NULL,
    resource_type TEXT NOT NULL,
    resource_id TEXT,
    target_count INTEGER,
    reason TEXT,
    result TEXT NOT NULL,
    detail_json TEXT NOT NULL
) STRICT;

CREATE TABLE change_events (
    change_cursor INTEGER PRIMARY KEY AUTOINCREMENT,
    occurred_at TEXT NOT NULL,
    entity_type TEXT NOT NULL,
    entity_id TEXT NOT NULL,
    change_kind TEXT NOT NULL
) STRICT;

INSERT INTO schema_metadata(singleton, schema_version, initialized_at)
VALUES (1, 1, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));

INSERT INTO instance_state(singleton, seat_universe_frozen_at, seat_universe_hash, initialized_at)
VALUES (1, NULL, NULL, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));
