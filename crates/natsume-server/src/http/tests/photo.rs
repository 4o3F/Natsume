use natsume_photo_protocol::{PhotoConfirmation, PhotoPairRequest};
use std::sync::Arc;

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Method, Request, StatusCode, header},
};
use diesel::{ExpressionMethods as _, QueryDsl as _, RunQueryDsl as _};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use uuid::Uuid;

use super::{Captured, TestDatabase, drive, router, unused_web_root};
use crate::{
    component::photo::{CredentialUse, EnrollmentError, PendingEnrollment, SourceState},
    db::PersistenceError,
    diesel_schema::{
        account_mappings, accounts, organizations, photo_initializations, photo_sources, seats,
        teams,
    },
    http::AppState,
    server_state,
    test_error::TestError,
    tls::PeerAddress,
};

type TestResult = Result<(), TestError>;

mod client_interop;
mod recovery;

#[tokio::test]
async fn source_path_rejections_use_json_errors_without_changing_authority() -> TestResult {
    use super::{
        LOGIN_NAME, PASSWORD, PasswordVerificationTestGuard, header_text, login_request,
        seed_operator,
    };
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = Fixture::new().await?;
    seed_operator(&f.database.database, LOGIN_NAME, PASSWORD).await?;
    let login = drive(&f.app, login_request(LOGIN_NAME, PASSWORD)?).await?;
    let cookie = header_text(&login.headers, &header::SET_COOKIE)?
        .split(';')
        .next()
        .ok_or(TestError::missing("administrator cookie"))?;
    let before = serde_json::to_value(f.state.photo().list_sources().await?)?;
    for (action, body) in [
        ("pairing-code", json!({})),
        ("reset-enrollment", json!({})),
        ("state", json!({"state":"disabled"})),
    ] {
        for id in [
            "invalid-photo-path-canary",
            "550e8400-e29b-41d4-a716-446655440000",
            "%FF",
        ] {
            let path = format!("sources/{id}/{action}");
            let unauthorized = f.send(Method::POST, &path, body.clone()).await?;
            assert_eq!(unauthorized.status, StatusCode::UNAUTHORIZED);
            assert_eq!(
                serde_json::from_slice::<Value>(&unauthorized.body)?["code"],
                "AUTHENTICATION_FAILED"
            );
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
            assert_eq!(response.status, StatusCode::BAD_REQUEST);
            assert_eq!(response.headers[header::CONTENT_TYPE], "application/json");
            assert_eq!(
                serde_json::from_slice::<Value>(&response.body)?,
                json!({"title":"Bad Request", "status":400, "code":"INVALID_REQUEST"})
            );
        }
        let missing = natsume_photo_protocol::SourceId::generate();
        let response = drive(
            &f.app,
            Request::builder()
                .method(Method::POST)
                .uri(format!("/api/v2/photo/sources/{missing}/{action}"))
                .header(header::COOKIE, cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))?,
        )
        .await?;
        assert_eq!(response.status, StatusCode::NOT_FOUND);
        assert_eq!(response.headers[header::CONTENT_TYPE], "application/json");
        assert_eq!(
            serde_json::from_slice::<Value>(&response.body)?["code"],
            "RESOURCE_NOT_FOUND"
        );
    }
    assert_eq!(
        serde_json::to_value(f.state.photo().list_sources().await?)?,
        before
    );
    f.state
        .photo()
        .check_token(&f.token, CredentialUse::Initialization)
        .await?;
    Ok(())
}

#[tokio::test]
async fn malformed_resource_paths_use_json_errors_before_bearer_validation() -> TestResult {
    let f = Fixture::new().await?;
    for with_token in [false, true] {
        let request = Request::builder().uri("/api/v2/photo/client/resources/%FFphoto-path-canary");
        let request = if with_token {
            request.header(header::AUTHORIZATION, format!("Bearer {}", f.token))
        } else {
            request
        };
        let response = drive(&f.app, request.body(Body::empty())?).await?;
        assert_eq!(response.status, StatusCode::BAD_REQUEST);
        assert_eq!(response.headers[header::CONTENT_TYPE], "application/json");
        assert_eq!(
            serde_json::from_slice::<Value>(&response.body)?,
            json!({"title":"Bad Request", "status":400, "code":"INVALID_REQUEST"})
        );
    }
    f.state
        .photo()
        .check_token(&f.token, CredentialUse::Initialization)
        .await?;
    Ok(())
}

