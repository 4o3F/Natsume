//! Real HTTP uploads, production background ownership, signatures and immutable files.
use super::super::PasswordVerificationTestGuard;
use super::imports::{admin, upload};
use super::*;
use crate::{
    background::{BackgroundTasks, start_background_tasks},
    diesel_schema::{photo_import_jobs as jobs, photo_submissions as submissions},
};
use natsume_photo_protocol::{
    FormatVersion, PhotoInitialization, Sha256Digest,
    results::{
        ManifestId, ManifestKind, PhotoFormat, ResultContext, ResultId, ResultsManifest,
        SignedResults, SubmissionId, TeamResult,
    },
};
use std::{
    fs,
    io::{Cursor, Write as _},
    os::unix::fs::OpenOptionsExt as _,
    sync::Arc,
    time::Duration,
};
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

pub(super) async fn fixture_package(f: &Fixture) -> Result<(SignedResults, Vec<u8>), TestError> {
    f.confirm().await?;
    let response = f
        .send(Method::POST, "client/initialization", Value::Null)
        .await?;
    assert_eq!(response.status, StatusCode::OK);
    let snapshot: PhotoInitialization = serde_json::from_slice(&response.body)?;
    let hash = Sha256Digest::of(&response.body);
    assert_eq!(f.send(Method::POST, "client/initialization/complete", json!({"initialization_id":snapshot.initialization_id, "manifest_sha256":hash, "request_id":Uuid::now_v7()})).await?.status, StatusCode::NO_CONTENT);
    let mut image = Cursor::new(Vec::new());
    image::RgbaImage::from_pixel(8, 4, image::Rgba([20, 40, 80, 255]))
        .write_to(&mut image, image::ImageFormat::Png)?;
    let image = image.into_inner();
    let team = &snapshot.teams[0];
    let manifest = ResultsManifest {
        format_version: FormatVersion::V1,
        kind: ManifestKind::TeamResults,
        manifest_id: ManifestId::generate(),
        context: ResultContext {
            source_id: snapshot.source_id,
            dataset_id: snapshot.dataset_id,
            initialization_id: snapshot.initialization_id,
            initialization_sha256: hash,
        },
        signing_public_key: f.key.clone(),
        results: vec![TeamResult {
            result_id: ResultId::generate(),
            submission_id: SubmissionId::generate(),
            account_id: team.account_id.clone(),
            account: team.account.clone(),
            image_sha256: Sha256Digest::of(&image),
            image_bytes: u64::try_from(image.len())
                .map_err(|_| TestError::missing("image fixture length"))?,
            image_format: PhotoFormat::Png,
            width: 8,
            height: 4,
        }],
    };
    Ok((
        manifest.sign(&ed25519_dalek::SigningKey::from_bytes(&[17; 32]))?,
        image,
    ))
}

pub(super) fn package(
    signed: &SignedResults,
    image: &[u8],
    compression: CompressionMethod,
    extra: Option<&str>,
) -> Result<Vec<u8>, TestError> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default()
        .compression_method(compression)
        .unix_permissions(0o600);
    writer.start_file("manifest.json", options)?;
    writer.write_all(&signed.to_json()?)?;
    writer.start_file(signed.manifest.results[0].image_entry(), options)?;
    writer.write_all(image)?;
    if let Some(name) = extra {
        writer.start_file(name, options)?;
        writer.write_all(b"unlisted")?;
    }
    Ok(writer.finish()?.into_inner())
}

pub(super) async fn background(f: &mut Fixture) -> Result<BackgroundTasks, TestError> {
    let features = feature::tests::ServerFixture::new(
        f.database.database.clone(),
        f.database.documents_path(),
    )
    .logos(f.logos.path().to_path_buf())
    .build_with_features()?;
    assert!(Arc::ptr_eq(
        features.photo.component(),
        &features.loaded.photo
    ));
    let running = start_background_tasks(&features.loaded.print)
        .await?
        .with_photo(features.loaded.photo.clone())
        .await?;
    f.app = features.loaded.http_router(unused_web_root());
    f.photo = features.photo;
    f.runtime = features.loaded.runtime;
    f.lifecycle = features.loaded.lifecycle;
    Ok(running)
}

