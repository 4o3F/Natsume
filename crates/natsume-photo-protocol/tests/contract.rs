use natsume_photo_protocol::{
    DatasetId, EnrollmentId, FormatVersion, InitializationId, LogoContentType,
    PhotoEnrollmentIssued, PhotoInitialization, PhotoLogo, PhotoTeam, Sha256Digest, SourceId,
};
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
        format_version: FormatVersion::V1,
        source_id: SourceId::generate(),
        dataset_id: DatasetId::generate(),
        initialization_id: InitializationId::generate(),
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
            sha256: Sha256Digest::of(b"example"),
            content_type: LogoContentType::Png,
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
fn token_is_opaque_to_client_contract_validation() -> Result {
    let issued = PhotoEnrollmentIssued {
        source_id: SourceId::generate(),
        enrollment_id: EnrollmentId::generate(),
        token: "a-server-private-format-v2".into(),
        expires_at_unix_ms: 1,
    };
    issued.validate().context(ContractSnafu)?;
    let bytes = serde_json::to_vec(&issued).context(JsonSnafu)?;
    let decoded: PhotoEnrollmentIssued = serde_json::from_slice(&bytes).context(JsonSnafu)?;
    assert_eq!(decoded.token, issued.token);
    Ok(())
}

#[test]
fn deserialization_rejects_invalid_identity_digest_format_and_version() -> Result {
    for (field, bad) in [
        ("source_id", serde_json::json!("not-a-uuid")),
        (
            "dataset_id",
            serde_json::json!("01900000-0000-4000-8000-000000000001"),
        ),
        ("initialization_id", serde_json::json!("../outside")),
        ("format_version", serde_json::json!(2)),
    ] {
        let mut value = serde_json::to_value(manifest()).context(JsonSnafu)?;
        value[field] = bad;
        assert!(
            serde_json::from_value::<PhotoInitialization>(value).is_err(),
            "{field}"
        );
    }
    for (field, bad) in [
        ("sha256", "AB".repeat(32)),
        ("content_type", "image/jpeg".into()),
    ] {
        let mut value = serde_json::to_value(manifest()).context(JsonSnafu)?;
        value["school_logos"][0][field] = serde_json::json!(bad);
        assert!(
            serde_json::from_value::<PhotoInitialization>(value).is_err(),
            "{field}"
        );
    }
    Ok(())
}

#[test]
fn enrollment_requests_and_confirmation_share_the_same_wire_types() -> Result {
    use natsume_photo_protocol::{
        InitializationRequestId, PhotoConfirmation, PhotoInitializationComplete, PhotoPairRequest,
        PhotoSource, SourceState,
    };
    let issued = PhotoEnrollmentIssued {
        source_id: SourceId::generate(),
        enrollment_id: EnrollmentId::generate(),
        token: "opaque".into(),
        expires_at_unix_ms: 1,
    };
    let pair = PhotoPairRequest {
        code: "one-time-code".into(),
        signing_public_key: hex::encode(
            ed25519_dalek::SigningKey::from_bytes(&[7; 32])
                .verifying_key()
                .to_bytes(),
        )
        .parse()
        .unwrap_or_else(|error| panic!("generated public key: {error:?}")),
        client_version: "test".into(),
    };
    let confirmation = PhotoConfirmation {
        enrollment_id: issued.enrollment_id.clone(),
        signing_public_key: pair.signing_public_key.clone(),
    };
    let decoded: PhotoConfirmation =
        serde_json::from_slice(&serde_json::to_vec(&confirmation).context(JsonSnafu)?)
            .context(JsonSnafu)?;
    assert_eq!(decoded.enrollment_id, issued.enrollment_id);
    let completion = PhotoInitializationComplete {
        initialization_id: InitializationId::generate(),
        manifest_sha256: Sha256Digest::of(b"snapshot"),
        request_id: InitializationRequestId::generate(),
    };
    let decoded: PhotoInitializationComplete =
        serde_json::from_slice(&serde_json::to_vec(&completion).context(JsonSnafu)?)
            .context(JsonSnafu)?;
    assert_eq!(decoded.manifest_sha256, completion.manifest_sha256);
    let source = PhotoSource {
        source_id: issued.source_id.clone(),
        name: "Desk".into(),
        state: SourceState::Enabled,
        enrollment_confirmed: true,
        initialization_completed: false,
    };
    let mut encoded = serde_json::to_value(source).context(JsonSnafu)?;
    encoded["state"] = serde_json::json!("unexpected");
    assert!(serde_json::from_value::<PhotoSource>(encoded).is_err());
    Ok(())
}

#[test]
fn public_keys_have_their_own_validation_and_wire_representation() {
    use natsume_photo_protocol::SigningPublicKey;
    let encoded = hex::encode(
        ed25519_dalek::SigningKey::from_bytes(&[11; 32])
            .verifying_key()
            .to_bytes(),
    );
    let key: SigningPublicKey = encoded
        .parse()
        .unwrap_or_else(|error| panic!("generated public key: {error:?}"));
    assert_eq!(
        serde_json::to_value(key).unwrap_or_else(|error| panic!("key serialization: {error:?}")),
        encoded
    );
    for invalid in [
        "00".repeat(32),
        "ab".repeat(31),
        encoded.to_uppercase(),
        "gg".repeat(32),
    ] {
        assert!(invalid.parse::<SigningPublicKey>().is_err());
    }
}