#[tokio::test]
async fn invalid_server_roster_is_a_recoverable_conflict_and_cannot_publish_a_snapshot()
-> TestResult {
    let f = Fixture::new().await?;
    f.confirm().await?;
    f.database
        .database
        .write(|tx| -> Result<(), PersistenceError> {
            diesel::update(accounts::table.filter(accounts::account_id.eq("account-a")))
                .set(accounts::domjudge_username.eq(""))
                .execute(tx.connection())?;
            Ok(())
        })
        .await
        .map_err(crate::db::TransactionError::into_error)?;
    let response = f
        .send(Method::POST, "client/initialization", Value::Null)
        .await?;
    assert_eq!(response.status, StatusCode::CONFLICT);
    let published = f
        .database
        .database
        .read(|tx| -> Result<i64, PersistenceError> {
            Ok(photo_initializations::table
                .count()
                .get_result(tx.connection())?)
        })
        .await
        .map_err(crate::db::TransactionError::into_error)?;
    assert_eq!(published, 0);
    f.database
        .database
        .write(|tx| -> Result<(), PersistenceError> {
            diesel::update(accounts::table.filter(accounts::account_id.eq("account-a")))
                .set(accounts::domjudge_username.eq("account-a"))
                .execute(tx.connection())?;
            Ok(())
        })
        .await
        .map_err(crate::db::TransactionError::into_error)?;
    assert_eq!(
        f.send(Method::POST, "client/initialization", Value::Null)
            .await?
            .status,
        StatusCode::OK
    );
    Ok(())
}

#[tokio::test]
async fn invalid_persisted_source_state_cannot_be_silently_reenabled() -> TestResult {
    let f = Fixture::new().await?;
    f.database
        .database
        .write(|tx| -> Result<(), PersistenceError> {
            diesel::update(photo_sources::table)
                .set(photo_sources::state.eq("unknown"))
                .execute(tx.connection())?;
            Ok(())
        })
        .await
        .map_err(crate::db::TransactionError::into_error)?;
    assert!(matches!(
        f.state
            .photo()
            .set_state(f.source, SourceState::Enabled)
            .await,
        Err(EnrollmentError::Persistence { .. })
    ));
    let state = f
        .database
        .database
        .read(|tx| -> Result<String, PersistenceError> {
            Ok(photo_sources::table
                .select(photo_sources::state)
                .first(tx.connection())?)
        })
        .await
        .map_err(crate::db::TransactionError::into_error)?;
    assert_eq!(state, "unknown");
    Ok(())
}

