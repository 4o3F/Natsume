//! Verified packages through real admin routes and transaction rollback.
use super::*;
use super::{
    imports::admin,
    validation::{background, fixture_package, package, submit, terminal},
};
use crate::{
    diesel_schema::photo_submissions as submissions, http::tests::PasswordVerificationTestGuard,
};
use natsume_photo_protocol::{
    Sha256Digest,
    results::{ManifestId, ResultId, SignedResults, SubmissionId},
};
use std::{error::Error as _, io::Cursor};
use zip::CompressionMethod;

#[test]
fn result_decision_accepts_the_reject_action_contract() -> TestResult {
    let _: crate::component::photo::PhotoDecision = serde_json::from_value(json!({
        "submission_id":SubmissionId::generate(), "expected_submission_revision":1,
        "action":{"action":"reject"}
    }))?;
    Ok(())
}

pub(super) async fn call(
    f: &Fixture,
    cookie: &str,
    method: Method,
    path: &str,
    value: Value,
) -> Result<Captured, TestError> {
    Ok(drive(
        &f.app,
        Request::builder()
            .method(method)
            .uri(format!("/api/v2/photo/imports/{path}"))
            .header(header::COOKIE, cookie)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(&value)?))?,
    )
    .await?)
}
pub(super) async fn ready(
    f: &Fixture,
    cookie: &str,
    signed: &SignedResults,
    image: &[u8],
) -> Result<Uuid, TestError> {
    let id = submit(
        f,
        cookie,
        package(signed, image, CompressionMethod::Stored, None)?,
    )
    .await?;
    assert_eq!(terminal(f, cookie, id).await?["state"], "ready");
    Ok(id)
}
pub(super) fn different(
    signed: &SignedResults,
    color: u8,
) -> Result<(SignedResults, Vec<u8>), TestError> {
    let mut bytes = Cursor::new(Vec::new());
    image::RgbaImage::from_pixel(8, 4, image::Rgba([color, 0, 0, 255]))
        .write_to(&mut bytes, image::ImageFormat::Png)?;
    let bytes = bytes.into_inner();
    let mut manifest = signed.manifest.clone();
    manifest.manifest_id = ManifestId::generate();
    let image = &mut manifest.results[0];
    image.result_id = ResultId::generate();
    image.submission_id = SubmissionId::generate();
    image.image_sha256 = Sha256Digest::of(&bytes);
    image.width = 8;
    image.height = 4;
    image.image_bytes = u64::try_from(bytes.len()).map_err(|_| TestError::missing("image size"))?;
    Ok((
        manifest.sign(&ed25519_dalek::SigningKey::from_bytes(&[17; 32]))?,
        bytes,
    ))
}
pub(super) fn value(response: Captured) -> Result<Value, TestError> {
    assert_eq!(response.status, StatusCode::OK);
    Ok(serde_json::from_reader(Cursor::new(response.body))?)
}

