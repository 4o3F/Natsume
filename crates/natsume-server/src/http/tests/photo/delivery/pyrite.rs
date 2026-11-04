//! The actual Pyrite CDP naming rules, formats, and missing-resource semantics.
use super::*;
use natsume_photo_protocol::results::PhotoFormat;
use std::collections::BTreeMap;

mod benchmark;

async fn export_pyrite(f: &Fixture, cookie: &str) -> Result<Captured, TestError> {
    let response = f
        .app
        .clone()
        .oneshot(
            Request::post("/api/v2/photo/exports/pyrite")
                .header(header::COOKIE, cookie)
                .body(Body::empty())?,
        )
        .await?;
    capture(response).await
}

fn files(response: Captured) -> Result<BTreeMap<String, Vec<u8>>, TestError> {
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.headers[header::CONTENT_TYPE], "application/zip");
    assert_eq!(response.headers[header::CACHE_CONTROL], "no-store");
    assert_eq!(
        response.headers[header::CONTENT_DISPOSITION],
        "attachment; filename=\"pyrite-assets.zip\""
    );
    Ok(archive(response.body)?.into_iter().collect())
}

#[tokio::test]
async fn pyrite_export_has_complete_roster_and_explicit_missing_assets() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let (f, cookie) = admin().await?;
    // Use a different valid encoding so a decode/re-encode would change the bytes.
    let logo = image::RgbaImage::from_pixel(8, 4, image::Rgba([255, 0, 0, 255]));
    let mut png = Vec::new();
    image::ImageEncoder::write_image(
        image::codecs::png::PngEncoder::new_with_quality(
            &mut png,
            image::codecs::png::CompressionType::Fast,
            image::codecs::png::FilterType::NoFilter,
        ),
        &logo,
        8,
        4,
        image::ExtendedColorType::Rgba8,
    )?;
    std::fs::write(f.logos.path().join("School.png"), &png)?;
    assert_eq!(
        export_pyrite(&f, "").await?.status,
        StatusCode::UNAUTHORIZED
    );
    let output = files(export_pyrite(&f, &cookie).await?)?;
    assert_eq!(
        output.keys().map(String::as_str).collect::<Vec<_>>(),
        [
            "affiliations/",
            "affiliations/INST-007.png",
            "config.toml",
            "roster.json",
            "teams/"
        ]
    );
    let roster: Value = serde_json::from_slice(&output["roster.json"])?;
    assert_eq!(roster["format_version"], 1);
    assert_eq!(roster["teams"].as_array().map(Vec::len), Some(2));
    assert_eq!(
        roster["teams"][0],
        json!({
            "id":"account-a", "organization_id":"INST-007",
            "school_name_zh":"学校", "school_name_en":"School",
            "team_name_zh":"队伍", "team_name_en":"Team",
            "photo_path":null, "logo_path":"affiliations/INST-007.png"
        })
    );
    assert!(!output.contains_key("accounts.yaml"));
    assert!(!output.contains_key("event-feed.ndjson"));
    image::load_from_memory_with_format(
        &output["affiliations/INST-007.png"],
        image::ImageFormat::Png,
    )?;
    assert_eq!(output["affiliations/INST-007.png"], png);
    assert_eq!(
        output["config.toml"],
        b"[presentation]\nlogo_extension = \"png\"\nteam_photo_extension = \"jpg\"\n"
    );

    std::fs::remove_file(f.logos.path().join("School.png"))?;
    let missing = files(export_pyrite(&f, &cookie).await?)?;
    let roster: Value = serde_json::from_slice(&missing["roster.json"])?;
    assert!(roster["teams"][0]["logo_path"].is_null());
    std::fs::write(f.logos.path().join("School.png"), b"corrupt")?;
    assert_eq!(
        export_pyrite(&f, &cookie).await?.status,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    Ok(())
}