#[tokio::test]
async fn invalid_persisted_confirmation_is_rejected_before_authentication() -> TestResult {
    let f = Fixture::new().await?;
    for confirmed in [-1, 2] {
        f.database
            .database
            .write(move |tx| -> Result<(), PersistenceError> {
                diesel::update(photo_sources::table)
                    .set(photo_sources::confirmed.eq(confirmed))
                    .execute(tx.connection())?;
                Ok(())
            })
            .await
            .map_err(crate::db::TransactionError::into_error)?;
        assert!(matches!(
            f.state
                .photo()
                .check_token(&f.token, CredentialUse::Initialization)
                .await,
            Err(EnrollmentError::Persistence { .. })
        ));
        assert_eq!(
            f.send(
                Method::POST,
                "enrollment/confirm",
                json!({"enrollment_id":f.enrollment, "signing_public_key":f.key})
            )
            .await?
            .status,
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }
    Ok(())
}

struct Fixture {
    database: TestDatabase,
    logos: tempfile::TempDir,
    state: AppState,
    app: axum::Router,
    source: natsume_photo_protocol::SourceId,
    token: String,
    enrollment: String,
    key: natsume_photo_protocol::SigningPublicKey,
}

impl Fixture {
    async fn new() -> Result<Self, TestError> {
        let database = TestDatabase::new().await?;
        let logos = tempfile::TempDir::new()?;
        image::RgbaImage::from_pixel(8, 4, image::Rgba([255, 0, 0, 255]))
            .save(logos.path().join("School.png"))?;
        database
            .database
            .write(|tx| -> Result<(), PersistenceError> {
                diesel::insert_into(organizations::table)
                    .values((
                        organizations::organization_id.eq(7_i64),
                        organizations::name_key.eq("school"),
                        organizations::name_zh.eq("学校"),
                        organizations::name_en.eq("School"),
                        organizations::country.eq("CHN"),
                    ))
                    .execute(tx.connection())?;
                for (account, seat) in [("account-a", "seat-a"), ("account-b", "seat-b")] {
                    diesel::insert_into(accounts::table)
                        .values((
                            accounts::account_id.eq(account),
                            accounts::domjudge_username.eq(account),
                            accounts::credential_revision.eq(3_i64),
                        ))
                        .execute(tx.connection())?;
                    diesel::insert_into(teams::table)
                        .values((
                            teams::account_id.eq(account),
                            teams::organization_id.eq(7_i64),
                            teams::name_zh.eq("队伍"),
                            teams::name_en.eq("Team"),
                            teams::category.eq("participant"),
                        ))
                        .execute(tx.connection())?;
                    diesel::insert_into(seats::table)
                        .values((seats::seat_id.eq(seat), seats::seat_code.eq(seat)))
                        .execute(tx.connection())?;
                    diesel::insert_into(account_mappings::table)
                        .values((
                            account_mappings::seat_id.eq(seat),
                            account_mappings::account_id.eq(account),
                        ))
                        .execute(tx.connection())?;
                }
                Ok(())
            })
            .await
            .map_err(crate::db::TransactionError::into_error)?;
        let state = Arc::new(server_state::tests::for_test_with_logos(
            database.database.clone(),
            database.documents_path(),
            logos.path().to_path_buf(),
        )?);
        let source = state
            .photo()
            .create_source("Photo desk".into())
            .await?
            .source_id;
        let code = state
            .photo()
            .issue_code(source.clone(), PendingEnrollment::Preserve)
            .await?;
        let key: natsume_photo_protocol::SigningPublicKey = hex::encode(
            ed25519_dalek::SigningKey::from_bytes(&[17; 32])
                .verifying_key()
                .to_bytes(),
        )
        .parse()
        .unwrap_or_else(|error| panic!("generated non-weak public key: {error:?}"));
        let issued = state
            .photo()
            .pair(PhotoPairRequest {
                code: code.code,
                signing_public_key: key.clone(),
                client_version: "2.11.0".into(),
            })
            .await?;
        let app = router(state.clone(), unused_web_root());
        Ok(Self {
            database,
            logos,
            state,
            app,
            source,
            token: issued.token.clone(),
            enrollment: issued.enrollment_id.to_string(),
            key,
        })
    }

    async fn send(&self, method: Method, path: &str, body: Value) -> Result<Captured, TestError> {
        Ok(drive(
            &self.app,
            Request::builder()
                .method(method)
                .uri(format!("/api/v2/photo/{path}"))
                .header(header::AUTHORIZATION, format!("Bearer {}", self.token))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))?,
        )
        .await?)
    }

    async fn confirm(&self) -> TestResult {
        let body = json!({"enrollment_id": self.enrollment, "signing_public_key": self.key});
        for _ in 0..2 {
            assert_eq!(
                self.send(Method::POST, "enrollment/confirm", body.clone())
                    .await?
                    .status,
                StatusCode::OK
            );
        }
        Ok(())
    }
}

