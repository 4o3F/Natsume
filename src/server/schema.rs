// @generated automatically by Diesel CLI.

diesel::table! {
    id_bind (mac) {
        mac -> Text,
        id -> Text,
    }
}

diesel::table! {
    player (id) {
        id -> Text,
        username -> Text,
        password -> Text,
        synced -> Integer,
    }
}

diesel::allow_tables_to_appear_in_same_query!(
    id_bind,
    player,
);
