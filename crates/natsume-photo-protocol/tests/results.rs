use ed25519_dalek::{Signature, Signer as _, SigningKey};
use natsume_photo_protocol::results::{
    ManifestId, PhotoFormat, RESULT_IMAGE_LIMIT, RESULT_IMAGES_LIMIT, ResultError, ResultId,
    ResultsManifest, SIGNING_DOMAIN, SignedResults, SubmissionId,
};
use natsume_photo_protocol::{MANIFEST_LIMIT, Sha256Digest, SigningPublicKey, TEAM_LIMIT};
use snafu::{ResultExt as _, Snafu};

#[derive(Debug, Snafu)]
enum Error {
    Json {
        source: serde_json::Error,
    },
    Contract {
        source: ResultError,
    },
    Hex {
        source: hex::FromHexError,
    },
    Value {
        source: natsume_photo_protocol::ValueError,
    },
    Signature {
        source: ed25519_dalek::SignatureError,
    },
}
type Result<T = ()> = std::result::Result<T, Error>;

fn manifest() -> Result<ResultsManifest> {
    serde_json::from_slice(include_bytes!("fixtures/results-v1.manifest.json")).context(JsonSnafu)
}
fn key() -> Result<SigningKey> {
    let mut seed = [0; 32];
    // Public RFC 8032 section 7.1 TEST 1 fixture, never an application credential.
    hex::decode_to_slice(
        "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60",
        &mut seed,
    )
    .context(HexSnafu)?;
    Ok(SigningKey::from_bytes(&seed))
}

#[test]
fn frozen_python_cryptography_vector_matches_exact_jcs_bytes_and_signature() -> Result {
    let manifest = manifest()?;
    let mut expected = SIGNING_DOMAIN.to_vec();
    expected.extend_from_slice(include_bytes!("fixtures/results-v1.manifest.json"));
    assert_eq!(manifest.signing_bytes().context(ContractSnafu)?, expected);
    let signed = manifest.sign(&key()?).context(ContractSnafu)?;
    assert_eq!(
        signed.signature.as_str(),
        include_str!("fixtures/results-v1.signature.hex")
    );
    assert_eq!(
        signed.to_json().context(ContractSnafu)?,
        include_bytes!("fixtures/results-v1.signed.json")
    );
    assert_eq!(
        SignedResults::parse_and_verify(
            include_bytes!("fixtures/results-v1.signed.json"),
            &manifest.context,
            &manifest.signing_public_key
        )
        .context(ContractSnafu)?,
        signed
    );
    assert_eq!(
        manifest.results[0].image_entry(),
        format!("images/{}.png", manifest.results[0].image_sha256)
    );
    Ok(())
}

#[test]
fn rfc8032_test_one_is_verified_independently_of_our_manifest_serialization() -> Result {
    let key = key()?;
    let mut expected = [0; 64];
    hex::decode_to_slice(
        concat!(
            "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e06522490155",
            "5fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b"
        ),
        &mut expected,
    )
    .context(HexSnafu)?;
    assert_eq!(key.sign(b"").to_bytes(), expected);
    key.verifying_key()
        .verify_strict(b"", &Signature::from_bytes(&expected))
        .context(SignatureSnafu)
}

#[test]
fn rfc8785_utf16_property_order_and_number_serialization_vectors() -> Result {
    let value = serde_json::json!({"\u{fb33}":"Hebrew", "😀":"Emoji", "€":"Euro", "ö":"Latin", "\u{80}":"Control", "1":"One", "\r":"Return"});
    let expected = "{\"\\r\":\"Return\",\"1\":\"One\",\"\u{80}\":\"Control\",\"ö\":\"Latin\",\"€\":\"Euro\",\"😀\":\"Emoji\",\"\u{fb33}\":\"Hebrew\"}";
    assert_eq!(
        serde_json_canonicalizer::to_vec(&value).context(JsonSnafu)?,
        expected.as_bytes()
    );
    let value: serde_json::Value =
        serde_json::from_str("[333333333.33333329,1E30,4.50,2e-3,0.000000000000000000000000001]")
            .context(JsonSnafu)?;
    assert_eq!(
        serde_json_canonicalizer::to_vec(&value).context(JsonSnafu)?,
        b"[333333333.3333333,1e+30,4.5,0.002,1e-27]"
    );
    Ok(())
}

#[test]
fn json_whitespace_property_order_and_unicode_escaping_do_not_change_signature() -> Result {
    let manifest = manifest()?;
    let signed = manifest.sign(&key()?).context(ContractSnafu)?;
    let pretty = serde_json::to_string_pretty(&signed)
        .context(JsonSnafu)?
        .replace("team-α", r"team-\u03b1");
    assert_eq!(
        SignedResults::parse_and_verify(
            pretty.as_bytes(),
            &manifest.context,
            &manifest.signing_public_key
        )
        .context(ContractSnafu)?,
        signed
    );
    Ok(())
}