#[tokio::test]
async fn immutable_roster_and_logo_survive_restart() -> TestResult {
    let mut f = Fixture::new().await?;
    assert_eq!(
        f.send(Method::POST, "client/initialization", Value::Null)
            .await?
            .status,
        StatusCode::FORBIDDEN
    );
    f.confirm().await?;
    let first = f
        .send(Method::POST, "client/initialization", Value::Null)
        .await?;
    assert_eq!(first.status, StatusCode::OK);
    assert_eq!(first.headers[header::CACHE_CONTROL], "no-store");
    let manifest: Value = serde_json::from_slice(&first.body)?;
    assert_eq!(manifest["teams"].as_array().map(Vec::len), Some(2));
    for team in manifest["teams"]
        .as_array()
        .ok_or(TestError::missing("teams"))?
    {
        assert_eq!(team.as_object().map(serde_json::Map::len), Some(7));
        assert_eq!(team["organization_id"], 7);
        for forbidden in [
            "seat",
            "seat_id",
            "seat_code",
            "password",
            "credential_revision",
        ] {
            assert!(team.get(forbidden).is_none());
        }
    }
    assert_eq!(manifest["school_logos"].as_array().map(Vec::len), Some(1));
    let hash = manifest["school_logos"][0]["sha256"]
        .as_str()
        .ok_or(TestError::missing("hash"))?;
    let resource_path = format!("client/resources/{hash}");
    let image = f.send(Method::GET, &resource_path, Value::Null).await?;
    assert_eq!(image.status, StatusCode::OK);
    assert_eq!(image.headers[header::CONTENT_TYPE], "image/png");
    assert_eq!(hex::encode(Sha256::digest(&image.body)), hash);
    let decoded = image::load_from_memory(&image.body)?;
    assert_eq!((decoded.width(), decoded.height()), (8, 4));
    assert_eq!(
        f.send(
            Method::GET,
            &format!("client/resources/{}", "0".repeat(64)),
            Value::Null
        )
        .await?
        .status,
        StatusCode::NOT_FOUND
    );
    // Change live input, then reconstruct the complete Server state.
    std::fs::remove_file(f.logos.path().join("School.png"))?;
    f.database
        .database
        .write(|tx| -> Result<(), PersistenceError> {
            diesel::update(teams::table)
                .set(teams::name_en.eq("Changed"))
                .execute(tx.connection())?;
            Ok(())
        })
        .await
        .map_err(crate::db::TransactionError::into_error)?;
    f.state = Arc::new(server_state::tests::for_test_with_logos(
        f.database.database.clone(),
        f.database.documents_path(),
        f.logos.path().to_path_buf(),
    )?);
    f.app = router(f.state.clone(), unused_web_root());
    assert_eq!(
        f.send(Method::POST, "client/initialization", Value::Null)
            .await?
            .body,
        first.body
    );
    assert_eq!(
        f.send(Method::GET, &resource_path, Value::Null).await?.body,
        image.body
    );
    Ok(())
}

#[tokio::test]
async fn completion_closes_downloads_and_only_replays_the_same_receipt() -> TestResult {
    let f = Fixture::new().await?;
    f.confirm().await?;
    let first = f
        .send(Method::POST, "client/initialization", Value::Null)
        .await?;
    let manifest: Value = serde_json::from_slice(&first.body)?;
    let hash = manifest["school_logos"][0]["sha256"]
        .as_str()
        .ok_or(TestError::missing("hash"))?;
    let resource_path = format!("client/resources/{hash}");
    let digest = hex::encode(Sha256::digest(&first.body));
    assert_eq!(first.headers[header::ETAG], format!("\"{digest}\""));
    let mut receipt = json!({"initialization_id":manifest["initialization_id"], "manifest_sha256": "00".repeat(32), "request_id":Uuid::now_v7().to_string()});
    assert_eq!(
        f.send(
            Method::POST,
            "client/initialization/complete",
            receipt.clone()
        )
        .await?
        .status,
        StatusCode::CONFLICT
    );
    receipt["manifest_sha256"] = json!(digest);
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
    f.database
        .database
        .write(|tx| -> Result<(), PersistenceError> {
            diesel::update(photo_sources::table)
                .set(photo_sources::token_expires_at_unix_ms.eq(0_i64))
                .execute(tx.connection())?;
            Ok(())
        })
        .await
        .map_err(crate::db::TransactionError::into_error)?;
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
    receipt["request_id"] = json!(Uuid::now_v7().to_string());
    assert_eq!(
        f.send(Method::POST, "client/initialization/complete", receipt)
            .await?
            .status,
        StatusCode::CONFLICT
    );
    assert_eq!(
        f.send(Method::POST, "client/initialization", Value::Null)
            .await?
            .status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.send(Method::GET, &resource_path, Value::Null)
            .await?
            .status,
        StatusCode::UNAUTHORIZED
    );
    Ok(())
}