pub(super) async fn submit(f: &Fixture, cookie: &str, bytes: Vec<u8>) -> Result<Uuid, TestError> {
    let response = drive(&f.app, upload(cookie, Body::from(bytes))?).await?;
    assert_eq!(response.status, StatusCode::ACCEPTED);
    let job: Value = serde_json::from_slice(&response.body)?;
    job["import_id"]
        .as_str()
        .ok_or(TestError::missing("import ID"))?
        .parse()
        .map_err(|_| TestError::missing("canonical import ID"))
}

pub(super) async fn terminal(f: &Fixture, cookie: &str, id: Uuid) -> Result<Value, TestError> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let response = drive(
                &f.app,
                Request::builder()
                    .uri(format!("/api/v2/photo/imports/{id}"))
                    .header(header::COOKIE, cookie)
                    .body(Body::empty())?,
            )
            .await?;
            assert_eq!(response.status, StatusCode::OK);
            let job: Value = serde_json::from_slice(&response.body)?;
            if job["state"] == "ready" || job["state"] == "failed" {
                return Ok(job);
            }
            tokio::task::yield_now().await;
        }
    })
    .await?
}

#[tokio::test]
async fn valid_packages_reach_ready_without_acceptance_and_repeated_manifest_is_idempotent()
-> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let (mut f, cookie) = admin().await?;
    let (signed, image) = fixture_package(&f).await?;
    let running = background(&mut f).await?;
    for method in [CompressionMethod::Stored, CompressionMethod::Deflated] {
        let id = submit(&f, &cookie, package(&signed, &image, method, None)?).await?;
        let job = terminal(&f, &cookie, id).await?;
        assert_eq!(job["state"], "ready");
        assert_eq!(job["verified_images"], 1);
    }
    let count = f
        .database
        .database
        .read(|tx| {
            submissions::table
                .count()
                .get_result::<i64>(tx.connection())
                .map_err(PersistenceError::from)
        })
        .await
        .map_err(crate::db::TransactionError::into_error)?;
    assert_eq!(count, 0);
    let object = f
        .database
        .documents_path()
        .with_extension("photo-results")
        .join("objects")
        .join(format!("{}.png", signed.manifest.results[0].image_sha256));
    assert_eq!(fs::read(object)?, image);
    running.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn bad_signature_extra_paths_corrupt_images_and_manifest_reuse_do_not_publish_results()
-> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let (mut f, cookie) = admin().await?;
    let (signed, image) = fixture_package(&f).await?;
    let running = background(&mut f).await?;
    let mut foreign = signed.manifest.clone();
    let key = ed25519_dalek::SigningKey::from_bytes(&[19; 32]);
    foreign.signing_public_key = hex::encode(key.verifying_key().to_bytes())
        .parse()
        .map_err(|_| TestError::missing("fixture public key"))?;
    let truncated = &image[..33];
    let mut damaged = signed.manifest.clone();
    damaged.results[0].image_bytes = 33;
    damaged.results[0].image_sha256 = Sha256Digest::of(truncated);
    let damaged = damaged.sign(&ed25519_dalek::SigningKey::from_bytes(&[17; 32]))?;
    let mut invalid_signature = signed.clone();
    invalid_signature.signature = "00".repeat(64).try_into()?;
    let cases = [
        (
            package(&invalid_signature, &image, CompressionMethod::Stored, None)?,
            "signature",
        ),
        (
            package(
                &foreign.sign(&key)?,
                &image,
                CompressionMethod::Stored,
                None,
            )?,
            "authorization",
        ),
        (
            package(
                &signed,
                &image,
                CompressionMethod::Stored,
                Some("../escape.png"),
            )?,
            "archive",
        ),
        (
            package(&signed, b"invalid image", CompressionMethod::Stored, None)?,
            "image",
        ),
        (
            package(&damaged, truncated, CompressionMethod::Stored, None)?,
            "image",
        ),
    ];
    for (bytes, reason) in cases {
        let id = submit(&f, &cookie, bytes).await?;
        let job = terminal(&f, &cookie, id).await?;
        assert_eq!(job["state"], "failed");
        assert_eq!(job["failure_code"], reason);
    }
    let id = submit(
        &f,
        &cookie,
        package(&signed, &image, CompressionMethod::Stored, None)?,
    )
    .await?;
    assert_eq!(terminal(&f, &cookie, id).await?["state"], "ready");
    let mut changed = signed.manifest.clone();
    changed.results[0].result_id = ResultId::generate();
    let changed = changed.sign(&ed25519_dalek::SigningKey::from_bytes(&[17; 32]))?;
    let id = submit(
        &f,
        &cookie,
        package(&changed, &image, CompressionMethod::Stored, None)?,
    )
    .await?;
    assert_eq!(terminal(&f, &cookie, id).await?["failure_code"], "conflict");
    running.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn startup_recovers_interrupted_upload_and_revalidates_complete_package() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let (mut f, cookie) = admin().await?;
    let (signed, image) = fixture_package(&f).await?;
    let interrupted = Uuid::now_v7();
    f.database
        .database
        .write(move |tx| {
            diesel::insert_into(jobs::table)
                .values((
                    jobs::import_id.eq(interrupted.to_string()),
                    jobs::state.eq("uploading"),
                    jobs::uploaded_bytes.eq(3),
                    jobs::created_at_unix_ms.eq(1),
                    jobs::updated_at_unix_ms.eq(1),
                ))
                .execute(tx.connection())?;
            Ok::<_, PersistenceError>(())
        })
        .await
        .map_err(crate::db::TransactionError::into_error)?;
    let root = f.database.documents_path().with_extension("photo-results");
    let partial = root.join("uploads").join(format!("{interrupted}.zip"));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&partial)?;
    file.write_all(b"par")?;
    file.sync_all()?;
    let running = background(&mut f).await?;
    assert_eq!(terminal(&f, &cookie, interrupted).await?["state"], "failed");
    assert!(!partial.exists());
    running.shutdown().await?;
    let id = submit(
        &f,
        &cookie,
        package(&signed, &image, CompressionMethod::Stored, None)?,
    )
    .await?;
    // Simulate interruption after the durable validation claim, before publication.
    f.database
        .database
        .write(move |tx| {
            diesel::update(jobs::table.find(id.to_string()))
                .set(jobs::state.eq("processing"))
                .execute(tx.connection())
                .map_err(PersistenceError::from)
        })
        .await
        .map_err(crate::db::TransactionError::into_error)?;
    let running = background(&mut f).await?;
    assert_eq!(terminal(&f, &cookie, id).await?["state"], "ready");
    assert_eq!(fs::read_dir(root.join("staging"))?.count(), 0);
    running.shutdown().await?;
    Ok(())
}

fn zip64(mut bytes: Vec<u8>) -> Result<Vec<u8>, TestError> {
    let end = bytes.len() - 22;
    let number = |at: usize| -> Result<[u8; 4], TestError> {
        bytes[at..at + 4]
            .try_into()
            .map_err(|_| TestError::missing("ZIP footer"))
    };
    let start = u32::from_le_bytes(number(end + 16)?);
    let size = u32::from_le_bytes(number(end + 12)?);
    let count = u16::from_le_bytes([bytes[end + 10], bytes[end + 11]]);
    let mut footer = bytes[end..].to_vec();
    bytes.truncate(end);
    bytes.extend_from_slice(b"PK\x06\x06");
    bytes.extend_from_slice(&44_u64.to_le_bytes());
    bytes.extend_from_slice(&[45, 0, 45, 0]);
    bytes.extend_from_slice(&[0; 8]);
    bytes.extend_from_slice(&u64::from(count).to_le_bytes());
    bytes.extend_from_slice(&u64::from(count).to_le_bytes());
    bytes.extend_from_slice(&u64::from(size).to_le_bytes());
    bytes.extend_from_slice(&u64::from(start).to_le_bytes());
    bytes.extend_from_slice(b"PK\x06\x07");
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(
        &u64::try_from(end)
            .map_err(|_| TestError::missing("ZIP offset"))?
            .to_le_bytes(),
    );
    bytes.extend_from_slice(&1_u32.to_le_bytes());
    footer[8..12].fill(255);
    footer[12..20].fill(255);
    bytes.extend_from_slice(&footer);
    Ok(bytes)
}

