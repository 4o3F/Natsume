// @generated automatically by Diesel CLI.

diesel::table! {
    journal_events (id) {
        id -> BigInt,
        occurred_at_ms -> BigInt,
        boot_id -> Text,
        kind -> Text,
        category -> Text,
        severity -> Text,
        device_id -> Nullable<Text>,
        session_id -> Nullable<Text>,
        actor_operator_id -> Nullable<Text>,
        request_id -> Nullable<Text>,
        operation_id -> Nullable<Text>,
        trace_id -> Nullable<Text>,
        seat_code -> Nullable<Text>,
        details -> Text,
    }
}
