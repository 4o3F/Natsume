// @generated automatically by Diesel CLI.

diesel::table! {
    photo_roster (account_id) {
        account_id -> Text,
        account -> Text,
        organization_id -> BigInt,
        school_name_zh -> Text,
        school_name_en -> Text,
        team_name_zh -> Text,
        team_name_en -> Text,
        logo_path -> Nullable<Text>,
        logo_sha256 -> Nullable<Text>,
    }
}

diesel::table! {
    photo_workspace (singleton) {
        singleton -> Integer,
        format_version -> Integer,
        ready -> Bool,
        default_image_path -> Nullable<Text>,
        default_image_sha256 -> Nullable<Text>,
    }
}

diesel::allow_tables_to_appear_in_same_query!(photo_roster, photo_workspace,);
