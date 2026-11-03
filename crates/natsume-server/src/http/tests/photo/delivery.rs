//! Roster lifecycle, exact image bytes, complete ZIPs and download ownership.
use super::*;
use super::{
    acceptance::{call, commit, different, ready, value, visible_pending},
    imports::admin,
    validation::{background, fixture_package},
};
use crate::http::tests::PasswordVerificationTestGuard;
use natsume_photo_protocol::{
    Sha256Digest,
    results::{ManifestId, ResultId, SignedResults, SubmissionId},
};
use std::{
    io::{Cursor, Read as _},
    os::unix::fs::{PermissionsExt as _, symlink},
    time::Duration,
};
use tower::ServiceExt as _;

async fn get(f: &Fixture, cookie: &str, path: &str) -> Result<Captured, TestError> {
    Ok(drive(
        &f.app,
        Request::builder()
            .uri(format!("/api/v2/photo/{path}"))
            .header(header::COOKIE, cookie)
            .body(Body::empty())?,
    )
    .await?)
}
async fn export(f: &Fixture, cookie: &str) -> Result<Captured, TestError> {
    let response = f
        .app
        .clone()
        .oneshot(
            Request::post("/api/v2/photo/exports")
                .header(header::COOKIE, cookie)
                .body(Body::empty())?,
        )
        .await?;
    let status = response.status();
    let headers = response.headers().clone();
    let body = axum::body::to_bytes(response.into_body(), 2 * 1024 * 1024)
        .await?
        .to_vec();
    Ok(Captured {
        status,
        headers,
        body,
    })
}
async fn rename(f: &Fixture, id: &'static str, account: &'static str) -> TestResult {
    f.database
        .database
        .write(move |tx| {
            diesel::update(accounts::table.find(id))
                .set(accounts::domjudge_username.eq(account))
                .execute(tx.connection())
                .map_err(PersistenceError::from)
        })
        .await?;
    Ok(())
}
async fn activate(
    f: &Fixture,
    cookie: &str,
    id: Uuid,
    index: usize,
    target: &str,
    expected: Value,
) -> Result<Value, TestError> {
    let preview = value(
        call(
            f,
            cookie,
            Method::GET,
            &format!("{id}/preview"),
            Value::Null,
        )
        .await?,
    )?;
    let entry = &preview["entries"][index];
    value(call(f,cookie,Method::POST,&format!("{id}/decisions"),json!({"submission_id":entry["submission_id"],"expected_submission_revision":entry["submission_revision"],"decision":{"action":"activate","account_id":target,"expected_current":expected}})).await?)
}
fn expected(current: &Value) -> Value {
    json!({"source_id":current["source_id"],"submission_id":current["submission_id"]})
}
fn archive(bytes: Vec<u8>) -> Result<Vec<(String, Vec<u8>)>, TestError> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))?;
    let mut entries = Vec::new();
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index)?;
        let name = entry.name()?.into_owned();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        entries.push((name, bytes));
    }
    Ok(entries)
}