#[tokio::test]
async fn zip64_is_supported_and_bad_directory_budgets_are_rejected() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let (mut f, cookie) = admin().await?;
    let (signed, image) = fixture_package(&f).await?;
    let running = background(&mut f).await?;
    let bytes = package(&signed, &image, CompressionMethod::Stored, None)?;
    let id = submit(&f, &cookie, zip64(bytes.clone())?).await?;
    assert_eq!(terminal(&f, &cookie, id).await?["state"], "ready");
    let end = bytes.len() - 22;
    for mode in 0..3 {
        let mut malformed = bytes.clone();
        match mode {
            0 => {
                malformed[end + 8..end + 10].copy_from_slice(&2002_u16.to_le_bytes());
                malformed[end + 10..end + 12].copy_from_slice(&2002_u16.to_le_bytes());
            }
            1 => malformed[end + 12..end + 16].copy_from_slice(&4_194_305_u32.to_le_bytes()),
            _ => malformed[end + 4] = 1,
        }
        let id = submit(&f, &cookie, malformed).await?;
        assert_eq!(terminal(&f, &cookie, id).await?["failure_code"], "archive");
    }
    running.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn queued_revoked_source_is_rejected_and_a_second_worker_cannot_take_ownership() -> TestResult
{
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let (mut f, cookie) = admin().await?;
    let (signed, image) = fixture_package(&f).await?;
    let id = submit(
        &f,
        &cookie,
        package(&signed, &image, CompressionMethod::Stored, None)?,
    )
    .await?;
    f.photo
        .component()
        .set_state(f.source.clone(), SourceState::Revoked)
        .await?;
    let running = background(&mut f).await?;
    assert_eq!(
        terminal(&f, &cookie, id).await?["failure_code"],
        "authorization"
    );
    let (ready, _receive) = tokio::sync::oneshot::channel();
    let duplicate = f
        .photo
        .component()
        .clone()
        .maintain_results(tokio_util::sync::CancellationToken::new(), ready)
        .await;
    assert!(matches!(
        duplicate,
        Err(crate::component::photo::ValidationError::Storage { .. })
    ));
    running.shutdown().await?;
    let count = f
        .database
        .database
        .read(|tx| {
            submissions::table
                .count()
                .get_result::<i64>(tx.connection())
                .map_err(PersistenceError::from)
        })
        .await
        .map_err(crate::db::TransactionError::into_error)?;
    assert_eq!(count, 0);
    Ok(())
}

#[tokio::test]
async fn missing_uploaded_file_is_failed_without_stopping_validation() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let (mut f, cookie) = admin().await?;
    let (signed, image) = fixture_package(&f).await?;
    let missing = Uuid::now_v7();
    f.database
        .database
        .write(move |tx| {
            diesel::insert_into(jobs::table)
                .values((
                    jobs::import_id.eq(missing.to_string()),
                    jobs::state.eq("processing"),
                    jobs::uploaded_bytes.eq(100),
                    jobs::created_at_unix_ms.eq(1),
                    jobs::updated_at_unix_ms.eq(1),
                ))
                .execute(tx.connection())
                .map_err(PersistenceError::from)
        })
        .await
        .map_err(crate::db::TransactionError::into_error)?;
    let running = background(&mut f).await?;
    assert_eq!(
        terminal(&f, &cookie, missing).await?["failure_code"],
        "storage"
    );
    let id = submit(
        &f,
        &cookie,
        package(&signed, &image, CompressionMethod::Stored, None)?,
    )
    .await?;
    assert_eq!(terminal(&f, &cookie, id).await?["state"], "ready");
    running.shutdown().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires NATSUME_PHOTO_RSS_IMAGE containing an 8000x10000 PNG; measure the test binary RSS separately"]
async fn one_eighty_megapixel_image_has_bounded_validation_memory() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let (mut f, cookie) = admin().await?;
    let (signed, _) = fixture_package(&f).await?;
    let image = fs::read(std::env::var("NATSUME_PHOTO_RSS_IMAGE")?)?;
    let mut manifest = signed.manifest;
    manifest.results[0].image_sha256 = Sha256Digest::of(&image);
    manifest.results[0].image_bytes =
        u64::try_from(image.len()).map_err(|_| TestError::missing("image size"))?;
    manifest.results[0].width = 8000;
    manifest.results[0].height = 10000;
    let signed = manifest.sign(&ed25519_dalek::SigningKey::from_bytes(&[17; 32]))?;
    let running = background(&mut f).await?;
    let id = submit(
        &f,
        &cookie,
        package(&signed, &image, CompressionMethod::Stored, None)?,
    )
    .await?;
    assert_eq!(terminal(&f, &cookie, id).await?["state"], "ready");
    running.shutdown().await?;
    Ok(())
}
