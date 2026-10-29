use super::*;

async fn expire_token(f: &Fixture) -> TestResult {
    let source = f.source.clone();
    f.database
        .database
        .write(move |tx| -> Result<(), PersistenceError> {
            diesel::update(photo_sources::table.find(source))
                .set(photo_sources::token_expires_at_unix_ms.eq(0_i64))
                .execute(tx.connection())?;
            Ok(())
        })
        .await
        .map_err(crate::db::TransactionError::into_error)?;
    Ok(())
}

async fn replace_and_confirm(f: &mut Fixture) -> TestResult {
    assert!(matches!(
        f.state
            .photo()
            .issue_code(f.source.clone(), PendingEnrollment::Preserve)
            .await,
        Err(EnrollmentError::Conflict)
    ));
    let replacement = f
        .state
        .photo()
        .issue_code(f.source.clone(), PendingEnrollment::Replace)
        .await;
    assert!(
        replacement.is_ok(),
        "explicit replacement must recover a confirmed source whose initialization has not completed"
    );
    assert!(
        f.state
            .photo()
            .check_token(&f.token, CredentialUse::Initialization)
            .await
            .is_err()
    );
    let key = hex::encode(
        ed25519_dalek::SigningKey::from_bytes(&[18; 32])
            .verifying_key()
            .to_bytes(),
    );
    let issued = f
        .state
        .photo()
        .pair(PhotoPairRequest {
            code: replacement?.code,
            signing_public_key: key.clone(),
            client_version: "2.11.0".into(),
        })
        .await?;
    assert_eq!(issued.source_id.as_str(), f.source);
    f.token = issued.token.clone();
    f.enrollment = issued.enrollment_id.to_string();
    f.key = key;
    f.confirm().await
}

#[tokio::test]
async fn confirmed_source_recovers_after_token_expiry_without_a_snapshot() -> TestResult {
    let mut f = Fixture::new().await?;
    f.confirm().await?;
    expire_token(&f).await?;
    assert_eq!(
        f.send(Method::POST, "client/initialization", Value::Null)
            .await?
            .status,
        StatusCode::UNAUTHORIZED
    );
    replace_and_confirm(&mut f).await?;
    assert_eq!(
        f.send(Method::POST, "client/initialization", Value::Null)
            .await?
            .status,
        StatusCode::OK
    );
    Ok(())
}

#[tokio::test]
async fn confirmed_source_replaces_an_uncompleted_snapshot_after_roster_correction() -> TestResult {
    let mut f = Fixture::new().await?;
    f.confirm().await?;
    let original = f
        .send(Method::POST, "client/initialization", Value::Null)
        .await?;
    assert_eq!(original.status, StatusCode::OK);
    let before: Value = serde_json::from_slice(&original.body)?;
    f.database
        .database
        .write(|tx| -> Result<(), PersistenceError> {
            diesel::update(teams::table.find("account-a"))
                .set(teams::name_zh.eq("Corrected team"))
                .execute(tx.connection())?;
            Ok(())
        })
        .await
        .map_err(crate::db::TransactionError::into_error)?;
    // Also require replacement to invalidate an old credential that is still valid.
    f.state
        .photo()
        .check_token(&f.token, CredentialUse::Initialization)
        .await?;
    replace_and_confirm(&mut f).await?;
    let replacement = f
        .send(Method::POST, "client/initialization", Value::Null)
        .await?;
    assert_eq!(replacement.status, StatusCode::OK);
    let after: Value = serde_json::from_slice(&replacement.body)?;
    assert_ne!(before["initialization_id"], after["initialization_id"]);
    assert_eq!(before["dataset_id"], after["dataset_id"]);
    assert!(
        after["teams"]
            .as_array()
            .ok_or(TestError::missing("teams"))?
            .iter()
            .any(|team| team["account_id"] == "account-a"
                && team["team_name_zh"] == "Corrected team")
    );
    assert_eq!(f.state.photo().list_sources().await?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn completed_source_still_rejects_explicit_replacement() -> TestResult {
    let f = Fixture::new().await?;
    f.confirm().await?;
    let snapshot = f
        .send(Method::POST, "client/initialization", Value::Null)
        .await?;
    assert_eq!(snapshot.status, StatusCode::OK);
    let manifest: Value = serde_json::from_slice(&snapshot.body)?;
    let receipt = json!({"initialization_id":manifest["initialization_id"],
        "manifest_sha256":hex::encode(Sha256::digest(&snapshot.body)), "request_id":Uuid::now_v7().to_string()});
    assert_eq!(
        f.send(
            Method::POST,
            "client/initialization/complete",
            receipt.clone()
        )
        .await?
        .status,
        StatusCode::NO_CONTENT
    );
    assert!(matches!(
        f.state
            .photo()
            .issue_code(f.source.clone(), PendingEnrollment::Replace)
            .await,
        Err(EnrollmentError::Conflict)
    ));
    assert_eq!(
        f.send(Method::POST, "client/initialization/complete", receipt)
            .await?
            .status,
        StatusCode::NO_CONTENT
    );
    Ok(())
}

#[tokio::test]
async fn disabled_and_revoked_sources_cannot_be_replaced() -> TestResult {
    for state in [SourceState::Disabled, SourceState::Revoked] {
        let f = Fixture::new().await?;
        f.confirm().await?;
        f.state.photo().set_state(f.source.clone(), state).await?;
        assert!(matches!(
            f.state
                .photo()
                .issue_code(f.source, PendingEnrollment::Replace)
                .await,
            Err(EnrollmentError::Conflict)
        ));
    }
    Ok(())
}

#[tokio::test]
async fn confirmed_source_replacement_requires_an_administrator_session() -> TestResult {
    use super::super::{
        LOGIN_NAME, PASSWORD, PasswordVerificationTestGuard, header_text, login_request,
        seed_operator,
    };
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = Fixture::new().await?;
    f.confirm().await?;
    let path = format!("sources/{}/reset-enrollment", f.source);
    let body = json!({});
    assert_eq!(
        f.send(Method::POST, &path, body.clone()).await?.status,
        StatusCode::UNAUTHORIZED
    );
    f.state
        .photo()
        .check_token(&f.token, CredentialUse::Initialization)
        .await?;
    seed_operator(&f.database.database, LOGIN_NAME, PASSWORD).await?;
    let login = drive(&f.app, login_request(LOGIN_NAME, PASSWORD)?).await?;
    let cookie = header_text(&login.headers, &header::SET_COOKIE)?
        .split(';')
        .next()
        .ok_or(TestError::missing("administrator cookie"))?;
    let response = drive(
        &f.app,
        Request::builder()
            .method(Method::POST)
            .uri(format!("/api/v2/photo/{path}"))
            .header(header::COOKIE, cookie)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))?,
    )
    .await?;
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(
        f.send(Method::POST, "client/initialization", Value::Null)
            .await?
            .status,
        StatusCode::UNAUTHORIZED
    );
    Ok(())
}