#[tokio::test]
async fn enrollment_recovery_expiry_and_revocation_are_explicit() -> TestResult {
    let f = Fixture::new().await?;
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
        .await?;
    assert_eq!(
        f.send(Method::POST, "enrollment/confirm", json!({}))
            .await?
            .status,
        StatusCode::UNAUTHORIZED
    );
    let (a, b) = tokio::join!(
        f.state.photo().pair(PhotoPairRequest {
            code: replacement.code.clone(),
            signing_public_key: f.key.clone(),
            client_version: "2.11.0".into()
        }),
        f.state.photo().pair(PhotoPairRequest {
            code: replacement.code,
            signing_public_key: f.key.clone(),
            client_version: "2.11.0".into()
        })
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    let issued = a.or(b)?;
    f.state
        .photo()
        .confirm(
            &issued.token,
            PhotoConfirmation {
                enrollment_id: issued.enrollment_id.clone(),
                signing_public_key: f.key.clone(),
            },
        )
        .await?;
    assert!(matches!(
        f.state
            .photo()
            .issue_code(f.source.clone(), PendingEnrollment::Preserve)
            .await,
        Err(EnrollmentError::Conflict)
    ));
    f.state
        .photo()
        .set_state(f.source.clone(), SourceState::Disabled)
        .await?;
    assert!(
        f.state
            .photo()
            .check_token(&issued.token.clone(), CredentialUse::Initialization)
            .await
            .is_err()
    );
    f.state
        .photo()
        .set_state(f.source.clone(), SourceState::Enabled)
        .await?;
    f.state
        .photo()
        .check_token(&issued.token.clone(), CredentialUse::Initialization)
        .await?;
    f.database
        .database
        .write(|tx| -> Result<(), PersistenceError> {
            diesel::update(photo_sources::table)
                .set(photo_sources::token_expires_at_unix_ms.eq(0_i64))
                .execute(tx.connection())?;
            Ok(())
        })
        .await
        .map_err(crate::db::TransactionError::into_error)?;
    assert!(
        f.state
            .photo()
            .check_token(&issued.token.clone(), CredentialUse::Initialization)
            .await
            .is_err()
    );
    f.state
        .photo()
        .set_state(f.source.clone(), SourceState::Revoked)
        .await?;
    assert!(matches!(
        f.state
            .photo()
            .set_state(f.source.clone(), SourceState::Enabled)
            .await,
        Err(EnrollmentError::Conflict)
    ));
    Ok(())
}

#[tokio::test]
async fn expired_pairing_code_cannot_be_redeemed() -> TestResult {
    let f = Fixture::new().await?;
    let source = f
        .state
        .photo()
        .create_source("Expired".into())
        .await?
        .source_id;
    let code = f
        .state
        .photo()
        .issue_code(source.clone(), PendingEnrollment::Preserve)
        .await?;
    f.database
        .database
        .write(|tx| -> Result<(), PersistenceError> {
            diesel::update(photo_sources::table)
                .set(photo_sources::pairing_expires_at_unix_ms.eq(0_i64))
                .execute(tx.connection())?;
            Ok(())
        })
        .await
        .map_err(crate::db::TransactionError::into_error)?;
    assert!(matches!(
        f.state
            .photo()
            .pair(PhotoPairRequest {
                code: code.code,
                signing_public_key: f.key,
                client_version: "2.11.0".into()
            })
            .await,
        Err(EnrollmentError::Authentication)
    ));
    Ok(())
}

#[tokio::test]
async fn protected_routes_authenticate_before_body_and_reject_other_credentials() -> TestResult {
    let f = Fixture::new().await?;
    for path in ["enrollment/confirm", "client/initialization/complete"] {
        for credential in [None, Some("npc1.invalid-print-token"), Some("invalid")] {
            let mut request = Request::builder()
                .method(Method::POST)
                .uri(format!("/api/v2/photo/{path}"));
            if let Some(token) = credential {
                request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
            }
            let response = drive(&f.app, request.body(Body::from("malformed"))?).await?;
            assert_eq!(response.status, StatusCode::UNAUTHORIZED);
        }
    }
    // Pairing is the only client operation that does not require a token.
    let response = drive(&f.app, Request::builder().method(Method::POST).uri("/api/v2/photo/enrollment/pair").extension(ConnectInfo(PeerAddress("192.0.2.1".parse()?))).header(header::CONTENT_TYPE, "application/json").body(Body::from(json!({"code":"ABCD-EFGH", "signing_public_key":"00".repeat(32), "client_version":"2.11.0"}).to_string()))?).await?;
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    for _ in 0..5 {
        f.state.photo().admit_pairing("192.0.2.2".parse()?)?;
    }
    assert!(matches!(
        f.state.photo().admit_pairing("192.0.2.2".parse()?),
        Err(EnrollmentError::RateLimited)
    ));
    Ok(())
}

#[tokio::test]
async fn missing_logos_are_optional_and_corrupt_cache_is_rejected() -> TestResult {
    let f = Fixture::new().await?;
    std::fs::remove_file(f.logos.path().join("School.png"))?;
    f.confirm().await?;
    let snapshot = f
        .send(Method::POST, "client/initialization", Value::Null)
        .await?;
    assert_eq!(snapshot.status, StatusCode::OK);
    let value: Value = serde_json::from_slice(&snapshot.body)?;
    assert_eq!(value["school_logos"], json!([]));
    f.database
        .database
        .write(|tx| -> Result<(), PersistenceError> {
            diesel::update(photo_initializations::table)
                .set(photo_initializations::manifest_json.eq("{}"))
                .execute(tx.connection())?;
            Ok(())
        })
        .await
        .map_err(crate::db::TransactionError::into_error)?;
    assert_eq!(
        f.send(Method::POST, "client/initialization", Value::Null)
            .await?
            .status,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    Ok(())
}

#[tokio::test]
async fn failed_storage_does_not_publish_a_snapshot() -> TestResult {
    let mut f = Fixture::new().await?;
    f.confirm().await?;
    let root = f
        .database
        .documents_path()
        .with_extension("photo-initializations");
    std::fs::remove_dir(&root)?;
    std::fs::write(&root, b"not a directory")?;
    assert_eq!(
        f.send(Method::POST, "client/initialization", Value::Null)
            .await?
            .status,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    let count = f
        .database
        .database
        .read(|tx| -> Result<i64, PersistenceError> {
            Ok(photo_initializations::table
                .count()
                .get_result(tx.connection())?)
        })
        .await
        .map_err(crate::db::TransactionError::into_error)?;
    assert_eq!(count, 0);
    std::fs::remove_file(root)?;
    // The store pins the validated directory handle. Restore it through the
    // normal startup boundary, never by following a replacement pathname.
    f.state = Arc::new(server_state::tests::for_test_with_logos(
        f.database.database.clone(),
        f.database.documents_path(),
        f.logos.path().to_path_buf(),
    )?);
    f.app = router(f.state.clone(), unused_web_root());
    assert_eq!(
        f.send(Method::POST, "client/initialization", Value::Null)
            .await?
            .status,
        StatusCode::OK
    );
    Ok(())
}

#[tokio::test]
async fn secret_storage_and_resource_ownership_are_scoped() -> TestResult {
    let f = Fixture::new().await?;
    let stored = f
        .database
        .database
        .read(|tx| -> Result<_, PersistenceError> {
            Ok(photo_sources::table
                .select((photo_sources::token_hash, photo_sources::pairing_code_hash))
                .first::<(Option<Vec<u8>>, Option<Vec<u8>>)>(tx.connection())?)
        })
        .await
        .map_err(crate::db::TransactionError::into_error)?;
    assert_eq!(stored.0, Some(Sha256::digest(f.token.as_bytes()).to_vec()));
    assert!(stored.1.is_none());
    f.confirm().await?;
    let snapshot = f
        .send(Method::POST, "client/initialization", Value::Null)
        .await?;
    let manifest: Value = serde_json::from_slice(&snapshot.body)?;
    let hash = manifest["school_logos"][0]["sha256"]
        .as_str()
        .ok_or(TestError::missing("hash"))?;
    let source = f
        .state
        .photo()
        .create_source("Other desk".into())
        .await?
        .source_id;
    let code = f
        .state
        .photo()
        .issue_code(source.clone(), PendingEnrollment::Preserve)
        .await?;
    let other = f
        .state
        .photo()
        .pair(PhotoPairRequest {
            code: code.code,
            signing_public_key: f.key.clone(),
            client_version: "2.11.0".into(),
        })
        .await?;
    f.state
        .photo()
        .confirm(
            &other.token,
            PhotoConfirmation {
                enrollment_id: other.enrollment_id.clone(),
                signing_public_key: f.key.clone(),
            },
        )
        .await?;
    assert!(matches!(
        f.state.photo().resource(&other.token, hash).await,
        Err(crate::component::photo::InitializationError::NotFound)
    ));
    let id = manifest["initialization_id"]
        .as_str()
        .ok_or(TestError::missing("initialization"))?;
    std::fs::write(
        f.database
            .documents_path()
            .with_extension("photo-initializations")
            .join(id)
            .join(format!("{hash}.png")),
        b"corrupt",
    )?;
    assert_eq!(
        f.send(
            Method::GET,
            &format!("client/resources/{hash}"),
            Value::Null
        )
        .await?
        .status,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    Ok(())
}

#[tokio::test]
async fn admin_routes_and_public_pairing_form_a_complete_boundary() -> TestResult {
    use super::{
        LOGIN_NAME, PASSWORD, PasswordVerificationTestGuard, header_text, login_request,
        seed_operator,
    };
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = Fixture::new().await?;
    seed_operator(&f.database.database, LOGIN_NAME, PASSWORD).await?;
    let login = drive(&f.app, login_request(LOGIN_NAME, PASSWORD)?).await?;
    let cookie = header_text(&login.headers, &header::SET_COOKIE)?
        .split(';')
        .next()
        .ok_or(TestError::missing("cookie"))?;
    let admin = |path: &str, body: Value| -> Result<_, axum::http::Error> {
        Request::builder()
            .method(Method::POST)
            .uri(format!("/api/v2/photo/{path}"))
            .header(header::COOKIE, cookie)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
    };
    assert_eq!(
        drive(
            &f.app,
            admin("sources", json!({"name":"New desk", "unknown":true}))?
        )
        .await?
        .status,
        StatusCode::BAD_REQUEST
    );
    let response = drive(&f.app, admin("sources", json!({"name":"New desk"}))?).await?;
    assert_eq!(response.status, StatusCode::CREATED);
    let value: Value = serde_json::from_slice(&response.body)?;
    let source = value["source_id"]
        .as_str()
        .ok_or(TestError::missing("source"))?;
    let response = drive(
        &f.app,
        admin(&format!("sources/{source}/pairing-code"), json!({}))?,
    )
    .await?;
    assert_eq!(response.status, StatusCode::OK);
    let value: Value = serde_json::from_slice(&response.body)?;
    let pair_body =
        json!({"code": value["code"], "signing_public_key":f.key, "client_version":"2.11.0"});
    let peer = "192.0.2.3".parse()?;
    let pair_request = || {
        Request::builder()
            .method(Method::POST)
            .uri("/api/v2/photo/enrollment/pair")
            .extension(ConnectInfo(PeerAddress(peer)))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(pair_body.to_string()))
    };
    assert_eq!(
        drive(&f.app, pair_request()?).await?.status,
        StatusCode::CREATED
    );
    assert_eq!(
        drive(&f.app, pair_request()?).await?.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        drive(
            &f.app,
            admin(
                &format!("sources/{source}/state"),
                json!({"state":"disabled"})
            )?
        )
        .await?
        .status,
        StatusCode::NO_CONTENT
    );
    let list = drive(
        &f.app,
        Request::builder()
            .uri("/api/v2/photo/sources")
            .header(header::COOKIE, cookie)
            .body(Body::empty())?,
    )
    .await?;
    assert_eq!(list.status, StatusCode::OK);
    let values: Vec<Value> = serde_json::from_slice(&list.body)?;
    assert_eq!(values.len(), 2);
    assert!(
        values
            .iter()
            .all(|v| v.get("token").is_none() && v.get("public_key").is_none())
    );
    Ok(())
}

#[tokio::test]
async fn snapshot_publication_is_rejected_after_shutdown_admission_closes() -> TestResult {
    let f = Fixture::new().await?;
    f.confirm().await?;
    f.state.lifecycle().begin_shutdown();
    let response = f
        .send(Method::POST, "client/initialization", Value::Null)
        .await?;
    assert_eq!(response.status, StatusCode::SERVICE_UNAVAILABLE);
    f.state.lifecycle().wait().await;
    Ok(())
}

#[tokio::test]
async fn snapshot_resource_rejects_symlinks_even_with_matching_bytes() -> TestResult {
    let f = Fixture::new().await?;
    f.confirm().await?;
    let response = f
        .send(Method::POST, "client/initialization", Value::Null)
        .await?;
    let manifest: Value = serde_json::from_slice(&response.body)?;
    let id = manifest["initialization_id"]
        .as_str()
        .ok_or(TestError::missing("initialization id"))?;
    let hash = manifest["school_logos"][0]["sha256"]
        .as_str()
        .ok_or(TestError::missing("logo hash"))?;
    let file = f
        .database
        .documents_path()
        .with_extension("photo-initializations")
        .join(id)
        .join(format!("{hash}.png"));
    let outside = f.logos.path().join("outside.png");
    std::fs::write(&outside, std::fs::read(&file)?)?;
    std::fs::remove_file(&file)?;
    std::os::unix::fs::symlink(outside, file)?;
    let response = f
        .send(
            Method::GET,
            &format!("client/resources/{hash}"),
            Value::Null,
        )
        .await?;
    assert_eq!(response.status, StatusCode::INTERNAL_SERVER_ERROR);
    Ok(())
}

#[tokio::test]
async fn invalid_persisted_photo_identity_preserves_the_value_error() -> TestResult {
    use std::error::Error as _;
    let f = Fixture::new().await?;
    f.database
        .database
        .write(|tx| -> Result<(), PersistenceError> {
            diesel::update(photo_sources::table)
                .set(photo_sources::enrollment_id.eq("invalid-enrollment"))
                .execute(tx.connection())?;
            Ok(())
        })
        .await
        .map_err(crate::db::TransactionError::into_error)?;
    let error = f
        .state
        .photo()
        .list_sources()
        .await
        .err()
        .unwrap_or_else(|| panic!("invalid persisted identity"));
    let mut source = error.source();
    let mut found = false;
    while let Some(current) = source {
        found |= current.is::<natsume_photo_protocol::ValueError>();
        source = current.source();
    }
    assert!(
        found,
        "retain protocol validation source through the persistence boundary"
    );
    assert_eq!(
        f.send(
            Method::POST,
            "enrollment/confirm",
            json!({"enrollment_id": f.enrollment, "signing_public_key": f.key})
        )
        .await?
        .status,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    Ok(())
}