#[tokio::test]
async fn pyrite_export_converts_non_png_logos_and_omits_ambiguous_matches() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let (f, cookie) = admin().await?;
    std::fs::remove_file(f.logos.path().join("School.png"))?;
    image::RgbImage::from_pixel(8, 4, image::Rgb([255, 0, 0]))
        .save(f.logos.path().join("School.jpg"))?;
    let output = files(export_pyrite(&f, &cookie).await?)?;
    let converted = image::load_from_memory_with_format(
        &output["affiliations/INST-007.png"],
        image::ImageFormat::Png,
    )?;
    assert_eq!((converted.width(), converted.height()), (8, 4));
    std::fs::write(f.logos.path().join("School.svg"), b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"8\" height=\"4\"><rect width=\"8\" height=\"4\" fill=\"red\"/></svg>")?;
    let ambiguous = files(export_pyrite(&f, &cookie).await?)?;
    assert!(!ambiguous.contains_key("affiliations/INST-007.png"));
    std::fs::remove_file(f.logos.path().join("School.jpg"))?;
    let output = files(export_pyrite(&f, &cookie).await?)?;
    let converted = image::load_from_memory_with_format(
        &output["affiliations/INST-007.png"],
        image::ImageFormat::Png,
    )?;
    assert_eq!((converted.width(), converted.height()), (8, 4));
    Ok(())
}

#[tokio::test]
async fn pyrite_export_converts_png_on_white_and_preserves_jpeg_bytes() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let (mut f, cookie) = admin().await?;
    let (signed, _) = fixture_package(&f).await?;
    let mut png = Cursor::new(Vec::new());
    image::RgbaImage::from_pixel(8, 4, image::Rgba([20, 40, 80, 0]))
        .write_to(&mut png, image::ImageFormat::Png)?;
    let png = png.into_inner();
    let mut manifest = signed.manifest;
    manifest.results[0].image_sha256 = Sha256Digest::of(&png);
    manifest.results[0].image_bytes = u64::try_from(png.len())?;
    let key = ed25519_dalek::SigningKey::from_bytes(&[17; 32]);
    let signed = manifest.clone().sign(&key)?;
    let running = background(&mut f).await?;
    let id = ready(&f, &cookie, &signed, &png).await?;
    commit(&f, &cookie, id).await?;

    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 95)
        .encode_image(&image::RgbImage::from_pixel(8, 4, image::Rgb([20, 40, 80])))?;
    manifest.manifest_id = ManifestId::generate();
    let result = &mut manifest.results[0];
    result.account_id = "account-b".into();
    result.account = "account-b".into();
    result.result_id = ResultId::generate();
    result.submission_id = SubmissionId::generate();
    result.image_format = PhotoFormat::Jpeg;
    result.image_sha256 = Sha256Digest::of(&jpeg);
    result.image_bytes = u64::try_from(jpeg.len())?;
    let signed = manifest.sign(&key)?;
    let id = ready(&f, &cookie, &signed, &jpeg).await?;
    commit(&f, &cookie, id).await?;
    rename(&f, "account-a", "team001").await?;
    let output = files(export_pyrite(&f, &cookie).await?)?;
    assert_eq!(output["teams/account-b.jpg"], jpeg);
    let converted = image::load_from_memory_with_format(
        &output["teams/team001.jpg"],
        image::ImageFormat::Jpeg,
    )?
    .into_rgb8();
    assert_eq!(converted.dimensions(), (8, 4));
    assert!(
        converted
            .pixels()
            .all(|pixel| pixel.0.into_iter().all(|channel| channel >= 250))
    );
    let roster: Value = serde_json::from_slice(&output["roster.json"])?;
    assert_eq!(roster["teams"][0]["photo_path"], "teams/team001.jpg");
    assert_eq!(get(&f, &cookie, "photos/account-a/image").await?.body, png);
    // A corrupt stored object fails the whole package; no partial success is published.
    let path = f
        .database
        .documents_path()
        .with_extension("photo-results")
        .join("objects")
        .join(format!("{}.jpg", signed.manifest.results[0].image_sha256));
    std::fs::write(&path, vec![0_u8; jpeg.len()])?;
    assert_eq!(
        export_pyrite(&f, &cookie).await?.status,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    std::fs::write(&path, &jpeg)?;
    rename(&f, "account-a", "ACCOUNT-B").await?;
    assert_eq!(
        export_pyrite(&f, &cookie).await?.status,
        StatusCode::CONFLICT
    );
    running.shutdown().await?;
    Ok(())
}