#[tokio::test]
async fn preview_is_read_only_and_reuploads_and_commits_preserve_submission_identity() -> TestResult
{
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let (mut f, cookie) = admin().await?;
    let (signed, image) = fixture_package(&f).await?;
    let running = background(&mut f).await?;
    let id = ready(&f, &cookie, &signed, &image).await?;
    let preview = value(
        call(
            &f,
            &cookie,
            Method::GET,
            &format!("{id}/preview"),
            Value::Null,
        )
        .await?,
    )?;
    assert_eq!(preview["committed"], false);
    assert_eq!(preview["entries"][0]["submission_revision"], 0);
    assert_eq!(preview["entries"][0]["current"], Value::Null);
    let count: i64 = f
        .database
        .database
        .read(|tx| {
            submissions::table
                .count()
                .get_result(tx.connection())
                .map_err(PersistenceError::from)
        })
        .await?;
    assert_eq!(count, 0);
    let accepted = commit(&f, &cookie, id).await?;
    assert_eq!(accepted["committed"], true);
    assert_eq!(accepted["entries"][0]["current"]["revision"], 1);
    assert_eq!(commit(&f, &cookie, id).await?, accepted);
    let mut manifest = signed.manifest.clone();
    manifest.manifest_id = ManifestId::generate();
    let same = manifest.sign(&ed25519_dalek::SigningKey::from_bytes(&[17; 32]))?;
    let second = ready(&f, &cookie, &same, &image).await?;
    let replay = commit(&f, &cookie, second).await?;
    assert_eq!(replay["entries"], accepted["entries"]);
    let count: i64 = f
        .database
        .database
        .read(|tx| {
            submissions::table
                .count()
                .get_result(tx.connection())
                .map_err(PersistenceError::from)
        })
        .await?;
    assert_eq!(count, 1);
    f.database
        .database
        .write(|tx| {
            diesel::update(submissions::table)
                .set(submissions::result_id.eq("invalid-result"))
                .execute(tx.connection())
                .map_err(PersistenceError::from)
        })
        .await?;
    let error = crate::photo_acceptance::AcceptPhotoResults::new(f.database.database.clone())
        .preview(id)
        .await
        .err()
        .ok_or_else(|| TestError::missing("persisted result ID error"))?;
    let mut source = error.source();
    let mut found = false;
    while let Some(error) = source {
        found |= error.is::<natsume_photo_protocol::ValueError>();
        source = error.source();
    }
    assert!(
        found,
        "persisted result IDs must retain their protocol validation source"
    );
    running.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn retake_requires_revision_checked_replacement_and_rejection_keeps_current_photo()
-> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let (mut f, cookie) = admin().await?;
    let (signed, image) = fixture_package(&f).await?;
    let running = background(&mut f).await?;
    let first = ready(&f, &cookie, &signed, &image).await?;
    let initial = commit(&f, &cookie, first).await?;
    let (retake, image) = different(&signed, 90)?;
    let second = ready(&f, &cookie, &retake, &image).await?;
    let pending = commit(&f, &cookie, second).await?;
    assert_eq!(pending["entries"][0]["state"], "pending_review");
    assert_eq!(
        pending["entries"][0]["current"],
        initial["entries"][0]["current"]
    );
    let team = &retake.manifest.results[0];
    let mut request = json!({"submission_id": team.submission_id, "expected_submission_revision":1,"action":{"action":"activate", "account_id":team.account_id,"expected_current":null}});
    assert_eq!(
        call(
            &f,
            &cookie,
            Method::POST,
            &format!("{second}/decisions"),
            request.clone()
        )
        .await?
        .status,
        StatusCode::CONFLICT
    );
    let old = &initial["entries"][0]["current"];
    request["action"]["expected_current"] =
        json!({"source_id":old["source_id"],"submission_id":old["submission_id"]});
    let updated = value(
        call(
            &f,
            &cookie,
            Method::POST,
            &format!("{second}/decisions"),
            request.clone(),
        )
        .await?,
    )?;
    assert_eq!(updated["entries"][0]["state"], "active");
    assert_eq!(updated["entries"][0]["current"]["revision"], 2);
    assert_eq!(
        call(
            &f,
            &cookie,
            Method::POST,
            &format!("{second}/decisions"),
            request
        )
        .await?
        .status,
        StatusCode::CONFLICT
    );
    let first_after = value(
        call(
            &f,
            &cookie,
            Method::GET,
            &format!("{first}/preview"),
            Value::Null,
        )
        .await?,
    )?;
    assert_eq!(first_after["entries"][0]["state"], "superseded");
    let (third, image) = different(&signed, 130)?;
    let third_id = ready(&f, &cookie, &third, &image).await?;
    commit(&f, &cookie, third_id).await?;
    let rejected = value(call(&f, &cookie, Method::POST, &format!("{third_id}/decisions"), json!({"submission_id":third.manifest.results[0].submission_id,"expected_submission_revision":1,"action":{"action":"reject"}})).await?)?;
    assert_eq!(rejected["entries"][0]["state"], "rejected");
    assert_eq!(
        rejected["entries"][0]["current"],
        updated["entries"][0]["current"]
    );
    running.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn removed_roster_identity_requires_explicit_association_and_preserves_original_identity()
-> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let (mut f, cookie) = admin().await?;
    let (signed, image) = fixture_package(&f).await?;
    let running = background(&mut f).await?;
    let id = ready(&f, &cookie, &signed, &image).await?;
    f.database
        .database
        .write(|tx| {
            diesel::delete(accounts::table.find("account-a"))
                .execute(tx.connection())
                .map_err(PersistenceError::from)
        })
        .await?;
    let pending = commit(&f, &cookie, id).await?;
    assert_eq!(pending["entries"][0]["state"], "needs_association");
    let request = json!({"submission_id":signed.manifest.results[0].submission_id,"expected_submission_revision":1,"action":{"action":"activate","account_id":"account-b","expected_current":null}});
    let associated = value(
        call(
            &f,
            &cookie,
            Method::POST,
            &format!("{id}/decisions"),
            request,
        )
        .await?,
    )?;
    assert_eq!(associated["entries"][0]["account_id"], "account-a");
    assert_eq!(associated["entries"][0]["account"], "account-a");
    assert_eq!(associated["entries"][0]["active_account_id"], "account-b");
    running.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn revoked_sources_and_reused_identities_cannot_change_current_photos() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let (mut f, cookie) = admin().await?;
    let (signed, image) = fixture_package(&f).await?;
    let running = background(&mut f).await?;
    let id = ready(&f, &cookie, &signed, &image).await?;
    let source = signed.manifest.context.source_id.clone();
    f.photo
        .component()
        .set_state(source.clone(), SourceState::Disabled)
        .await?;
    assert_eq!(
        call(
            &f,
            &cookie,
            Method::GET,
            &format!("{id}/preview"),
            Value::Null
        )
        .await?
        .status,
        StatusCode::OK
    );
    assert_eq!(
        call(
            &f,
            &cookie,
            Method::POST,
            &format!("{id}/commit"),
            Value::Null
        )
        .await?
        .status,
        StatusCode::FORBIDDEN
    );
    f.photo
        .component()
        .set_state(source, SourceState::Enabled)
        .await?;
    let accepted = commit(&f, &cookie, id).await?;
    let (other, image) = different(&signed, 160)?;
    let mut manifest = other.manifest;
    manifest.results[0].submission_id = signed.manifest.results[0].submission_id.clone();
    manifest.results[0].result_id = signed.manifest.results[0].result_id.clone();
    let forged = manifest.sign(&ed25519_dalek::SigningKey::from_bytes(&[17; 32]))?;
    let other = ready(&f, &cookie, &forged, &image).await?;
    assert_eq!(
        call(
            &f,
            &cookie,
            Method::GET,
            &format!("{other}/preview"),
            Value::Null
        )
        .await?
        .status,
        StatusCode::CONFLICT
    );
    assert_eq!(
        call(
            &f,
            &cookie,
            Method::POST,
            &format!("{other}/commit"),
            Value::Null
        )
        .await?
        .status,
        StatusCode::CONFLICT
    );
    assert_eq!(
        value(
            call(
                &f,
                &cookie,
                Method::GET,
                &format!("{id}/preview"),
                Value::Null
            )
            .await?
        )?,
        accepted
    );
    running.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn revoked_sources_allow_preview_but_not_decisions() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let (mut f, cookie) = admin().await?;
    let (signed, image) = fixture_package(&f).await?;
    let running = background(&mut f).await?;
    let id = ready(&f, &cookie, &signed, &image).await?;
    let accepted = commit(&f, &cookie, id).await?;
    let (retake, image) = different(&signed, 175)?;
    let retake_id = ready(&f, &cookie, &retake, &image).await?;
    let pending = commit(&f, &cookie, retake_id).await?;
    f.photo
        .component()
        .set_state(
            signed.manifest.context.source_id.clone(),
            SourceState::Revoked,
        )
        .await?;
    assert_eq!(
        value(
            call(
                &f,
                &cookie,
                Method::GET,
                &format!("{retake_id}/preview"),
                Value::Null
            )
            .await?
        )?,
        pending
    );
    assert_eq!(
        call(
            &f,
            &cookie,
            Method::POST,
            &format!("{retake_id}/decisions"),
            json!({
                "submission_id":retake.manifest.results[0].submission_id,
                "expected_submission_revision":1,
                "action":{"action":"reject"}
            })
        )
        .await?
        .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        value(
            call(
                &f,
                &cookie,
                Method::GET,
                &format!("{id}/preview"),
                Value::Null
            )
            .await?
        )?,
        accepted
    );
    running.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn concurrent_commits_preserve_first_photo_and_failed_commit_rolls_back_every_row()
-> TestResult {
    use diesel::connection::SimpleConnection as _;
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let (mut f, cookie) = admin().await?;
    let (signed, image) = fixture_package(&f).await?;
    let running = background(&mut f).await?;
    let id = ready(&f, &cookie, &signed, &image).await?;
    // Fault injection intentionally uses DDL, not handwritten application SQL.
    f.database.database.write(|tx| tx.connection().batch_execute("CREATE TRIGGER photo_test_fail_commit BEFORE UPDATE OF state ON photo_import_jobs WHEN NEW.state = 'committed' BEGIN SELECT RAISE(ABORT, 'injected'); END;").map_err(PersistenceError::from)).await?;
    assert_eq!(
        call(
            &f,
            &cookie,
            Method::POST,
            &format!("{id}/commit"),
            Value::Null
        )
        .await?
        .status,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    let counts: (i64, i64) = f
        .database
        .database
        .read(|tx| {
            Ok::<_, PersistenceError>((
                submissions::table.count().get_result(tx.connection())?,
                submissions::table
                    .filter(submissions::state.eq("active"))
                    .count()
                    .get_result(tx.connection())?,
            ))
        })
        .await?;
    assert_eq!(counts, (0, 0));
    f.database
        .database
        .write(|tx| {
            tx.connection()
                .batch_execute("DROP TRIGGER photo_test_fail_commit")
                .map_err(PersistenceError::from)
        })
        .await?;
    let (other, image) = different(&signed, 200)?;
    let second = ready(&f, &cookie, &other, &image).await?;
    let p1 = format!("{id}/commit");
    let p2 = format!("{second}/commit");
    let (a, b) = tokio::join!(
        call(&f, &cookie, Method::POST, &p1, Value::Null),
        call(&f, &cookie, Method::POST, &p2, Value::Null)
    );
    let a = value(a?)?;
    let b = value(b?)?;
    let states = [
        a["entries"][0]["state"].as_str(),
        b["entries"][0]["state"].as_str(),
    ];
    assert!(states.contains(&Some("active")) && states.contains(&Some("pending_review")));
    let counts: (i64, i64) = f
        .database
        .database
        .read(|tx| {
            Ok::<_, PersistenceError>((
                submissions::table.count().get_result(tx.connection())?,
                submissions::table
                    .filter(submissions::state.eq("active"))
                    .count()
                    .get_result(tx.connection())?,
            ))
        })
        .await?;
    assert_eq!(counts, (2, 1));
    running.shutdown().await?;
    Ok(())
}

pub(super) async fn commit(f: &Fixture, cookie: &str, id: Uuid) -> Result<Value, TestError> {
    value(
        call(
            f,
            cookie,
            Method::POST,
            &format!("{id}/commit"),
            Value::Null,
        )
        .await?,
    )
}

pub(super) async fn pending(f: &Fixture, cookie: &str, query: &str) -> Result<Captured, TestError> {
    Ok(drive(
        &f.app,
        Request::builder()
            .uri(format!("/api/v2/photo/submissions{query}"))
            .header(header::COOKIE, cookie)
            .body(Body::empty())?,
    )
    .await?)
}

pub(super) async fn visible_pending(
    f: &Fixture,
    cookie: &str,
    mut query: String,
) -> Result<Value, TestError> {
    loop {
        let page = value(pending(f, cookie, &query).await?)?;
        if page["entries"]
            .as_array()
            .is_some_and(|entries| !entries.is_empty())
            || page["next"].is_null()
        {
            return Ok(page);
        }
        let source = page["next"]["after_source_id"]
            .as_str()
            .ok_or(TestError::missing("source cursor"))?;
        let submission = page["next"]["after_submission_id"]
            .as_str()
            .ok_or(TestError::missing("submission cursor"))?;
        query = format!("?limit=1&after_source_id={source}&after_submission_id={submission}");
    }
}

#[tokio::test]
async fn pending_list_is_bounded_and_resumes_by_stable_identity_after_a_decision() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let (mut f, cookie) = admin().await?;
    let (signed, image) = fixture_package(&f).await?;
    let running = background(&mut f).await?;
    let first = ready(&f, &cookie, &signed, &image).await?;
    commit(&f, &cookie, first).await?;
    let mut manifest = signed.manifest.clone();
    manifest.manifest_id = ManifestId::generate();
    manifest.results[0].result_id = ResultId::generate();
    manifest.results[0].submission_id = SubmissionId::generate();
    let mut second = manifest.results[0].clone();
    second.account_id = "account-b".into();
    second.account = "account-b".into();
    second.result_id = ResultId::generate();
    second.submission_id = SubmissionId::generate();
    manifest.results.push(second);
    let (different, changed_image) = different(&signed, 42)?;
    for result in &mut manifest.results {
        result.image_sha256 = different.manifest.results[0].image_sha256.clone();
        result.image_bytes = different.manifest.results[0].image_bytes;
    }
    let two = manifest.sign(&ed25519_dalek::SigningKey::from_bytes(&[17; 32]))?;
    let id = ready(&f, &cookie, &two, &changed_image).await?;
    f.database
        .database
        .write(|tx| {
            diesel::delete(accounts::table.find("account-b"))
                .execute(tx.connection())
                .map_err(PersistenceError::from)
        })
        .await?;
    commit(&f, &cookie, id).await?;
    assert_eq!(pending(&f, "", "").await?.status, StatusCode::UNAUTHORIZED);
    for query in [
        "?limit=0",
        "?limit=101",
        "?unknown=1",
        "?after_source_id=invalid",
    ] {
        assert_eq!(
            pending(&f, &cookie, query).await?.status,
            StatusCode::BAD_REQUEST
        );
    }
    let page1 = visible_pending(&f, &cookie, "?limit=1".into()).await?;
    assert_eq!(page1["entries"].as_array().map(Vec::len), Some(1));
    assert!(!page1["next"].is_null());
    let entry = &page1["entries"][0];
    value(call(&f, &cookie, Method::POST, &format!("{id}/decisions"), json!({"submission_id":entry["submission_id"],"expected_submission_revision":entry["revision"],"action":{"action":"reject"}})).await?)?;
    let source = page1["next"]["after_source_id"]
        .as_str()
        .ok_or(TestError::missing("source cursor"))?;
    let submission = page1["next"]["after_submission_id"]
        .as_str()
        .ok_or(TestError::missing("submission cursor"))?;
    assert_eq!(
        pending(&f, &cookie, &format!("?after_source_id={source}"))
            .await?
            .status,
        StatusCode::BAD_REQUEST
    );
    let page2 = value(
        pending(
            &f,
            &cookie,
            &format!("?limit=1&after_source_id={source}&after_submission_id={submission}"),
        )
        .await?,
    )?;
    assert_eq!(page2["entries"].as_array().map(Vec::len), Some(1));
    assert_ne!(page2["entries"][0]["submission_id"], entry["submission_id"]);
    assert_eq!(page2["next"], Value::Null);
    running.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn mixed_package_accepts_valid_team_and_explicit_reassociation_can_target_another_identity()
-> TestResult {
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
    f.database
        .database
        .write(|tx| {
            diesel::delete(accounts::table.find("account-a"))
                .execute(tx.connection())
                .map_err(PersistenceError::from)
        })
        .await?;
    let accepted = commit(&f, &cookie, id).await?;
    assert_eq!(accepted["entries"][0]["state"], "needs_association");
    assert_eq!(accepted["entries"][1]["state"], "active");
    let current = &accepted["entries"][1]["current"];
    let decision = json!({"submission_id":signed.manifest.results[0].submission_id,"expected_submission_revision":1,"action":{"action":"activate","account_id":"account-b","expected_current":{"source_id":current["source_id"],"submission_id":current["submission_id"]}}});
    let replaced = value(
        call(
            &f,
            &cookie,
            Method::POST,
            &format!("{id}/decisions"),
            decision,
        )
        .await?,
    )?;
    assert_eq!(replaced["entries"][0]["state"], "active");
    assert_eq!(replaced["entries"][0]["active_account_id"], "account-b");
    assert_eq!(replaced["entries"][0]["account_id"], "account-a");
    assert_eq!(replaced["entries"][1]["state"], "superseded");
    assert_eq!(replaced["entries"][1]["current"]["revision"], 2);
    running.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn acceptance_routes_require_admin_and_canonical_ids_and_closed_decision_bodies() -> TestResult
{
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let (f, cookie) = admin().await?;
    for (method, action) in [
        (Method::GET, "preview"),
        (Method::POST, "commit"),
        (Method::POST, "decisions"),
    ] {
        assert_eq!(
            call(
                &f,
                "",
                method.clone(),
                &format!("invalid/{action}"),
                json!({})
            )
            .await?
            .status,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            call(
                &f,
                &cookie,
                method.clone(),
                &format!("invalid/{action}"),
                json!({})
            )
            .await?
            .status,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            call(
                &f,
                &cookie,
                method,
                &format!("550e8400-e29b-41d4-a716-446655440000/{action}"),
                json!({})
            )
            .await?
            .status,
            StatusCode::BAD_REQUEST
        );
    }
    let id = Uuid::now_v7();
    assert_eq!(
        call(
            &f,
            &cookie,
            Method::GET,
            &format!("{id}/preview"),
            Value::Null
        )
        .await?
        .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(call(&f, &cookie, Method::POST, &format!("{id}/decisions"), json!({"submission_id":"x","expected_submission_revision":1,"action":{"action":"reject","unexpected":true}})).await?.status, StatusCode::BAD_REQUEST);
    let submission = SubmissionId::generate();
    for body in [
        json!({"submission_id":"invalid", "expected_submission_revision":1,"action":{"action":"reject"}}),
        json!({"submission_id":"550e8400-e29b-41d4-a716-446655440000", "expected_submission_revision":1,"action":{"action":"reject"}}),
        json!({"submission_id":submission, "expected_submission_revision":1,"action":{"action":"activate","account_id":"account-a","expected_current":{"source_id":"invalid","submission_id":submission}}}),
    ] {
        let response = call(&f, &cookie, Method::POST, &format!("{id}/decisions"), body).await?;
        assert_eq!(response.status, StatusCode::BAD_REQUEST);
        let error: Value = serde_json::from_slice(&response.body)?;
        assert_eq!(error["code"], "INVALID_REQUEST");
    }
    Ok(())
}

#[tokio::test]
async fn pending_preview_agrees_after_roster_change() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let (mut f, cookie) = admin().await?;
    let (signed, image) = fixture_package(&f).await?;
    let running = background(&mut f).await?;
    let first = ready(&f, &cookie, &signed, &image).await?;
    commit(&f, &cookie, first).await?;
    let (retake, bytes) = different(&signed, 91)?;
    let second = ready(&f, &cookie, &retake, &bytes).await?;
    commit(&f, &cookie, second).await?;
    f.database
        .database
        .write(|tx| -> Result<(), crate::db::PersistenceError> {
            diesel::delete(accounts::table.find("account-a")).execute(tx.connection())?;
            Ok(())
        })
        .await?;
    let pending = visible_pending(&f, &cookie, String::new()).await?;
    let target = retake.manifest.results[0].submission_id.as_str();
    let listed = pending["entries"]
        .as_array()
        .and_then(|entries| entries.iter().find(|row| row["submission_id"] == target))
        .ok_or_else(|| TestError::missing("pending retake"))?;
    let preview = value(
        call(
            &f,
            &cookie,
            Method::GET,
            &format!("{second}/preview"),
            Value::Null,
        )
        .await?,
    )?;
    running.shutdown().await?;
    assert_eq!(
        preview["entries"][0]["state"], listed["state"],
        "same submission must have one current state projection"
    );
    Ok(())
}

#[tokio::test]
async fn upload_and_startup_worker_errors_do_not_include_panic_payload() -> TestResult {
    let source = tokio::spawn(async { panic!("review-panic-payload-canary") })
        .await
        .err()
        .ok_or_else(|| TestError::missing("panic join error"))?;
    let source = crate::task::TaskFailure::from(source);
    let upload = crate::component::photo::ResultImportError::Worker { source };
    let startup = crate::component::photo::PhotoStorageError::Worker { source };
    for report in [
        snafu::Report::from_error(upload).to_string(),
        snafu::Report::from_error(startup).to_string(),
    ] {
        assert!(
            !report.contains("review-panic-payload-canary"),
            "Report must not contain panic payload"
        );
        assert!(report.contains("the task panicked"));
    }
    Ok(())
}
