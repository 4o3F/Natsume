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
        retired_at_unix_ms -> Nullable<BigInt>,
    }
}

diesel::table! {
    device_home_targets (device_id) {
        device_id -> Text,
        reset_epoch -> Nullable<BigInt>,
    }
}

diesel::table! {
    device_power_targets (device_id) {
        device_id -> Text,
        shutdown_epoch -> Nullable<BigInt>,
        expires_at_unix_ms -> Nullable<BigInt>,
    }
}

diesel::table! {
    device_session_targets (device_id) {
        device_id -> Text,
        foreground_target -> Text,
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
        client_ip -> Nullable<Text>,
        server_observed_ip -> Nullable<Text>,
        ip_observed_at_unix_ms -> Nullable<BigInt>,
    }
}

diesel::table! {
    gateway_credentials (device_id) {
        device_id -> Text,
        credential_id -> Text,
        gateway_csr_der -> Nullable<Binary>,
        gateway_leaf_der -> Nullable<Binary>,
    }
}

diesel::table! {
    operator_accounts (operator_id) {
        operator_id -> Text,
        username -> Text,
        role -> Text,
        password_hash -> Text,
        credential_revision -> BigInt,
    }
}

diesel::table! {
    operator_invitations (invitation_id) {
        invitation_id -> Text,
        role -> Text,
        issuer_operator_id -> Text,
        token_hash -> Binary,
        created_at_unix_ms -> BigInt,
        expires_at_unix_ms -> BigInt,
    }
}

diesel::table! {
    operator_password_resets (operator_id) {
        operator_id -> Text,
        reset_id -> Text,
        issuer_operator_id -> Text,
        token_hash -> Binary,
        created_at_unix_ms -> BigInt,
        expires_at_unix_ms -> BigInt,
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
    organizations (organization_id) {
        organization_id -> BigInt,
        name_key -> Text,
        name_zh -> Text,
        name_en -> Text,
        country -> Text,
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
    }
}

diesel::table! {
    print_documents (ready_sequence) {
        ready_sequence -> BigInt,
        source_id -> Text,
        request_id -> Text,
        sha256 -> Text,
        bytes -> BigInt,
        pages -> BigInt,
        ahead_tasks -> BigInt,
        ahead_pages -> BigInt,
    }
}

diesel::table! {
    print_printers (printer_id) {
        printer_id -> Text,
        name -> Text,
        pairing_code_hash -> Nullable<Binary>,
        pairing_expires_at_unix_ms -> Nullable<BigInt>,
        client_id -> Nullable<Text>,
        client_token_hash -> Nullable<Binary>,
        client_confirmed -> Integer,
    }
}

diesel::table! {
    print_requests (source_id, request_id) {
        source_id -> Text,
        request_id -> Text,
        account_id -> Text,
        account -> Text,
        team_name -> Text,
        seat_label -> Nullable<Text>,
        original_filename -> Text,
        source_sha256 -> Text,
        generation_failure -> Nullable<Text>,
    }
}

diesel::table! {
    print_sources (source_id) {
        source_id -> Text,
        name -> Text,
        token_hash -> Binary,
        revoked -> Integer,
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

diesel::table! {
    target_submission_receipts (operation_id) {
        operation_id -> Text,
        operator_id -> Text,
        request_json -> Text,
        results_json -> Text,
    }
}

diesel::table! {
    teams (account_id) {
        account_id -> Text,
        organization_id -> BigInt,
        name_zh -> Text,
        name_en -> Text,
        category -> Text,
    }
}

diesel::joinable!(account_mappings -> accounts (account_id));
diesel::joinable!(account_mappings -> seats (seat_id));
diesel::joinable!(binding_negotiations -> devices (device_id));
diesel::joinable!(device_bindings -> devices (device_id));
diesel::joinable!(device_bindings -> seats (seat_id));
diesel::joinable!(device_control_keys -> devices (device_id));
diesel::joinable!(device_home_targets -> devices (device_id));
diesel::joinable!(device_power_targets -> devices (device_id));
diesel::joinable!(device_session_targets -> devices (device_id));
diesel::joinable!(gateway_credentials -> devices (device_id));
diesel::joinable!(operator_invitations -> operator_accounts (issuer_operator_id));
diesel::joinable!(operator_sessions -> operator_accounts (operator_id));
diesel::joinable!(print_requests -> print_sources (source_id));
diesel::joinable!(server_vault_records -> accounts (account_id));
diesel::joinable!(teams -> accounts (account_id));
diesel::joinable!(teams -> organizations (organization_id));

diesel::allow_tables_to_appear_in_same_query!(
    account_mappings,
    accounts,
    binding_negotiations,
    device_bindings,
    device_control_keys,
    device_home_targets,
    device_power_targets,
    device_session_targets,
    devices,
    gateway_credentials,
    operator_accounts,
    operator_invitations,
    operator_password_resets,
    operator_sessions,
    organizations,
    pending_import_candidate,
    print_documents,
    print_printers,
    print_requests,
    print_sources,
    runtime_config,
    seats,
    server_vault_records,
    site_identity,
    target_submission_receipts,
    teams,
);
