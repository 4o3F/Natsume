use natsume_photo_protocol::{PhotoInitialization, PhotoLogo, PhotoTeam, credential_parts};
use snafu::{ResultExt as _, Snafu};

#[derive(Debug, Snafu)]
enum Error {
    Json {
        source: serde_json::Error,
    },
    Contract {
        source: natsume_photo_protocol::ValidationError,
    },
}
type Result<T = ()> = std::result::Result<T, Error>;

fn manifest() -> PhotoInitialization {
    PhotoInitialization {
        format_version: 1,
        source_id: "01900000-0000-7000-8000-000000000001".into(),
        dataset_id: "01900000-0000-7000-8000-000000000002".into(),
        initialization_id: "01900000-0000-7000-8000-000000000003".into(),
        teams: vec![PhotoTeam {
            account_id: "account-id".into(),
            account: "team-1".into(),
            organization_id: 7,
            school_name_zh: "学校".into(),
            school_name_en: String::new(),
            team_name_zh: "队伍".into(),
            team_name_en: String::new(),
        }],
        school_logos: vec![PhotoLogo {
            organization_id: 7,
            sha256: "ab".repeat(32),
            content_type: "image/png".into(),
            size_bytes: 10,
        }],
    }
}

#[test]
fn shared_snapshot_round_trips_and_rejects_unknown_fields() -> Result {
    let manifest = manifest();
    let bytes = serde_json::to_vec(&manifest).context(JsonSnafu)?;
    let decoded: PhotoInitialization = serde_json::from_slice(&bytes).context(JsonSnafu)?;
    decoded
        .validate(&manifest.source_id)
        .context(ContractSnafu)?;
    let mut value = serde_json::to_value(&manifest).context(JsonSnafu)?;
    value["teams"][0]["password"] = serde_json::json!("forbidden");
    assert!(serde_json::from_value::<PhotoInitialization>(value).is_err());
    Ok(())
}

#[test]
fn invalid_roster_cannot_be_published_or_installed() {
    for identity in ["", " ", "\t"] {
        let mut manifest = manifest();
        manifest.teams[0].account = identity.into();
        assert!(manifest.validate(&manifest.source_id).is_err());
    }
    let mut manifest = manifest();
    manifest.teams.push(manifest.teams[0].clone());
    assert!(manifest.validate(&manifest.source_id).is_err());
}

#[test]
fn logo_relationships_and_sizes_are_validated_at_both_ends() {
    let mut manifest = manifest();
    manifest.school_logos[0].organization_id = 8;
    assert!(manifest.validate(&manifest.source_id).is_err());
    manifest.school_logos[0].organization_id = 7;
    manifest.school_logos[0].size_bytes = natsume_photo_protocol::IMAGE_LIMIT as u64 + 1;
    assert!(manifest.validate(&manifest.source_id).is_err());
}

#[test]
fn credential_format_requires_canonical_v7_ids_and_lowercase_hashes() {
    let manifest = manifest();
    let token = format!(
        "nph1.{}.{}.{}",
        manifest.source_id,
        manifest.initialization_id,
        "ab".repeat(32)
    );
    assert_eq!(
        credential_parts(&token),
        Some((
            manifest.source_id.as_str(),
            manifest.initialization_id.as_str()
        ))
    );
    assert!(credential_parts(&token.replace("abab", "ABAB")).is_none());
    assert!(credential_parts(&token.replace("7000", "4000")).is_none());
}