#[tokio::test]
async fn current_photos_keep_stable_identity_and_use_current_export_names() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let (mut f, cookie) = admin().await?;
    let (signed, image) = fixture_package(&f).await?;
    let running = background(&mut f).await?;
    let id = ready(&f, &cookie, &signed, &image).await?;
    commit(&f, &cookie, id).await?;
    assert_eq!(
        get(&f, &cookie, "photos/account-a/image").await?.body,
        image
    );
    rename(&f, "account-a", "renamed-a").await?;
    let listed = value(get(&f, &cookie, "photos").await?)?;
    assert_eq!(listed["entries"][0]["account"], "renamed-a");
    assert_eq!(
        archive(export(&f, &cookie).await?.body)?,
        vec![("renamed-a.png".into(), image)]
    );
    assert!(
        visible_pending(&f, &cookie, String::new()).await?["entries"]
            .as_array()
            .is_some_and(Vec::is_empty)
    );
    for query in ["photos?limit=0", "photos?limit=101", "photos?unexpected=1"] {
        assert_eq!(
            get(&f, &cookie, query).await?.status,
            StatusCode::BAD_REQUEST
        );
    }
    running.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn stale_target_identity_cannot_replace_a_newly_approved_photo() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let (mut f, cookie) = admin().await?;
    let (signed, image) = fixture_package(&f).await?;
    let running = background(&mut f).await?;
    let first = ready(&f, &cookie, &signed, &image).await?;
    let accepted = commit(&f, &cookie, first).await?;
    let old = expected(&accepted["entries"][0]["current"]);
    let (b, bytes) = different(&signed, 25)?;
    let second = ready(&f, &cookie, &b, &bytes).await?;
    commit(&f, &cookie, second).await?;
    let (c, bytes) = different(&signed, 26)?;
    let third = ready(&f, &cookie, &c, &bytes).await?;
    commit(&f, &cookie, third).await?;
    activate(&f, &cookie, second, 0, "account-a", old.clone()).await?;
    let request = json!({"submission_id":c.manifest.results[0].submission_id,"expected_submission_revision":1,"decision":{"action":"activate","account_id":"account-a","expected_current":old}});
    assert_eq!(
        call(
            &f,
            &cookie,
            Method::POST,
            &format!("{third}/decisions"),
            request
        )
        .await?
        .status,
        StatusCode::CONFLICT
    );
    assert_eq!(
        value(get(&f, &cookie, "photos").await?)?["entries"][0]["image_sha256"],
        json!(b.manifest.results[0].image_sha256)
    );
    running.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn recreated_team_does_not_inherit_old_photo_even_with_the_same_username() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let (mut f, cookie) = admin().await?;
    let (signed, image) = fixture_package(&f).await?;
    let running = background(&mut f).await?;
    let id = ready(&f, &cookie, &signed, &image).await?;
    commit(&f, &cookie, id).await?;
    f.database
        .database
        .write(|tx| -> Result<(), PersistenceError> {
            diesel::delete(accounts::table.find("account-a")).execute(tx.connection())?;
            diesel::insert_into(accounts::table)
                .values((
                    accounts::account_id.eq("account-rebuilt"),
                    accounts::domjudge_username.eq("account-a"),
                    accounts::credential_revision.eq(1_i64),
                ))
                .execute(tx.connection())?;
            diesel::insert_into(teams::table)
                .values((
                    teams::account_id.eq("account-rebuilt"),
                    teams::organization_id.eq(7_i64),
                    teams::name_zh.eq("重建队伍"),
                    teams::name_en.eq("Rebuilt"),
                    teams::category.eq("official"),
                ))
                .execute(tx.connection())?;
            diesel::insert_into(account_mappings::table)
                .values((
                    account_mappings::account_id.eq("account-rebuilt"),
                    account_mappings::seat_id.eq("seat-a"),
                ))
                .execute(tx.connection())?;
            Ok(())
        })
        .await?;
    assert_eq!(
        get(&f, &cookie, "photos/account-rebuilt/image")
            .await?
            .status,
        StatusCode::NOT_FOUND
    );
    assert!(archive(export(&f, &cookie).await?.body)?.is_empty());
    let associated = activate(&f, &cookie, id, 0, "account-rebuilt", Value::Null).await?;
    assert_eq!(associated["entries"][0]["account_id"], "account-a");
    assert_eq!(
        associated["entries"][0]["active_account_id"],
        "account-rebuilt"
    );
    assert_eq!(
        get(&f, &cookie, "photos/account-rebuilt/image").await?.body,
        image
    );
    running.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn missing_or_corrupt_objects_never_return_a_successful_image_or_partial_zip() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let (mut f, cookie) = admin().await?;
    let (signed, image) = fixture_package(&f).await?;
    let running = background(&mut f).await?;
    let id = ready(&f, &cookie, &signed, &image).await?;
    commit(&f, &cookie, id).await?;
    let path = f
        .database
        .documents_path()
        .with_extension("photo-results")
        .join("objects")
        .join(format!("{}.png", signed.manifest.results[0].image_sha256));
    std::fs::write(&path, vec![0_u8; image.len()])?;
    assert_eq!(
        export(&f, &cookie).await?.status,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(
        get(&f, &cookie, "photos/account-a/image").await?.status,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    std::fs::remove_file(&path)?;
    assert_eq!(
        export(&f, &cookie).await?.status,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    let outside = path.with_file_name("outside.png");
    std::fs::write(&outside, &image)?;
    std::fs::set_permissions(&outside, std::fs::Permissions::from_mode(0o600))?;
    symlink(&outside, &path)?;
    assert_eq!(
        export(&f, &cookie).await?.status,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(
        std::fs::read_dir(
            path.parent()
                .ok_or(TestError::missing("objects parent"))?
                .parent()
                .ok_or(TestError::missing("result root"))?
                .join("exports")
        )?
        .count(),
        0
    );
    running.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn export_payload_budget_rejects_before_reading_image_data_and_releases_its_slot()
-> TestResult {
    use crate::diesel_schema::photo_submissions as submissions;
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let (mut f, cookie) = admin().await?;
    let (signed, image) = fixture_package(&f).await?;
    let running = background(&mut f).await?;
    let id = ready(&f, &cookie, &signed, &image).await?;
    commit(&f, &cookie, id).await?;
    let source = signed.manifest.context.source_id.to_string();
    let hash = signed.manifest.results[0].image_sha256.to_string();
    // Inject oversized persisted payload facts while retaining a small real
    // immutable file. 413 rather than integrity failure proves preflight occurs
    // before image copying, without creating multi-gigabyte test artifacts.
    f.database
        .database
        .write(move |tx| -> Result<(), PersistenceError> {
            diesel::update(submissions::table)
                .set(submissions::image_bytes.eq(50_i64 * 1024 * 1024))
                .execute(tx.connection())?;
            for n in 0..165 {
                let account = format!("budget-{n}");
                let submission = SubmissionId::generate().to_string();
                diesel::insert_into(accounts::table)
                    .values((
                        accounts::account_id.eq(&account),
                        accounts::domjudge_username.eq(&account),
                        accounts::credential_revision.eq(1_i64),
                    ))
                    .execute(tx.connection())?;
                diesel::insert_into(teams::table)
                    .values((
                        teams::account_id.eq(&account),
                        teams::organization_id.eq(7_i64),
                        teams::name_zh.eq("预算测试"),
                        teams::name_en.eq("Budget"),
                        teams::category.eq("official"),
                    ))
                    .execute(tx.connection())?;
                diesel::insert_into(seats::table)
                    .values((seats::seat_id.eq(&account), seats::seat_code.eq(&account)))
                    .execute(tx.connection())?;
                diesel::insert_into(account_mappings::table)
                    .values((
                        account_mappings::account_id.eq(&account),
                        account_mappings::seat_id.eq(&account),
                    ))
                    .execute(tx.connection())?;
                diesel::insert_into(submissions::table)
                    .values((
                        submissions::source_id.eq(&source),
                        submissions::submission_id.eq(&submission),
                        submissions::result_id.eq(&submission),
                        submissions::import_id.eq(id.to_string()),
                        submissions::account_id.eq(&account),
                        submissions::account.eq(&account),
                        submissions::image_sha256.eq(&hash),
                        submissions::state.eq("active"),
                        submissions::assigned_account_id.eq(&account),
                        submissions::image_format.eq("png"),
                        submissions::image_bytes.eq(50_i64 * 1024 * 1024),
                        submissions::revision.eq(1_i64),
                    ))
                    .execute(tx.connection())?;
            }
            Ok(())
        })
        .await?;
    assert_eq!(
        export(&f, &cookie).await?.status,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    let bytes = i64::try_from(image.len()).map_err(|_| TestError::missing("fixture bytes"))?;
    f.database
        .database
        .write(move |tx| {
            diesel::update(submissions::table)
                .set(submissions::image_bytes.eq(bytes))
                .execute(tx.connection())
                .map_err(PersistenceError::from)
        })
        .await?;
    assert_eq!(archive(export(&f, &cookie).await?.body)?.len(), 166);
    running.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn unsafe_export_names_and_case_collisions_are_rejected_without_renaming() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let (mut f, cookie) = admin().await?;
    let (signed, image) = fixture_package(&f).await?;
    let mut manifest = signed.manifest;
    let mut second = manifest.results[0].clone();
    second.account_id = "account-b".into();
    second.account = "account-b".into();
    second.result_id = ResultId::generate();
    second.submission_id = SubmissionId::generate();
    manifest.results.push(second);
    let signed = manifest.sign(&ed25519_dalek::SigningKey::from_bytes(&[17; 32]))?;
    let running = background(&mut f).await?;
    let id = ready(&f, &cookie, &signed, &image).await?;
    commit(&f, &cookie, id).await?;
    assert_eq!(
        archive(export(&f, &cookie).await?.body)?,
        vec![
            ("account-a.png".into(), image.clone()),
            ("account-b.png".into(), image)
        ]
    );
    for name in ["CON", "../escape", "team.", "COM¹", "Case"] {
        rename(&f, "account-a", name).await?;
        if name == "CON" {
            let page = value(get(&f, &cookie, "photos?limit=1").await?)?;
            assert_eq!(page["entries"][0]["account"], "CON");
            assert_eq!(page["next"], "account-a");
            assert_eq!(
                value(get(&f, &cookie, "photos?limit=1&after_account_id=account-a").await?)?["entries"]
                    [0]["account_id"],
                "account-b"
            );
        }
        if name != "Case" {
            assert_eq!(export(&f, &cookie).await?.status, StatusCode::CONFLICT);
        }
    }
    rename(&f, "account-b", "case").await?;
    assert_eq!(export(&f, &cookie).await?.status, StatusCode::CONFLICT);
    running.shutdown().await?;
    Ok(())
}

fn large_package(signed: &SignedResults) -> Result<(SignedResults, Vec<u8>), TestError> {
    let mut seed = 19_u32;
    let image = image::RgbaImage::from_fn(512, 512, |_, _| {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        let [r, g, b, _] = seed.to_le_bytes();
        image::Rgba([r, g, b, 255])
    });
    let mut bytes = Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png)?;
    let bytes = bytes.into_inner();
    assert!(bytes.len() > 256 * 1024);
    let mut manifest = signed.manifest.clone();
    manifest.manifest_id = ManifestId::generate();
    let entry = &mut manifest.results[0];
    entry.width = 512;
    entry.height = 512;
    entry.image_sha256 = Sha256Digest::of(&bytes);
    entry.image_bytes =
        u64::try_from(bytes.len()).map_err(|_| TestError::missing("fixture length"))?;
    Ok((
        manifest.sign(&ed25519_dalek::SigningKey::from_bytes(&[17; 32]))?,
        bytes,
    ))
}

#[tokio::test]
async fn export_aggregates_independent_registered_sources_and_shared_objects() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let (mut f, cookie) = admin().await?;
    let (signed, image) = fixture_package(&f).await?;
    let first_source = signed.manifest.context.source_id.clone();
    let running = background(&mut f).await?;
    let first = ready(&f, &cookie, &signed, &image).await?;
    commit(&f, &cookie, first).await?;
    let source = f
        .photo
        .component()
        .create_source("Second photo station".into())
        .await?
        .source_id;
    let code = f
        .photo
        .component()
        .issue_code(source.clone(), PendingEnrollment::Preserve)
        .await?;
    let paired = f
        .photo
        .component()
        .pair(PhotoPairRequest {
            code: code.code,
            signing_public_key: f.key.clone(),
            client_version: "2.12.0".into(),
        })
        .await?;
    f.source = source;
    f.token = paired.token.clone();
    f.enrollment = paired.enrollment_id.to_string();
    let (second, image) = fixture_package(&f).await?;
    assert_ne!(first_source, second.manifest.context.source_id);
    let mut manifest = second.manifest;
    manifest.results[0].account_id = "account-b".into();
    manifest.results[0].account = "account-b".into();
    let signed = manifest.sign(&ed25519_dalek::SigningKey::from_bytes(&[17; 32]))?;
    let second = ready(&f, &cookie, &signed, &image).await?;
    commit(&f, &cookie, second).await?;
    let page = value(get(&f, &cookie, "photos").await?)?;
    assert_ne!(
        page["entries"][0]["source_id"],
        page["entries"][1]["source_id"]
    );
    assert_eq!(
        archive(export(&f, &cookie).await?.body)?,
        vec![
            ("account-a.png".into(), image.clone()),
            ("account-b.png".into(), image)
        ]
    );
    running.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn a_replacement_during_download_does_not_change_the_prepared_export_snapshot() -> TestResult
{
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let (mut f, cookie) = admin().await?;
    let (signed, _) = fixture_package(&f).await?;
    let (signed, image) = large_package(&signed)?;
    let running = background(&mut f).await?;
    let id = ready(&f, &cookie, &signed, &image).await?;
    let accepted = commit(&f, &cookie, id).await?;
    let held = f
        .app
        .clone()
        .oneshot(
            Request::post("/api/v2/photo/exports")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())?,
        )
        .await?;
    let (retake, changed) = different(&signed, 22)?;
    let second = ready(&f, &cookie, &retake, &changed).await?;
    commit(&f, &cookie, second).await?;
    activate(
        &f,
        &cookie,
        second,
        0,
        "account-a",
        expected(&accepted["entries"][0]["current"]),
    )
    .await?;
    let bytes = axum::body::to_bytes(held.into_body(), 2 * 1024 * 1024).await?;
    assert_eq!(
        archive(bytes.to_vec())?,
        vec![("account-a.png".into(), image)]
    );
    assert_eq!(
        archive(export(&f, &cookie).await?.body)?,
        vec![("account-a.png".into(), changed)]
    );
    running.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn rejecting_an_invalid_assignment_preserves_other_submissions() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let (mut f, cookie) = admin().await?;
    let (signed, image) = fixture_package(&f).await?;
    let running = background(&mut f).await?;
    let id = ready(&f, &cookie, &signed, &image).await?;
    commit(&f, &cookie, id).await?;
    f.database
        .database
        .write(|tx| {
            diesel::delete(accounts::table.find("account-a"))
                .execute(tx.connection())
                .map_err(PersistenceError::from)
        })
        .await?;
    let request = json!({"submission_id":signed.manifest.results[0].submission_id,"expected_submission_revision":1,"decision":{"action":"reject"}});
    let rejected = value(
        call(
            &f,
            &cookie,
            Method::POST,
            &format!("{id}/decisions"),
            request,
        )
        .await?,
    )?;
    assert_eq!(rejected["entries"][0]["state"], "rejected");
    assert!(archive(export(&f, &cookie).await?.body)?.is_empty());
    let (new, image) = different(&signed, 33)?;
    let new_id = ready(&f, &cookie, &new, &image).await?;
    assert_eq!(
        commit(&f, &cookie, new_id).await?["entries"][0]["submission_revision"],
        1
    );
    let active = activate(&f, &cookie, new_id, 0, "account-b", Value::Null).await?;
    assert_eq!(active["entries"][0]["active_account_id"], "account-b");
    running.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn stalled_and_cancelled_downloads_hold_capacity_until_the_tracked_producer_stops()
-> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let (mut f, cookie) = admin().await?;
    let (signed, _) = fixture_package(&f).await?;
    let (signed, image) = large_package(&signed)?;
    let running = background(&mut f).await?;
    let id = ready(&f, &cookie, &signed, &image).await?;
    commit(&f, &cookie, id).await?;
    let request = || {
        Request::post("/api/v2/photo/exports")
            .header(header::COOKIE, &cookie)
            .body(Body::empty())
    };
    let held = f.app.clone().oneshot(request()?).await?;
    assert_eq!(held.status(), StatusCode::OK);
    let busy = export(&f, &cookie).await?;
    assert_eq!(busy.status, StatusCode::SERVICE_UNAVAILABLE);
    drop(held);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let response = f.app.clone().oneshot(request()?).await?;
        if response.status() == StatusCode::OK {
            drop(response);
            break;
        }
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(tokio::time::Instant::now() < deadline);
        tokio::task::yield_now().await;
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        f.lifecycle.begin_shutdown();
        f.lifecycle.wait().await;
    })
    .await?;
    running.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn export_ignores_historical_invalid_associations() -> TestResult {
    use crate::diesel_schema::photo_submissions as history;
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let (mut f, cookie) = admin().await?;
    let (signed, image) = fixture_package(&f).await?;
    let running = background(&mut f).await?;
    let id = ready(&f, &cookie, &signed, &image).await?;
    commit(&f, &cookie, id).await?;
    let source = signed.manifest.context.source_id.to_string();
    let hash = signed.manifest.results[0].image_sha256.to_string();
    f.database
        .database
        .write(move |tx| -> Result<(), crate::db::PersistenceError> {
            for index in 0..natsume_photo_protocol::TEAM_LIMIT {
                let account = format!("removed-{index}");
                let submission = SubmissionId::generate().to_string();
                diesel::insert_into(history::table)
                    .values((
                        history::source_id.eq(&source),
                        history::submission_id.eq(&submission),
                        history::result_id.eq(ResultId::generate().to_string()),
                        history::import_id.eq(id.to_string()),
                        history::account_id.eq(&account),
                        history::account.eq(&account),
                        history::image_sha256.eq(&hash),
                        history::state.eq("active"),
                        history::assigned_account_id.eq(&account),
                        history::image_format.eq("png"),
                        history::image_bytes.eq(1_i64),
                        history::revision.eq(1_i64),
                    ))
                    .execute(tx.connection())?;
            }
            Ok(())
        })
        .await?;
    let response = drive(
        &f.app,
        Request::post("/api/v2/photo/exports")
            .header(header::COOKIE, &cookie)
            .body(Body::empty())?,
    )
    .await?;
    running.shutdown().await?;
    assert_eq!(
        response.status,
        StatusCode::OK,
        "one current valid photo must export despite removed-team history"
    );
    assert_eq!(
        archive(response.body)?,
        vec![("account-a.png".into(), image)]
    );
    Ok(())
}
