// @generated automatically by Diesel CLI.

diesel::table! {
    account_mappings (seat_id) {
        seat_id -> Text,
        account_id -> Text,
    }
}

diesel::table! {
    accounts (account_id) {
        account_id -> Text,
        domjudge_username -> Text,
        credential_revision -> BigInt,
    }
}

diesel::table! {
    audit_events (audit_event_id) {
        audit_event_id -> Text,
        occurred_at_unix_ms -> BigInt,
        actor -> Text,
        action_kind -> Text,
        resource_type -> Text,
        resource_id -> Nullable<Text>,
        result -> Text,
        reason_code -> Nullable<Text>,
        correlation_id -> Text,
        group_correlation_id -> Nullable<Text>,
        redacted_detail_json -> Text,
    }
}

diesel::table! {
    binding_negotiations (device_id) {
        device_id -> Text,
        negotiation_id -> Text,
        submission_epoch -> Nullable<BigInt>,
        seat_code -> Nullable<Text>,
        evaluation_error_code -> Nullable<Text>,
    }
}

diesel::table! {
    device_bindings (binding_id) {
        binding_id -> Text,
        device_id -> Text,
        seat_id -> Text,
    }
}

diesel::table! {
    device_control_keys (public_key) {
        public_key -> Binary,
        device_id -> Text,
        status -> Text,
        activated_at_unix_ms -> BigInt,
        activated_audit_event_id -> Text,
        retired_at_unix_ms -> Nullable<BigInt>,
        retired_audit_event_id -> Nullable<Text>,
    }
}

diesel::table! {
    device_home_targets (device_id) {
        device_id -> Text,
        reset_epoch -> Nullable<BigInt>,
    }
}

diesel::table! {
    device_session_targets (device_id) {
        device_id -> Text,
        lock_state -> Text,
        terminate_epoch -> Nullable<BigInt>,
    }
}

diesel::table! {
    devices (device_id) {
        device_id -> Text,
        machine_hardware_id -> Text,
        evidence_quality -> Text,
        state -> Text,
        created_at_unix_ms -> BigInt,
    }
}

diesel::table! {
    gateway_credentials (device_id) {
        device_id -> Text,
        credential_id -> Text,
        gateway_csr_der -> Nullable<Binary>,
        gateway_leaf_der -> Nullable<Binary>,
        issuer_chain_der -> Nullable<Binary>,
    }
}

diesel::table! {
    operator_accounts (operator_id) {
        operator_id -> Text,
        username -> Text,
        role -> Text,
        password_hash -> Text,
    }
}

diesel::table! {
    operator_sessions (session_credential_hash) {
        session_credential_hash -> Binary,
        operator_id -> Text,
        expires_at_unix_ms -> BigInt,
    }
}

diesel::table! {
    pending_import_candidate (singleton) {
        singleton -> Integer,
        candidate_id -> Text,
        expires_at_unix_ms -> BigInt,
        preview_token_hash -> Binary,
        fingerprint_version -> Integer,
        candidate_fingerprint_sha256 -> Binary,
        baseline_fingerprint_sha256 -> Binary,
        redacted_preview_json -> Text,
        created_audit_event_id -> Text,
    }
}

diesel::table! {
    runtime_config (singleton) {
        singleton -> Integer,
        domjudge_origin -> Text,
    }
}

diesel::table! {
    seats (seat_id) {
        seat_id -> Text,
        seat_code -> Text,
    }
}

diesel::table! {
    server_vault_records (account_id) {
        account_id -> Text,
        nonce -> Binary,
        ciphertext -> Binary,
    }
}

diesel::table! {
    site_identity (singleton) {
        singleton -> Integer,
        fleet_namespace_uuid -> Text,
    }
}

diesel::joinable!(account_mappings -> accounts (account_id));
diesel::joinable!(account_mappings -> seats (seat_id));
diesel::joinable!(binding_negotiations -> devices (device_id));
diesel::joinable!(device_bindings -> devices (device_id));
diesel::joinable!(device_bindings -> seats (seat_id));
diesel::joinable!(device_control_keys -> devices (device_id));
diesel::joinable!(device_home_targets -> devices (device_id));
diesel::joinable!(device_session_targets -> devices (device_id));
diesel::joinable!(gateway_credentials -> devices (device_id));
diesel::joinable!(operator_sessions -> operator_accounts (operator_id));
diesel::joinable!(pending_import_candidate -> audit_events (created_audit_event_id));
diesel::joinable!(server_vault_records -> accounts (account_id));

diesel::allow_tables_to_appear_in_same_query!(
    account_mappings,
    accounts,
    audit_events,
    binding_negotiations,
    device_bindings,
    device_control_keys,
    device_home_targets,
    device_session_targets,
    devices,
    gateway_credentials,
    operator_accounts,
    operator_sessions,
    pending_import_candidate,
    runtime_config,
    seats,
    server_vault_records,
    site_identity,
);
