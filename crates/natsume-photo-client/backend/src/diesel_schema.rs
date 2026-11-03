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
    photo_team_results (account_id) {
        account_id -> Text,
        confirmation_id -> Text,
        result_id -> Text,
        submission_id -> Text,
        image_path -> Text,
        image_sha256 -> Text,
        image_bytes -> BigInt,
        image_format -> Text,
        width -> Integer,
        height -> Integer,
        revision -> BigInt,
    }
}

diesel::table! {
    photo_workspace (singleton) {
        singleton -> Integer,
        format_version -> Integer,
        ready -> Integer,
    }
}

diesel::allow_tables_to_appear_in_same_query!(photo_roster, photo_team_results, photo_workspace,);