#[test]
fn duplicate_unknown_and_unsupported_fields_are_rejected_at_every_level() -> Result {
    let manifest = manifest()?;
    let raw = include_str!("fixtures/results-v1.signed.json");
    let cases = [
        raw.replacen(
            "\"format_version\":1",
            "\"format_version\":1,\"format_version\":1",
            1,
        ),
        raw.replacen(
            "\"source_id\":",
            &format!(
                "\"source_id\":\"{}\",\"source_id\":",
                manifest.context.source_id
            ),
            1,
        ),
        raw.replacen("\"account\":", "\"account\":\"another\",\"account\":", 1),
        raw.replacen(
            "\"signature\":",
            &format!(
                "\"signature\":\"{}\",\"signature\":",
                include_str!("fixtures/results-v1.signature.hex")
            ),
            1,
        ),
        raw.replacen('{', "{\"unknown\":0,", 1),
        raw.replacen(
            "\"format_version\":1",
            "\"unknown\":0,\"format_version\":1",
            1,
        ),
        raw.replacen("\"source_id\":", "\"unknown\":0,\"source_id\":", 1),
        raw.replacen("\"account\":", "\"unknown\":0,\"account\":", 1),
        raw.replace("\"format_version\":1", "\"format_version\":2"),
        raw.replace("team_results", "default_image"),
        raw.replace("\"image_bytes\":123", "\"image_bytes\":1.23e2"),
        raw.replace("\"png\"", "\"gif\""),
    ];
    for input in cases {
        assert!(
            SignedResults::parse_and_verify(
                input.as_bytes(),
                &manifest.context,
                &manifest.signing_public_key
            )
            .is_err()
        );
    }
    Ok(())
}

#[test]
fn every_context_field_and_the_registered_key_are_external_authorization_requirements() -> Result {
    let manifest = manifest()?;
    let mut contexts = vec![manifest.context.clone(); 4];
    contexts[0].source_id = natsume_photo_protocol::SourceId::generate();
    contexts[1].dataset_id = natsume_photo_protocol::DatasetId::generate();
    contexts[2].initialization_id = natsume_photo_protocol::InitializationId::generate();
    contexts[3].initialization_sha256 = Sha256Digest::of(b"other initialization");
    for context in contexts {
        let mut changed = manifest.clone();
        changed.context = context;
        // Even a valid signature cannot substitute a different initialization.
        let signed = changed.sign(&key()?).context(ContractSnafu)?;
        assert!(matches!(
            signed.verify(&manifest.context, &manifest.signing_public_key),
            Err(ResultError::Authorization)
        ));
    }
    let other_key = SigningKey::from_bytes(&[7; 32]);
    assert!(matches!(
        manifest.sign(&other_key),
        Err(ResultError::Authorization)
    ));
    let mut other = manifest.clone();
    other.signing_public_key = hex::encode(other_key.verifying_key().to_bytes())
        .parse::<SigningPublicKey>()
        .context(ValueSnafu)?;
    let signed = other.sign(&other_key).context(ContractSnafu)?;
    assert!(matches!(
        signed.verify(&manifest.context, &manifest.signing_public_key),
        Err(ResultError::Authorization)
    ));
    Ok(())
}

#[test]
fn each_result_field_and_manifest_identity_are_covered_by_the_signature() -> Result {
    let manifest = manifest()?;
    let signed = manifest.sign(&key()?).context(ContractSnafu)?;
    let mut variants = vec![signed.clone(); 11];
    variants[0].manifest.manifest_id = ManifestId::generate();
    variants[1].manifest.results[0].result_id = ResultId::generate();
    variants[2].manifest.results[0].submission_id = SubmissionId::generate();
    variants[3].manifest.results[0].account_id.push('x');
    variants[4].manifest.results[0].account.push('x');
    variants[5].manifest.results[0].image_sha256 = Sha256Digest::of(b"changed image");
    variants[6].manifest.results[0].image_bytes += 1;
    variants[7].manifest.results[0].image_format = PhotoFormat::Jpeg;
    variants[8].manifest.results[0].width += 1;
    variants[9].manifest.results[0].height += 1;
    variants[10].signature = "00".repeat(64).try_into().context(ContractSnafu)?;
    for variant in variants {
        assert!(
            variant
                .verify(&manifest.context, &manifest.signing_public_key)
                .is_err()
        );
    }
    // Signing without the versioned domain is not accepted either.
    let mut no_domain = signed;
    no_domain.signature = hex::encode(
        key()?
            .sign(include_bytes!("fixtures/results-v1.manifest.json"))
            .to_bytes(),
    )
    .try_into()
    .context(ContractSnafu)?;
    assert!(
        no_domain
            .verify(&manifest.context, &manifest.signing_public_key)
            .is_err()
    );
    Ok(())
}

