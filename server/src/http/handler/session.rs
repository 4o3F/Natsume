use axum::{
    Extension, Json, Router,
    extract::{DefaultBodyLimit, State, rejection::JsonRejection},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{delete, post},
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{
    application::operator::{self, OperatorIdentity},
    audit::CorrelationId,
};

use super::super::{AppState, cookie, error::ApiError, middleware};

const SESSION_REQUEST_BODY_LIMIT_BYTES: usize = 4_096;

pub(in crate::http) fn public_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/session",
            post(create_session).layer(DefaultBodyLimit::max(SESSION_REQUEST_BODY_LIMIT_BYTES)),
        )
        .route("/session", delete(delete_session))
}

pub(in crate::http) fn protected_routes(state: AppState) -> Router<AppState> {
    Router::new().route("/session", middleware::operator_get(state, read_session))
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SessionRequest {
    login_name: String,
    #[schema(write_only)]
    password: String,
}

#[derive(Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SessionResponse {
    operator_id: Uuid,
    role: &'static str,
}

#[utoipa::path(
    post,
    path = "/api/v2/session",
    operation_id = "createSession",
    request_body = SessionRequest,
    responses(
        (status = 200, description = "Session established", body = SessionResponse),
        (status = 400, description = "Invalid closed request"),
        (status = 401, description = "Authentication failed"),
        (status = 413, description = "Request body exceeds the session ingress limit"),
        (status = 500, description = "Internal failure")
    )
)]
pub(crate) async fn create_session(
    State(state): State<AppState>,
    Extension(correlation_id): Extension<CorrelationId>,
    request: Result<Json<SessionRequest>, JsonRejection>,
) -> Response {
    let Json(request) = match request {
        Ok(request) => request,
        Err(rejection) if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE => {
            return rejection.into_response();
        }
        Err(_) => {
            return ApiError::invalid_request("session_request_body_rejected", correlation_id)
                .into_response();
        }
    };

    let signed_in = match operator::sign_in(
        &state.database,
        correlation_id,
        &request.login_name,
        request.password,
    )
    .await
    {
        Ok(signed_in) => signed_in,
        Err(error) => return ApiError::from_operator(error, correlation_id).into_response(),
    };
    let wire_credential = signed_in.credential().to_wire();
    let Ok(session_cookie) = cookie::issue_session_credential(wire_credential.expose()) else {
        return ApiError::internal_error("session_cookie_issuance_failed", correlation_id)
            .into_response();
    };
    identity_response(signed_in.identity(), Some(session_cookie), correlation_id)
}

#[utoipa::path(
    get,
    path = "/api/v2/session",
    operation_id = "getSession",
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "Current session", body = SessionResponse),
        (status = 401, description = "Session authentication failed"),
        (status = 500, description = "Internal failure")
    )
)]
pub(crate) async fn read_session(
    Extension(identity): Extension<OperatorIdentity>,
    Extension(correlation_id): Extension<CorrelationId>,
) -> Response {
    identity_response(identity, None, correlation_id)
}

#[utoipa::path(
    delete,
    path = "/api/v2/session",
    operation_id = "deleteSession",
    security(("sessionCookie" = [])),
    responses(
        (status = 204, description = "Session terminated or credential-state no-op"),
        (status = 500, description = "Session termination infrastructure failure")
    )
)]
pub(crate) async fn delete_session(
    State(state): State<AppState>,
    Extension(correlation_id): Extension<CorrelationId>,
    headers: HeaderMap,
) -> Response {
    let response = match cookie::session_credential(&headers) {
        Ok(wire_credential) => {
            match operator::terminate_session(&state.database, correlation_id, wire_credential)
                .await
            {
                Ok(()) => StatusCode::NO_CONTENT.into_response(),
                Err(error) => ApiError::from_operator(error, correlation_id).into_response(),
            }
        }
        Err(()) => StatusCode::NO_CONTENT.into_response(),
    };
    cookie::with_clearing_session_cookie(response).unwrap_or_else(|()| {
        ApiError::internal_error("session_clearing_cookie_failed", correlation_id).into_response()
    })
}

