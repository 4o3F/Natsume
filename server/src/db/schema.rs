// @generated automatically by Diesel CLI.

diesel::table! {
    accounts (account_id) {
        account_id -> Text,
        domjudge_username -> Text,
        credential_vault_record_id -> Text,
        credential_revision -> Integer,
    }
}

diesel::table! {
    audit_events (audit_event_id) {
        audit_event_id -> Text,
        occurred_at -> Text,
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
    device_bindings (seat_id) {
        seat_id -> Text,
        device_pk -> Text,
        binding_revision -> Integer,
    }
}

diesel::table! {
    device_tokens (device_pk) {
        device_pk -> Text,
        enrollment_request_id -> Text,
        token_hash -> Binary,
    }
}

diesel::table! {
    devices (device_pk) {
        device_pk -> Text,
        machine_hardware_id -> Text,
        hardware_identity_quality -> Text,
        state -> Text,
    }
}

diesel::table! {
    gateway_certificates (certificate_id) {
        certificate_id -> Text,
        device_pk -> Text,
        enrollment_request_id -> Text,
        serial -> Text,
        spki_sha256 -> Binary,
        not_after -> Text,
        status -> Text,
    }
}

diesel::table! {
    operator_accounts (operator_id) {
        operator_id -> Text,
        login_name -> Text,
        role -> Text,
        password_hash -> Text,
    }
}

diesel::table! {
    operator_sessions (session_credential_hash) {
        session_credential_hash -> Binary,
        operator_id -> Text,
        expires_at -> Text,
    }
}

diesel::table! {
    provisioning_window (singleton) {
        singleton -> Nullable<Integer>,
        state -> Text,
        revision -> Integer,
        last_audit_event_id -> Nullable<Text>,
    }
}

diesel::table! {
    seats (seat_id) {
        seat_id -> Text,
        seat_code -> Text,
    }
}

diesel::joinable!(device_bindings -> devices (device_pk));
diesel::joinable!(device_bindings -> seats (seat_id));
diesel::joinable!(device_tokens -> devices (device_pk));
diesel::joinable!(gateway_certificates -> devices (device_pk));
diesel::joinable!(operator_sessions -> operator_accounts (operator_id));
diesel::joinable!(provisioning_window -> audit_events (last_audit_event_id));

diesel::allow_tables_to_appear_in_same_query!(
    accounts,
    audit_events,
    device_bindings,
    device_tokens,
    devices,
    gateway_certificates,
    operator_accounts,
    operator_sessions,
    provisioning_window,
    seats,
);