#[tokio::test]
async fn replacement_and_completion_cannot_both_commit() -> TestResult {
    let f = Fixture::new().await?;
    f.confirm().await?;
    let snapshot = f
        .send(Method::POST, "client/initialization", Value::Null)
        .await?;
    assert_eq!(snapshot.status, StatusCode::OK);
    let manifest: Value = serde_json::from_slice(&snapshot.body)?;
    let receipt = json!({"initialization_id":manifest["initialization_id"],
        "manifest_sha256":hex::encode(Sha256::digest(&snapshot.body)), "request_id":Uuid::now_v7().to_string()});
    let (completion, replacement) = tokio::join!(
        f.send(
            Method::POST,
            "client/initialization/complete",
            receipt.clone()
        ),
        f.state
            .photo()
            .issue_code(f.source.clone(), PendingEnrollment::Replace)
    );
    let status = completion?.status;
    match replacement {
        Ok(_) => {
            assert_eq!(status, StatusCode::UNAUTHORIZED);
            assert_eq!(
                f.send(Method::POST, "client/initialization/complete", receipt)
                    .await?
                    .status,
                StatusCode::UNAUTHORIZED
            );
        }
        Err(EnrollmentError::Conflict) => {
            assert_eq!(status, StatusCode::NO_CONTENT);
            assert_eq!(
                f.send(Method::POST, "client/initialization/complete", receipt)
                    .await?
                    .status,
                StatusCode::NO_CONTENT
            );
        }
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

#[tokio::test]
async fn replacement_prevents_old_snapshot_publication() -> TestResult {
    let f = Fixture::new().await?;
    f.confirm().await?;
    let (download, replacement) = tokio::join!(
        f.send(Method::POST, "client/initialization", Value::Null),
        f.state
            .photo()
            .issue_code(f.source.clone(), PendingEnrollment::Replace)
    );
    replacement?;
    assert!(matches!(
        download?.status,
        StatusCode::OK | StatusCode::UNAUTHORIZED
    ));
    let source = f.source.clone();
    let count: i64 = f
        .database
        .database
        .read(move |tx| -> Result<_, PersistenceError> {
            photo_initializations::table
                .filter(photo_initializations::source_id.eq(source))
                .count()
                .get_result(tx.connection())
                .map_err(PersistenceError::from)
        })
        .await
        .map_err(crate::db::TransactionError::into_error)?;
    assert_eq!(
        count, 0,
        "reset must remove or prevent the old enrollment's snapshot"
    );
    assert_eq!(
        f.send(Method::POST, "client/initialization", Value::Null)
            .await?
            .status,
        StatusCode::UNAUTHORIZED
    );
    Ok(())
}

#[tokio::test]
async fn failed_replacement_rolls_back_snapshot_removal_and_preserves_credentials() -> TestResult {
    use diesel::connection::SimpleConnection as _;
    let f = Fixture::new().await?;
    f.confirm().await?;
    let before = f
        .send(Method::POST, "client/initialization", Value::Null)
        .await?;
    assert_eq!(before.status, StatusCode::OK);
    // Fault injection only: fail the credential update after snapshot deletion.
    f.database
        .database
        .write(|tx| -> Result<(), PersistenceError> {
            tx.connection()
                .batch_execute(
                    "CREATE TRIGGER fail_photo_reset BEFORE UPDATE ON photo_sources
             BEGIN SELECT RAISE(ABORT, 'injected reset failure'); END;",
                )
                .map_err(PersistenceError::from)?;
            Ok(())
        })
        .await
        .map_err(crate::db::TransactionError::into_error)?;
    assert!(matches!(
        f.state
            .photo()
            .issue_code(f.source.clone(), PendingEnrollment::Replace)
            .await,
        Err(EnrollmentError::Persistence { .. })
    ));
    let after = f
        .send(Method::POST, "client/initialization", Value::Null)
        .await?;
    assert_eq!(after.status, StatusCode::OK);
    assert_eq!(before.body, after.body);
    Ok(())
}