fn identity_response(
    identity: OperatorIdentity,
    session_cookie: Option<HeaderValue>,
    correlation_id: CorrelationId,
) -> Response {
    let body = SessionResponse {
        operator_id: identity.operator_id(),
        role: identity.role().as_persisted(),
    };
    let encoded = serde_json::to_string(&body).unwrap_or_else(|_| {
        tracing::error!(
            correlation_id = %correlation_id.as_text(),
            "session response serialization invariant failed"
        );
        panic!("session response serialization invariant failed");
    });
    let mut response = (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        encoded,
    )
        .into_response();
    if let Some(session_cookie) = session_cookie {
        response
            .headers_mut()
            .insert(header::SET_COOKIE, session_cookie);
    }
    response
}

#[cfg(test)]
mod tests {
    use std::{
        net::{IpAddr, Ipv4Addr, SocketAddr},
        time::Duration,
    };

    use axum::{
        body::Body,
        http::{Method, Request, StatusCode, header},
        serve::Listener,
    };
    use cookie::Cookie;
    use diesel::{
        ExpressionMethods, QueryDsl, QueryableByName, RunQueryDsl, sql_types::BigInt,
        sqlite::SqliteConnection,
    };
    use serde_json::Value;
    use snafu::Snafu;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        sync::oneshot,
        time::timeout,
    };
    use uuid::Uuid;

    use crate::{
        application::operator::{OperatorRole, tests::PasswordVerificationTestGuard},
        db::{
            Database, DatabaseConfig, operator as db_operator,
            schema::{audit_events, operator_accounts, operator_sessions},
            tests::{test_data_version, test_lock_database, test_observer},
        },
        tls::{
            TlsListener,
            tests::{TestIdentity, connect_test_client},
        },
    };

    use super::super::super::{
        middleware::CORRELATION_ID_HEADER,
        router,
        tests::{
            Captured, SupportFailure, TestDatabase, canonical_correlation_id, check_error_response,
            cookie_request, drive, header_text, login_request, normalized_error_response_body,
            request, response_body_text, response_contains, seed_operator, unused_web_root,
        },
    };

    const LOCALHOST: Ipv4Addr = Ipv4Addr::LOCALHOST;
    const TEST_TIMEOUT: Duration = Duration::from_secs(10);
    const LOGIN_NAME: &str = "http-admin";
    const PASSWORD: &str = "http-password-canary";

    #[tokio::test]
    async fn request_rejections_and_invalid_cookies_are_uniform_and_redacted()
    -> Result<(), TestFailure> {
        let fixture = TestDatabase::new().await?;
        let application = router(fixture.database.clone(), unused_web_root());
        for (content_type, body, canary) in [
            (
                "application/json",
                r#"{"login_name":"malformed-parser-canary","password":}"#,
                "malformed-parser-canary",
            ),
            (
                "application/json",
                r#"{"login_name":"unknown-field-login","password":"unknown-field-password","unexpected":"unknown-field-canary"}"#,
                "unknown-field-canary",
            ),
            (
                "application/json",
                r#"{"login_name":"missing-field-canary"}"#,
                "missing-field-canary",
            ),
            (
                "text/plain",
                r#"{"login_name":"wrong-content-login","password":"wrong-content-canary"}"#,
                "wrong-content-canary",
            ),
        ] {
            let request = Request::builder()
                .method(Method::POST)
                .uri("/api/v2/session")
                .header(header::CONTENT_TYPE, content_type)
                .body(Body::from(body))
                .map_err(|_| TestFailure::RequestBuildFailed)?;
            let response = drive(&application, request).await?;
            check_error_response(
                &response,
                StatusCode::BAD_REQUEST,
                "Bad Request",
                "INVALID_REQUEST",
            )?;
            let body = response_body_text(&response)?;
            if response_contains(&response, canary)
                || body.contains("unknown field")
                || body.contains("missing field")
                || body.contains("expected value")
            {
                return Err(TestFailure::ParserDetailEscaped);
            }
        }

        let missing = drive(&application, request(Method::GET, "/api/v2/session", "")?).await?;
        check_error_response(
            &missing,
            StatusCode::UNAUTHORIZED,
            "Unauthorized",
            "AUTHENTICATION_FAILED",
        )?;
        let normalized_missing = normalized_error_response_body(&missing)?;
        for cookie in [
            format!("__Secure-natsume_session={}", "A".repeat(64)),
            format!("__Secure-natsume_session={}", "a".repeat(63)),
            format!("__Secure-natsume_session={}", "g".repeat(64)),
            format!("__Secure-natsume_session={} ", "a".repeat(64)),
            format!(
                "__Secure-natsume_session={0}; __Secure-natsume_session={0}",
                "a".repeat(64)
            ),
        ] {
            let request = Request::builder()
                .method(Method::GET)
                .uri("/api/v2/session")
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .map_err(|_| TestFailure::RequestBuildFailed)?;
            let response = drive(&application, request).await?;
            check_error_response(
                &response,
                StatusCode::UNAUTHORIZED,
                "Unauthorized",
                "AUTHENTICATION_FAILED",
            )?;
            if normalized_error_response_body(&response)? != normalized_missing {
                return Err(TestFailure::CredentialFailuresWereDistinguishable);
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn http_session_lifecycle_preserves_cookie_audit_and_noop_contracts()
    -> Result<(), TestFailure> {
        let _verification_guard = PasswordVerificationTestGuard::acquire().await;
        let fixture = TestDatabase::new().await?;
        let operator_id =
            seed_operator(&fixture.database, LOGIN_NAME, OperatorRole::Admin, PASSWORD).await?;
        let application = router(fixture.database.clone(), unused_web_root());

        let login = login_request(LOGIN_NAME, PASSWORD)?;
        let login_response = drive(&application, login).await?;
        if login_response.status != StatusCode::OK {
            return Err(TestFailure::ValidLoginFailed);
        }
        let cookie_pair =
            validate_login_response(&fixture.database, &login_response, operator_id).await?;

        for duplicate_cookie in duplicated_live_cookie_headers(&cookie_pair)? {
            let duplicate_response = drive(
                &application,
                cookie_request(Method::GET, "/api/v2/session", &duplicate_cookie)?,
            )
            .await?;
            if duplicate_response.status != StatusCode::UNAUTHORIZED {
                return Err(TestFailure::DuplicateCookieWasAccepted);
            }
            check_error_response(
                &duplicate_response,
                StatusCode::UNAUTHORIZED,
                "Unauthorized",
                "AUTHENTICATION_FAILED",
            )?;
        }

        let expiry_before = session_expiry(&fixture.database).await?;
        let get_response = drive(
            &application,
            cookie_request(Method::GET, "/api/v2/session", &cookie_pair)?,
        )
        .await?;
        if get_response.status != StatusCode::OK
            || get_response.body != login_response.body
            || get_response.headers.contains_key(header::SET_COOKIE)
            || session_expiry(&fixture.database).await? != expiry_before
        {
            return Err(TestFailure::SessionReadChangedState);
        }

        let delete_response = drive(
            &application,
            cookie_request(Method::DELETE, "/api/v2/session", &cookie_pair)?,
        )
        .await?;
        if delete_response.status != StatusCode::NO_CONTENT
            || !delete_response.body.is_empty()
            || !session_cookie_value(&delete_response, 0)?.is_empty()
        {
            return Err(TestFailure::SessionDeletionContractChanged);
        }
        let after_delete = drive(
            &application,
            cookie_request(Method::GET, "/api/v2/session", &cookie_pair)?,
        )
        .await?;
        check_error_response(
            &after_delete,
            StatusCode::UNAUTHORIZED,
            "Unauthorized",
            "AUTHENTICATION_FAILED",
        )?;

        let mut observer =
            test_observer(&fixture.path).map_err(|_| TestFailure::DatabaseEvidenceFailed)?;
        let counts_before = session_and_audit_counts(&fixture.database).await?;
        let version_before =
            test_data_version(&mut observer).map_err(|_| TestFailure::DatabaseEvidenceFailed)?;
        let second_delete = drive(
            &application,
            cookie_request(Method::DELETE, "/api/v2/session", &cookie_pair)?,
        )
        .await?;
        if second_delete.status != StatusCode::NO_CONTENT
            || session_and_audit_counts(&fixture.database).await? != counts_before
            || test_data_version(&mut observer).map_err(|_| TestFailure::DatabaseEvidenceFailed)?
                != version_before
        {
            return Err(TestFailure::RepeatedDeleteWroteState);
        }
        let no_cookie_counts = session_and_audit_counts(&fixture.database).await?;
        let no_cookie_version =
            test_data_version(&mut observer).map_err(|_| TestFailure::DatabaseEvidenceFailed)?;
        let no_cookie_delete = drive(
            &application,
            request(Method::DELETE, "/api/v2/session", "")?,
        )
        .await?;
        if no_cookie_delete.status != StatusCode::NO_CONTENT
            || !session_cookie_value(&no_cookie_delete, 0)?.is_empty()
            || session_and_audit_counts(&fixture.database).await? != no_cookie_counts
            || test_data_version(&mut observer).map_err(|_| TestFailure::DatabaseEvidenceFailed)?
                != no_cookie_version
        {
            return Err(TestFailure::MissingCookieDeleteWroteState);
        }
        Ok(())
    }

    #[tokio::test]
    async fn termination_persistence_failure_is_reported_and_session_remains_live()
    -> Result<(), TestFailure> {
        let _verification_guard = PasswordVerificationTestGuard::acquire().await;
        let fixture = TestDatabase::new().await?;
        let operator_id =
            seed_operator(&fixture.database, LOGIN_NAME, OperatorRole::Admin, PASSWORD).await?;
        let application = router(fixture.database.clone(), unused_web_root());
        let login_response = drive(&application, login_request(LOGIN_NAME, PASSWORD)?).await?;
        let cookie_pair =
            validate_login_response(&fixture.database, &login_response, operator_id).await?;

        let database_lock =
            test_lock_database(&fixture.path).map_err(|_| TestFailure::DatabaseEvidenceFailed)?;
        let failed_delete = drive(
            &application,
            cookie_request(Method::DELETE, "/api/v2/session", &cookie_pair)?,
        )
        .await?;
        check_error_response(
            &failed_delete,
            StatusCode::INTERNAL_SERVER_ERROR,
            "Internal Server Error",
            "INTERNAL_ERROR",
        )?;
        if !session_cookie_value(&failed_delete, 0)?.is_empty() {
            return Err(TestFailure::TerminationFailureDidNotClearCookie);
        }
        drop(database_lock);

        let reopened = Database::connect_and_migrate(&DatabaseConfig::new(&fixture.path, false))
            .await
            .map_err(|_| TestFailure::FixtureFailed)?;
        let remaining_sessions = db_operator::tests::test_session_and_audit_counts(&reopened)
            .await
            .map_err(|_| TestFailure::DatabaseEvidenceFailed)?
            .0;
        let retry_application = router(reopened, unused_web_root());
        let authenticated = drive(
            &retry_application,
            cookie_request(Method::GET, "/api/v2/session", &cookie_pair)?,
        )
        .await?;
        if remaining_sessions != 1
            || authenticated.status != StatusCode::OK
            || authenticated.body != login_response.body
        {
            return Err(TestFailure::TerminationFailureRemovedSession);
        }
        Ok(())
    }

    #[tokio::test]
    async fn body_limit_precedes_password_and_database_work() -> Result<(), TestFailure> {
        let _verification_guard = PasswordVerificationTestGuard::acquire().await;
        let fixture = TestDatabase::new().await?;
        let application = router(fixture.database.clone(), unused_web_root());

        let exact_request = Request::builder()
            .method(Method::POST)
            .uri("/api/v2/session")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from("x".repeat(4_096)))
            .map_err(|_| TestFailure::RequestBuildFailed)?;
        let exact_response = drive(&application, exact_request).await?;
        if exact_response.status != StatusCode::BAD_REQUEST {
            return Err(TestFailure::ExactBodyLimitWasRejected);
        }

        let mut observer =
            test_observer(&fixture.path).map_err(|_| TestFailure::DatabaseEvidenceFailed)?;
        let counts_before = session_and_audit_counts_on(&mut observer)?;
        let version_before =
            test_data_version(&mut observer).map_err(|_| TestFailure::DatabaseEvidenceFailed)?;
        let _database_lock =
            test_lock_database(&fixture.path).map_err(|_| TestFailure::DatabaseEvidenceFailed)?;
        let oversized_request = Request::builder()
            .method(Method::POST)
            .uri("/api/v2/session")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from("x".repeat(4_097)))
            .map_err(|_| TestFailure::RequestBuildFailed)?;
        let oversized_response = drive(&application, oversized_request).await?;
        if oversized_response.status != StatusCode::PAYLOAD_TOO_LARGE
            || session_and_audit_counts_on(&mut observer)? != counts_before
            || test_data_version(&mut observer).map_err(|_| TestFailure::DatabaseEvidenceFailed)?
                != version_before
        {
            return Err(TestFailure::BodyLimitPerformedApplicationWork);
        }
        canonical_correlation_id(&oversized_response.headers)?;
        Ok(())
    }

    #[tokio::test]
    async fn login_failures_are_byte_identical_after_correlation_normalization()
    -> Result<(), TestFailure> {
        let _verification_guard = PasswordVerificationTestGuard::acquire().await;
        let fixture = TestDatabase::new().await?;
        seed_operator(&fixture.database, LOGIN_NAME, OperatorRole::Admin, PASSWORD).await?;
        seed_operator(
            &fixture.database,
            "corrupt-http-login",
            OperatorRole::Admin,
            PASSWORD,
        )
        .await?;
        fixture
            .database
            .interact(|connection| {
                diesel::update(
                    operator_accounts::table
                        .filter(operator_accounts::login_name.eq("corrupt-http-login")),
                )
                .set(operator_accounts::password_hash.eq("corrupted-http-phc-canary"))
                .execute(connection)
            })
            .await
            .map_err(|_| TestFailure::DatabaseEvidenceFailed)?
            .map_err(|_| TestFailure::DatabaseEvidenceFailed)?;
        let application = router(fixture.database.clone(), unused_web_root());
        let responses = [
            drive(
                &application,
                login_request("unknown-http-login", "unknown-password-canary")?,
            )
            .await?,
            drive(
                &application,
                login_request(LOGIN_NAME, "wrong-password-canary")?,
            )
            .await?,
            drive(
                &application,
                login_request("corrupt-http-login", "corrupt-password-canary")?,
            )
            .await?,
        ];
        let normalized = normalized_response(&responses[0])?;
        for response in &responses {
            check_error_response(
                response,
                StatusCode::UNAUTHORIZED,
                "Unauthorized",
                "AUTHENTICATION_FAILED",
            )?;
            if normalized_response(response)? != normalized {
                return Err(TestFailure::LoginFailuresWereDistinguishable);
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn real_tls_login_and_session_read_use_the_injected_database() -> Result<(), TestFailure>
    {
        let _verification_guard = PasswordVerificationTestGuard::acquire().await;
        let fixture = TestDatabase::new().await?;
        let operator_id =
            seed_operator(&fixture.database, LOGIN_NAME, OperatorRole::Admin, PASSWORD).await?;
        let identity = TestIdentity::new(LOCALHOST).map_err(|_| TestFailure::TlsFixtureFailed)?;
        let listener = TlsListener::bind(
            SocketAddr::from((LOCALHOST, 0)),
            identity.certificate_path(),
            identity.private_key_path(),
        )
        .await
        .map_err(|_| TestFailure::TlsFixtureFailed)?;
        let address = Listener::local_addr(&listener).map_err(|_| TestFailure::TlsFixtureFailed)?;
        let application = router(fixture.database.clone(), unused_web_root());
        let (shutdown_sender, shutdown_receiver) = oneshot::channel();
        let server = tokio::spawn(async move {
            axum::serve(listener, application)
                .with_graceful_shutdown(async move {
                    let _shutdown_result = shutdown_receiver.await;
                })
                .await
        });

        let body = format!(r#"{{"login_name":"{LOGIN_NAME}","password":"{PASSWORD}"}}"#);
        let post = format!(
            "POST /api/v2/session HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let post_response = tls_request(&identity, address, post.as_bytes()).await?;
        if post_response.status != 200 {
            return Err(TestFailure::TlsSessionRequestFailed);
        }
        let set_cookie = raw_header(&post_response.headers, "set-cookie")?;
        let cookie_pair = set_cookie
            .split(';')
            .next()
            .ok_or(TestFailure::TlsSessionRequestFailed)?;
        let get = format!(
            "GET /api/v2/session HTTP/1.1\r\nHost: {address}\r\nCookie: {cookie_pair}\r\nConnection: close\r\n\r\n"
        );
        let get_response = tls_request(&identity, address, get.as_bytes()).await?;
        let expected_body = format!(r#"{{"operator_id":"{operator_id}","role":"admin"}}"#);
        if get_response.status != 200
            || std::str::from_utf8(&get_response.body)
                .map_err(|_| TestFailure::TlsSessionRequestFailed)?
                != expected_body
        {
            return Err(TestFailure::TlsSessionRequestFailed);
        }
        shutdown_sender
            .send(())
            .map_err(|()| TestFailure::TlsServerFailed)?;
        match timeout(TEST_TIMEOUT, server).await {
            Ok(Ok(Ok(()))) => Ok(()),
            Ok(Ok(Err(_)) | Err(_)) | Err(_) => Err(TestFailure::TlsServerFailed),
        }
    }

    fn session_cookie_value(
        response: &Captured,
        expected_max_age_seconds: i64,
    ) -> Result<String, TestFailure> {
        let value = header_text(&response.headers, &header::SET_COOKIE)?;
        let parsed = Cookie::parse(value).map_err(|_| TestFailure::CookieContractChanged)?;
        if parsed.name() != "__Secure-natsume_session"
            || parsed.path() != Some("/api/v2")
            || parsed.secure() != Some(true)
            || parsed.http_only() != Some(true)
            || parsed.same_site() != Some(cookie::SameSite::Strict)
            || parsed
                .max_age()
                .map(cookie::time::SignedDuration::whole_seconds)
                != Some(expected_max_age_seconds)
            || parsed.expires().is_some()
        {
            return Err(TestFailure::CookieContractChanged);
        }
        Ok(parsed.value().to_owned())
    }

    async fn validate_login_response(
        database: &Database,
        response: &Captured,
        operator_id: Uuid,
    ) -> Result<String, TestFailure> {
        let response_correlation = canonical_correlation_id(&response.headers)?;
        let credential = session_cookie_value(response, 28_800)?;
        if credential.len() != 64
            || !credential
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(TestFailure::CookieContractChanged);
        }
        let cookie_pair = format!("__Secure-natsume_session={credential}");
        let login_json: Value =
            serde_json::from_slice(&response.body).map_err(|_| TestFailure::ResponseJsonInvalid)?;
        let login_object = login_json
            .as_object()
            .ok_or(TestFailure::ResponseJsonInvalid)?;
        let expected_operator_id = operator_id.to_string();
        if login_object.len() != 2
            || login_object.get("operator_id").and_then(Value::as_str)
                != Some(expected_operator_id.as_str())
            || login_object.get("role").and_then(Value::as_str) != Some("admin")
            || response_body_text(response)?.contains(&credential)
            || response_body_text(response)?.contains(PASSWORD)
            || response_body_text(response)?.contains("password")
        {
            return Err(TestFailure::SessionResponseContractChanged);
        }
        let audit_correlation = database
            .interact(|connection| {
                audit_events::table
                    .filter(audit_events::action_kind.eq("establish_session"))
                    .select(audit_events::correlation_id)
                    .first::<String>(connection)
            })
            .await
            .map_err(|_| TestFailure::DatabaseEvidenceFailed)?
            .map_err(|_| TestFailure::DatabaseEvidenceFailed)?;
        if audit_correlation != response_correlation {
            return Err(TestFailure::AuditCorrelationChanged);
        }
        Ok(cookie_pair)
    }

    fn duplicated_live_cookie_headers(cookie_pair: &str) -> Result<[String; 2], TestFailure> {
        let (name, live_credential) = cookie_pair
            .split_once('=')
            .ok_or(TestFailure::CookieContractChanged)?;
        let replacement = if live_credential.starts_with('a') {
            'b'
        } else {
            'a'
        };
        let decoy = format!("{replacement}{}", &live_credential[1..]);
        Ok([
            format!("{cookie_pair}; {name}={decoy}"),
            format!("{name}={decoy}; {cookie_pair}"),
        ])
    }

    fn normalized_response(response: &Captured) -> Result<NormalizedResponse, TestFailure> {
        canonical_correlation_id(&response.headers)?;
        let mut headers = response
            .headers
            .iter()
            .map(|(name, value)| {
                let value = if name == CORRELATION_ID_HEADER {
                    "NORMALIZED".to_owned()
                } else {
                    value
                        .to_str()
                        .map(str::to_owned)
                        .map_err(|_| TestFailure::ResponseHeaderInvalid)?
                };
                Ok((name.as_str().to_owned(), value))
            })
            .collect::<Result<Vec<_>, TestFailure>>()?;
        headers.sort();
        Ok(NormalizedResponse {
            status: response.status,
            headers,
            body: normalized_error_response_body(response)?,
        })
    }

    async fn session_expiry(database: &Database) -> Result<String, TestFailure> {
        database
            .interact(|connection| {
                operator_sessions::table
                    .select(operator_sessions::expires_at)
                    .first(connection)
            })
            .await
            .map_err(|_| TestFailure::DatabaseEvidenceFailed)?
            .map_err(|_| TestFailure::DatabaseEvidenceFailed)
    }

    async fn session_and_audit_counts(database: &Database) -> Result<(i64, i64), TestFailure> {
        db_operator::tests::test_session_and_audit_counts(database)
            .await
            .map_err(|_| TestFailure::DatabaseEvidenceFailed)
    }

    fn session_and_audit_counts_on(
        connection: &mut SqliteConnection,
    ) -> Result<(i64, i64), TestFailure> {
        diesel::sql_query(
            "SELECT (SELECT COUNT(*) FROM operator_sessions) AS sessions, \
             (SELECT COUNT(*) FROM audit_events) AS audits",
        )
        .get_result::<PersistenceCountsRow>(connection)
        .map(|row| (row.sessions, row.audits))
        .map_err(|_| TestFailure::DatabaseEvidenceFailed)
    }

    #[derive(QueryableByName)]
    struct PersistenceCountsRow {
        #[diesel(sql_type = BigInt)]
        sessions: i64,
        #[diesel(sql_type = BigInt)]
        audits: i64,
    }

    async fn tls_request(
        identity: &TestIdentity,
        address: SocketAddr,
        request: &[u8],
    ) -> Result<RawResponse, TestFailure> {
        let mut stream =
            connect_test_client(address, identity.ca_certificate(), IpAddr::V4(LOCALHOST))
                .await
                .map_err(|_| TestFailure::TlsSessionRequestFailed)?;
        stream
            .write_all(request)
            .await
            .map_err(|_| TestFailure::TlsSessionRequestFailed)?;
        let mut response = Vec::new();
        timeout(TEST_TIMEOUT, stream.read_to_end(&mut response))
            .await
            .map_err(|_| TestFailure::TlsSessionRequestFailed)?
            .map_err(|_| TestFailure::TlsSessionRequestFailed)?;
        parse_raw_response(&response)
    }

    fn parse_raw_response(response: &[u8]) -> Result<RawResponse, TestFailure> {
        let split = response
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .ok_or(TestFailure::TlsSessionRequestFailed)?;
        let headers = std::str::from_utf8(&response[..split])
            .map_err(|_| TestFailure::TlsSessionRequestFailed)?;
        let mut lines = headers.lines();
        let status = lines
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|value| value.parse::<u16>().ok())
            .ok_or(TestFailure::TlsSessionRequestFailed)?;
        Ok(RawResponse {
            status,
            headers: lines.map(str::to_owned).collect(),
            body: response[split + 4..].to_vec(),
        })
    }

    fn raw_header<'a>(headers: &'a [String], name: &str) -> Result<&'a str, TestFailure> {
        headers
            .iter()
            .find_map(|line| {
                line.split_once(':').and_then(|(candidate, value)| {
                    candidate.eq_ignore_ascii_case(name).then_some(value.trim())
                })
            })
            .ok_or(TestFailure::TlsSessionRequestFailed)
    }

    #[derive(PartialEq, Eq)]
    struct NormalizedResponse {
        status: StatusCode,
        headers: Vec<(String, String)>,
        body: String,
    }

    struct RawResponse {
        status: u16,
        headers: Vec<String>,
        body: Vec<u8>,
    }

    #[derive(Debug, Snafu)]
    enum TestFailure {
        #[snafu(display("an HTTP test helper failed"))]
        #[snafu(context(false))]
        Support { source: SupportFailure },
        #[snafu(display("the HTTP test fixture failed"))]
        FixtureFailed,
        #[snafu(display("the HTTP request could not be built"))]
        RequestBuildFailed,
        #[snafu(display("an HTTP response header was invalid"))]
        ResponseHeaderInvalid,
        #[snafu(display("an HTTP response was not valid JSON"))]
        ResponseJsonInvalid,
        #[snafu(display("request parser detail escaped the HTTP boundary"))]
        ParserDetailEscaped,
        #[snafu(display("session credential failures were distinguishable"))]
        CredentialFailuresWereDistinguishable,
        #[snafu(display("a duplicated live session cookie was accepted"))]
        DuplicateCookieWasAccepted,
        #[snafu(display("a valid operator login failed"))]
        ValidLoginFailed,
        #[snafu(display("the session cookie contract changed"))]
        CookieContractChanged,
        #[snafu(display("the session response contract changed"))]
        SessionResponseContractChanged,
        #[snafu(display("the audit correlation changed"))]
        AuditCorrelationChanged,
        #[snafu(display("session authentication changed persisted state"))]
        SessionReadChangedState,
        #[snafu(display("the session deletion contract changed"))]
        SessionDeletionContractChanged,
        #[snafu(display("a repeated session deletion wrote state"))]
        RepeatedDeleteWroteState,
        #[snafu(display("a missing-cookie deletion wrote state"))]
        MissingCookieDeleteWroteState,
        #[snafu(display("a termination failure did not send the clearing cookie"))]
        TerminationFailureDidNotClearCookie,
        #[snafu(display("a termination failure removed the live session"))]
        TerminationFailureRemovedSession,
        #[snafu(display("the exact body limit was rejected"))]
        ExactBodyLimitWasRejected,
        #[snafu(display("an oversized body performed application work"))]
        BodyLimitPerformedApplicationWork,
        #[snafu(display("login failures were distinguishable"))]
        LoginFailuresWereDistinguishable,
        #[snafu(display("database evidence could not be read"))]
        DatabaseEvidenceFailed,
        #[snafu(display("the TLS test fixture failed"))]
        TlsFixtureFailed,
        #[snafu(display("the TLS session request failed"))]
        TlsSessionRequestFailed,
        #[snafu(display("the TLS server failed"))]
        TlsServerFailed,
    }
}
