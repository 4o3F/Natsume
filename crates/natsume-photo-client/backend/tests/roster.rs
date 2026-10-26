use natsume_photo_client::roster::{Error as RosterError, Roster};
use serde_json::{Value, json};
use snafu::{OptionExt as _, ResultExt as _, Snafu};

const FIXTURE: &[u8] = include_bytes!("fixtures/roster.json");

#[derive(Debug, Snafu)]
enum Error {
    Roster { source: RosterError },
    Json { source: serde_json::Error },
    Io { source: std::io::Error },
    Fixture,
}
type Result<T = ()> = std::result::Result<T, Error>;

fn fixture() -> Result<Value> {
    serde_json::from_slice(FIXTURE).context(JsonSnafu)
}

#[test]
fn maps_two_teams_without_confusing_internal_and_external_identity() -> Result {
    let roster = Roster::parse(FIXTURE).context(RosterSnafu)?;
    let teams = roster.teams();
    assert_eq!(teams.len(), 2);
    assert_eq!(teams[0].account_id, "0199a000-0000-7000-8000-000000000001");
    assert_eq!(teams[0].account, "T-001");
    assert_eq!(teams[0].organization_id, 1);
    assert_eq!(teams[0].school_name_zh, "测试大学");
    assert_eq!(teams[0].school_name_en, "Test University");
    assert_eq!(teams[0].team_name_zh, "测试一队");
    assert_eq!(teams[0].team_name_en, "Test Team One");
    assert_eq!(teams[1].account, "T-002");
    assert!(teams[1].school_name_en.is_empty());
    assert!(teams[1].team_name_en.is_empty());
    Ok(())
}

#[test]
fn missing_optional_english_names_default_to_empty() -> Result {
    let mut input = fixture()?;
    let first = input[0].as_object_mut().context(FixtureSnafu)?;
    first.remove("school_name_en");
    first.remove("team_name_en");
    let bytes = serde_json::to_vec(&input).context(JsonSnafu)?;
    let roster = Roster::parse(&bytes).context(RosterSnafu)?;
    assert!(roster.teams()[0].school_name_en.is_empty());
    assert!(roster.teams()[0].team_name_en.is_empty());
    Ok(())
}

#[test]
fn rejects_empty_roster() {
    assert!(matches!(Roster::parse(b"[]"), Err(RosterError::Empty)));
}

#[test]
fn rejects_empty_or_whitespace_identity() -> Result {
    for field in ["account_id", "account"] {
        for value in ["", " \t\n"] {
            let mut input = fixture()?;
            input[0][field] = json!(value);
            let bytes = serde_json::to_vec(&input).context(JsonSnafu)?;
            assert!(
                matches!(Roster::parse(&bytes), Err(RosterError::MissingIdentity { row: 1, field: actual }) if actual == field)
            );
        }
    }
    Ok(())
}

#[test]
fn rejects_duplicate_internal_or_external_identity() -> Result {
    for field in ["account_id", "account"] {
        let mut input = fixture()?;
        input[1][field] = input[0][field].clone();
        let bytes = serde_json::to_vec(&input).context(JsonSnafu)?;
        assert!(
            matches!(Roster::parse(&bytes), Err(RosterError::DuplicateIdentity { row: 2, field: actual }) if actual == field)
        );
    }
    Ok(())
}

#[test]
fn rejects_credentials_seats_and_unknown_fields() -> Result {
    for field in [
        "password",
        "seat_id",
        "seat_code",
        "credential_revision",
        "unexpected",
    ] {
        let mut input = fixture()?;
        input[0][field] = json!("must-not-be-imported");
        let bytes = serde_json::to_vec(&input).context(JsonSnafu)?;
        assert!(matches!(
            Roster::parse(&bytes),
            Err(RosterError::Json { .. })
        ));
    }
    Ok(())
}

#[test]
fn rejects_invalid_json_missing_required_fields_and_wrong_types() -> Result {
    for bytes in [b"[".as_slice(), b"{}", b"null", b"[{}]"] {
        assert!(matches!(
            Roster::parse(bytes),
            Err(RosterError::Json { .. })
        ));
    }
    let mut input = fixture()?;
    input[0]["organization_id"] = json!("not-an-integer");
    let bytes = serde_json::to_vec(&input).context(JsonSnafu)?;
    assert!(matches!(
        Roster::parse(&bytes),
        Err(RosterError::Json { .. })
    ));
    Ok(())
}

#[test]
fn file_loading_handles_unicode_paths_and_preserves_input() -> Result {
    let directory = tempfile::tempdir().context(IoSnafu)?;
    let path = directory.path().join("测试 名单.json");
    std::fs::write(&path, FIXTURE).context(IoSnafu)?;
    let roster = Roster::load(&path).context(RosterSnafu)?;
    assert_eq!(roster.teams().len(), 2);
    assert_eq!(std::fs::read(&path).context(IoSnafu)?, FIXTURE);
    assert!(matches!(
        Roster::load(&directory.path().join("missing.json")),
        Err(RosterError::Read { .. })
    ));
    Ok(())
}