fn two_results() -> Result<ResultsManifest> {
    let mut manifest = manifest()?;
    let mut second = manifest.results[0].clone();
    second.account_id = "account-B".into();
    second.account = "team-B".into();
    second.result_id = ResultId::generate();
    second.submission_id = SubmissionId::generate();
    manifest.results.push(second);
    Ok(manifest)
}

#[test]
fn duplicate_identities_unsorted_arrays_and_inconsistent_shared_images_are_rejected() -> Result {
    let manifest = two_results()?;
    manifest.validate().context(ContractSnafu)?;
    let mut variants = vec![manifest.clone(); 9];
    variants[0].results.swap(0, 1);
    variants[1].results[1].account_id = manifest.results[0].account_id.clone();
    variants[2].results[1].account = manifest.results[0].account.clone();
    variants[3].results[1].result_id = manifest.results[0].result_id.clone();
    variants[4].results[1].submission_id = manifest.results[0].submission_id.clone();
    variants[5].results[1].image_bytes += 1;
    variants[6].results[1].image_format = PhotoFormat::Jpeg;
    variants[7].results[1].width += 1;
    variants[8].results[1].height += 1;
    for variant in variants {
        assert!(variant.validate().is_err());
    }
    Ok(())
}

#[test]
fn result_count_individual_image_and_pixel_bounds_are_enforced() -> Result {
    let manifest = manifest()?;
    let mut empty = manifest.clone();
    empty.results.clear();
    assert!(matches!(empty.validate(), Err(ResultError::Count)));
    empty.results = vec![manifest.results[0].clone(); TEAM_LIMIT + 1];
    assert!(matches!(empty.validate(), Err(ResultError::Count)));
    let mut variants = vec![manifest; 8];
    variants[0].results[0].image_bytes = 0;
    variants[1].results[0].image_bytes = RESULT_IMAGE_LIMIT + 1;
    variants[2].results[0].width = 0;
    variants[3].results[0].height = 0;
    variants[4].results[0].width = 80_000_001;
    variants[4].results[0].height = 1;
    variants[5].results[0].width = u32::MAX;
    variants[5].results[0].height = u32::MAX;
    variants[6].results[0].account_id = " ".into();
    variants[7].results[0].account = "\n".into();
    for variant in variants {
        assert!(variant.validate().is_err());
    }
    Ok(())
}

#[test]
fn package_budget_counts_distinct_images_and_allows_2000_shared_references() -> Result {
    let mut manifest = manifest()?;
    let template = manifest.results[0].clone();
    manifest.results = (0..TEAM_LIMIT)
        .map(|index| {
            let mut entry = template.clone();
            entry.account_id = format!("account-{index:04}");
            entry.account = format!("team-{index:04}");
            entry.result_id = ResultId::generate();
            entry.submission_id = SubmissionId::generate();
            entry.image_bytes = RESULT_IMAGE_LIMIT;
            entry
        })
        .collect();
    manifest.sign(&key()?).context(ContractSnafu)?;
    for (index, entry) in manifest.results.iter_mut().enumerate() {
        entry.image_sha256 = Sha256Digest::of(&index.to_be_bytes());
    }
    let count = usize::try_from(RESULT_IMAGES_LIMIT / RESULT_IMAGE_LIMIT)
        .unwrap_or_else(|_| panic!("budget count fits usize"));
    manifest.results.truncate(count);
    manifest.validate().context(ContractSnafu)?;
    let mut overflow = template;
    overflow.account_id = "z-last".into();
    overflow.account = "z-last".into();
    overflow.image_sha256 = Sha256Digest::of(b"last");
    overflow.image_bytes = RESULT_IMAGE_LIMIT;
    manifest.results.push(overflow);
    assert!(matches!(manifest.validate(), Err(ResultError::Image)));
    Ok(())
}

#[test]
fn input_json_and_generated_envelope_are_bounded_and_bad_signature_encodings_are_rejected() -> Result
{
    let manifest = manifest()?;
    assert!(matches!(
        SignedResults::parse_and_verify(
            &vec![b' '; MANIFEST_LIMIT + 1],
            &manifest.context,
            &manifest.signing_public_key
        ),
        Err(ResultError::ManifestLimit)
    ));
    let raw = include_str!("fixtures/results-v1.signed.json");
    let signature = include_str!("fixtures/results-v1.signature.hex");
    for invalid in ["ab".to_owned(), signature.to_uppercase(), "g".repeat(128)] {
        assert!(
            SignedResults::parse_and_verify(
                raw.replace(signature, &invalid).as_bytes(),
                &manifest.context,
                &manifest.signing_public_key
            )
            .is_err()
        );
    }
    let mut huge = manifest;
    huge.results[0].account = "\u{1}".repeat(MANIFEST_LIMIT / 6 + 1);
    assert!(matches!(
        huge.sign(&key()?),
        Err(ResultError::ManifestLimit)
    ));
    Ok(())
}
