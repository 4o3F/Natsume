use super::dto::*;
use crate::{
    component::operator::tests::{PasswordVerificationTestGuard, occupy_sign_in_capacity},
    http::{
        self, AppState,
        tests::{self as support, Captured, TestDatabase, drive, seed_operator},
    },
};
use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use serde_json::{Value, json};
use uuid::Uuid;

const PASSWORD: &str = "http-operator-password1!";
const NEW_PASSWORD: &str = "http-replacement-password2@";
type TestResult = Result<(), Box<dyn std::error::Error>>;

struct Fixture {
    database: TestDatabase,
    state: AppState,
    app: Router,
    admin_id: Uuid,
    admin_cookie: String,
}
impl Fixture {
    async fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let database = TestDatabase::new().await?;
        let admin_id = seed_operator(&database.database, "admin", PASSWORD).await?;
        let state = support::server_state(&database)?;
        let app = http::router(state.clone(), support::unused_web_root());
        let admin_cookie = login(&app, "admin", PASSWORD).await?;
        Ok(Self {
            database,
            state,
            app,
            admin_id,
            admin_cookie,
        })
    }
    async fn request(
        &self,
        method: Method,
        path: &str,
        cookie: Option<&str>,
        body: Value,
    ) -> Result<Captured, Box<dyn std::error::Error>> {
        send(&self.app, method, path, cookie, body.to_string()).await
    }
    async fn invite(&self, role: &str) -> Result<Value, Box<dyn std::error::Error>> {
        let response = self
            .request(
                Method::POST,
                "/api/v2/operator/invitations",
                Some(&self.admin_cookie),
                json!({"role": role}),
            )
            .await?;
        assert_eq!(response.status, StatusCode::CREATED);
        assert_no_store(&response);
        Ok(serde_json::from_slice(&response.body)?)
    }
    async fn signup(&self, name: &str, role: &str) -> Result<Value, Box<dyn std::error::Error>> {
        let invitation = self.invite(role).await?;
        let response = self
            .request(
                Method::POST,
                "/api/v2/operator/register",
                None,
                registration(&invitation, name),
            )
            .await?;
        assert_eq!(response.status, StatusCode::CREATED);
        assert!(!response.headers.contains_key(header::SET_COOKIE));
        Ok(serde_json::from_slice(&response.body)?)
    }
    async fn recovery(&self, id: &str) -> Result<Value, Box<dyn std::error::Error>> {
        let response = self
            .request(
                Method::POST,
                &format!("/api/v2/operator/accounts/{id}/password-resets"),
                Some(&self.admin_cookie),
                json!({}),
            )
            .await?;
        assert_eq!(response.status, StatusCode::CREATED);
        assert_no_store(&response);
        Ok(serde_json::from_slice(&response.body)?)
    }
}

async fn send(
    app: &Router,
    method: Method,
    path: &str,
    cookie: Option<&str>,
    body: String,
) -> Result<Captured, Box<dyn std::error::Error>> {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    Ok(drive(app, builder.body(Body::from(body))?).await?)
}
async fn login(
    app: &Router,
    username: &str,
    password: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let response = drive(app, support::login_request(username, password)?).await?;
    assert_eq!(response.status, StatusCode::OK);
    Ok(response.headers[header::SET_COOKIE]
        .to_str()?
        .split(';')
        .next()
        .ok_or("missing cookie")?
        .to_owned())
}
fn registration(invite: &Value, name: &str) -> Value {
    json!({"token": invite["token"], "username": name, "password": PASSWORD, "password_confirmation": PASSWORD})
}
fn reset_form(recovery: &Value) -> Value {
    json!({"token": recovery["token"], "password": NEW_PASSWORD, "password_confirmation": NEW_PASSWORD})
}
fn change_form() -> Value {
    json!({"current_password": PASSWORD, "password": NEW_PASSWORD, "password_confirmation": NEW_PASSWORD})
}
fn assert_no_store(response: &Captured) {
    assert_eq!(response.headers[header::CACHE_CONTROL], "no-store");
}
fn assert_error(response: &Captured, status: StatusCode, code: &str) -> TestResult {
    assert_eq!(response.status, status);
    assert_no_store(response);
    let body: Value = serde_json::from_slice(&response.body)?;
    assert_eq!(body["status"], status.as_u16());
    assert_eq!(body["code"], code);
    assert_eq!(body.as_object().ok_or("not an object")?.len(), 3);
    Ok(())
}

#[tokio::test]
async fn namespace_cache_policy_covers_outer_rejections_and_unmounted_paths() -> TestResult {
    let database = TestDatabase::new().await?;
    let app = http::router(
        support::server_state(&database)?,
        support::unused_web_root(),
    );
    for (method, path, status) in [
        (
            Method::POST,
            "/api/v2/operator/register/inspect",
            StatusCode::BAD_REQUEST,
        ),
        (
            Method::HEAD,
            "/api/v2/operator/accounts",
            StatusCode::NOT_FOUND,
        ),
        (
            Method::GET,
            "/api/v2/operator/register",
            StatusCode::METHOD_NOT_ALLOWED,
        ),
        (
            Method::GET,
            "/api/v2/operator/unmounted",
            StatusCode::NOT_FOUND,
        ),
    ] {
        let request = Request::builder()
            .method(method)
            .uri(path)
            .body(Body::empty())?;
        let response = drive(&app, request).await?;
        assert_eq!(response.status, status);
        assert_no_store(&response);
    }
    let health = drive(
        &app,
        Request::builder()
            .uri("/api/v2/health")
            .body(Body::empty())?,
    )
    .await?;
    assert_eq!(health.status, StatusCode::OK);
    assert!(!health.headers.contains_key(header::CACHE_CONTROL));
    Ok(())
}

#[tokio::test]
async fn session_responses_include_account_username_for_admin_and_viewer() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = Fixture::new().await?;
    let viewer_name = "查看者-viewer";
    let viewer = f.signup(viewer_name, "viewer").await?;
    for (username, role, operator_id) in [
        ("admin", "admin", json!(f.admin_id)),
        (viewer_name, "viewer", viewer["operator_id"].clone()),
    ] {
        let expected = json!({"operator_id": operator_id, "username": username, "role": role});
        let login = f
            .request(
                Method::POST,
                "/api/v2/session",
                None,
                json!({"login_name": username, "password": PASSWORD}),
            )
            .await?;
        assert_eq!(login.status, StatusCode::OK);
        assert_eq!(serde_json::from_slice::<Value>(&login.body)?, expected);
        let cookie = support::header_text(&login.headers, &header::SET_COOKIE)?
            .split(';')
            .next()
            .ok_or("missing cookie")?;
        let session = drive(
            &f.app,
            support::cookie_request(Method::GET, "/api/v2/session", cookie)?,
        )
        .await?;
        assert_eq!(session.status, StatusCode::OK);
        assert_eq!(serde_json::from_slice::<Value>(&session.body)?, expected);
        assert!(!session.headers.contains_key(header::SET_COOKIE));
    }
    Ok(())
}

#[tokio::test]
async fn session_read_rejects_account_deleted_after_authentication() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = Fixture::new().await?;
    f.signup("viewer", "viewer").await?;
    let signed_in = f
        .state
        .operator()
        .sign_in("viewer", PASSWORD.into())
        .await?;
    let identity = signed_in.identity();
    let deletion = f
        .request(
            Method::DELETE,
            &format!("/api/v2/operator/accounts/{}", identity.operator_id()),
            Some(&f.admin_cookie),
            json!({}),
        )
        .await?;
    assert_eq!(deletion.status, StatusCode::NO_CONTENT);
    let response = crate::http::handler::session::read_session(
        axum::extract::State(f.state),
        axum::Extension(identity),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(!response.headers().contains_key(header::SET_COOKIE));
    Ok(())
}

fn unread_management_body() -> Body {
    Body::from_stream(futures_util::stream::poll_fn(
        |_| -> std::task::Poll<Option<Result<axum::body::Bytes, std::io::Error>>> {
            panic!("rejected administrator request body was polled")
        },
    ))
}

#[tokio::test]
async fn every_admin_route_rejects_before_body_reads_and_observes_demotion() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = Fixture::new().await?;
    let second = f.signup("second", "admin").await?;
    let cookie = login(&f.app, "second", PASSWORD).await?;
    let second_id = second["operator_id"].as_str().ok_or("missing id")?;
    let demotion = f
        .request(
            Method::PATCH,
            &format!("/api/v2/operator/accounts/{second_id}"),
            Some(&f.admin_cookie),
            json!({"role":"viewer"}),
        )
        .await?;
    assert_eq!(demotion.status, StatusCode::NO_CONTENT);
    let id = f.admin_id;
    for (method, path) in [
        (Method::GET, "/api/v2/operator/accounts".to_owned()),
        (Method::PATCH, format!("/api/v2/operator/accounts/{id}")),
        (Method::DELETE, format!("/api/v2/operator/accounts/{id}")),
        (Method::GET, "/api/v2/operator/invitations".to_owned()),
        (Method::POST, "/api/v2/operator/invitations".to_owned()),
        (Method::DELETE, format!("/api/v2/operator/invitations/{id}")),
        (
            Method::POST,
            format!("/api/v2/operator/invitations/{id}/actions/regenerate"),
        ),
        (
            Method::POST,
            format!("/api/v2/operator/accounts/{id}/password-resets"),
        ),
        (Method::GET, "/api/v2/print/sources".to_owned()),
        (Method::POST, "/api/v2/print/sources".to_owned()),
        (Method::DELETE, format!("/api/v2/print/sources/{id}")),
        (Method::GET, "/api/v2/print/printers".to_owned()),
        (Method::POST, "/api/v2/print/printers".to_owned()),
        (
            Method::POST,
            format!("/api/v2/print/printers/{id}/pairing-code"),
        ),
        (
            Method::DELETE,
            format!("/api/v2/print/printers/{id}/binding"),
        ),
        (Method::PATCH, format!("/api/v2/devices/{id}")),
        (Method::DELETE, format!("/api/v2/devices/{id}/binding")),
        (Method::PUT, "/api/v2/provisioning-window".to_owned()),
        (Method::POST, "/api/v2/target-submissions".to_owned()),
        (
            Method::POST,
            format!("/api/v2/enrollment-reviews/{id}/actions/approve"),
        ),
        (
            Method::POST,
            format!("/api/v2/enrollment-reviews/{id}/actions/deny"),
        ),
        (Method::POST, "/api/v2/imports".to_owned()),
        (Method::GET, "/api/v2/imports".to_owned()),
        (Method::GET, "/api/v2/imports/template".to_owned()),
        (Method::DELETE, format!("/api/v2/imports/{id}")),
        (Method::POST, format!("/api/v2/imports/{id}/actions/commit")),
        (Method::GET, "/api/v2/organizations".to_owned()),
        (Method::GET, format!("/api/v2/imports/{id}/organizations")),
        (
            Method::GET,
            format!("/api/v2/imports/{id}/organizations/1/logo"),
        ),
        (Method::GET, "/api/v2/exports/domjudge".to_owned()),
    ] {
        for credential in [None, Some(cookie.as_str())] {
            let mut request = Request::builder()
                .method(method.clone())
                .uri(&path)
                .header(header::CONTENT_TYPE, "application/json");
            if let Some(cookie) = credential {
                request = request.header(header::COOKIE, cookie);
            }
            let response = drive(&f.app, request.body(unread_management_body())?).await?;
            let status = if credential.is_none() {
                StatusCode::UNAUTHORIZED
            } else {
                StatusCode::FORBIDDEN
            };
            assert_eq!(response.status, status, "{method} {path}");
            let body: Value = serde_json::from_slice(&response.body)?;
            assert_eq!(body.as_object().ok_or("invalid error")?.len(), 3);
            assert_eq!(body["status"], status.as_u16());
            assert_eq!(
                body["code"],
                if credential.is_none() {
                    "AUTHENTICATION_FAILED"
                } else {
                    "AUTHORIZATION_DENIED"
                }
            );
            if path.starts_with("/api/v2/operator/") || path.starts_with("/api/v2/print/") {
                assert_no_store(&response);
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn registration_is_closed_consumes_once_and_keeps_conflicting_invites_usable() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = Fixture::new().await?;
    let invitation = f.invite("viewer").await?;
    let inspect = f
        .request(
            Method::POST,
            "/api/v2/operator/register/inspect",
            None,
            json!({"token":invitation["token"]}),
        )
        .await?;
    assert_eq!(inspect.status, StatusCode::OK);
    assert_eq!(
        serde_json::from_slice::<Value>(&inspect.body)?,
        json!({"role":"viewer"})
    );
    assert_no_store(&inspect);
    let mut form = registration(&invitation, "admin");
    let conflict = f
        .request(
            Method::POST,
            "/api/v2/operator/register",
            None,
            form.clone(),
        )
        .await?;
    assert_error(
        &conflict,
        StatusCode::CONFLICT,
        "OPERATOR_LOGIN_NAME_CONFLICT",
    )?;
    for (field, value) in [
        ("role", json!("admin")),
        ("password", json!("no-number-or-symbol")),
        ("username", json!(" trimmed ")),
        ("password_confirmation", json!(NEW_PASSWORD)),
    ] {
        let mut invalid = form.clone();
        invalid[field] = value;
        let response = f
            .request(Method::POST, "/api/v2/operator/register", None, invalid)
            .await?;
        assert_error(&response, StatusCode::BAD_REQUEST, "INVALID_REQUEST")?;
    }
    form["username"] = json!("Admin");
    let accepted = f
        .request(
            Method::POST,
            "/api/v2/operator/register",
            None,
            form.clone(),
        )
        .await?;
    assert_eq!(accepted.status, StatusCode::CREATED);
    assert!(!accepted.headers.contains_key(header::SET_COOKIE));
    let account: Value = serde_json::from_slice(&accepted.body)?;
    assert_eq!(account["username"], "Admin");
    assert_eq!(account["role"], "viewer");
    assert_eq!(account.as_object().ok_or("invalid account")?.len(), 3);
    assert_error(
        &f.request(Method::POST, "/api/v2/operator/register", None, form)
            .await?,
        StatusCode::GONE,
        "OPERATOR_LINK_UNAVAILABLE",
    )?;
    login(&f.app, "Admin", PASSWORD).await?;
    let list = f
        .request(
            Method::GET,
            "/api/v2/operator/accounts",
            Some(&f.admin_cookie),
            json!({}),
        )
        .await?;
    let accounts: Value = serde_json::from_slice(&list.body)?;
    assert_eq!(accounts.as_array().ok_or("invalid list")?.len(), 2);
    assert!(!String::from_utf8(list.body)?.contains("password"));
    Ok(())
}

#[tokio::test]
async fn list_expiry_regeneration_and_revocation_never_return_old_plaintext() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = Fixture::new().await?;
    let one = f.invite("admin").await?;
    f.database
        .database
        .write(|tx| {
            use diesel::{RunQueryDsl, sql_query};
            sql_query("UPDATE operator_invitations SET expires_at_unix_ms = 0")
                .execute(tx.connection())
                .map(|_| ())
                .map_err(|_| crate::db::PersistenceError::OperationFailed)
        })
        .await
        .map_err(crate::db::TransactionError::into_error)?;
    let list = f
        .request(
            Method::GET,
            "/api/v2/operator/invitations",
            Some(&f.admin_cookie),
            json!({}),
        )
        .await?;
    assert_no_store(&list);
    let body: Value = serde_json::from_slice(&list.body)?;
    assert_eq!(body[0]["expired"], true);
    assert_eq!(body[0].as_object().ok_or("invalid invite")?.len(), 6);
    assert!(!String::from_utf8(list.body)?.contains("token"));
    let id = one["invitation"]["invitation_id"]
        .as_str()
        .ok_or("missing id")?;
    let refreshed = f
        .request(
            Method::POST,
            &format!("/api/v2/operator/invitations/{id}/actions/regenerate"),
            Some(&f.admin_cookie),
            json!({}),
        )
        .await?;
    assert_eq!(refreshed.status, StatusCode::CREATED);
    let two: Value = serde_json::from_slice(&refreshed.body)?;
    assert_eq!(two["invitation"]["role"], "admin");
    assert_eq!(two["invitation"]["expired"], false);
    assert_ne!(two["token"], one["token"]);
    assert_error(
        &f.request(
            Method::POST,
            "/api/v2/operator/register/inspect",
            None,
            json!({"token":one["token"]}),
        )
        .await?,
        StatusCode::GONE,
        "OPERATOR_LINK_UNAVAILABLE",
    )?;
    let id = two["invitation"]["invitation_id"]
        .as_str()
        .ok_or("missing id")?;
    for _ in 0..2 {
        assert_eq!(
            f.request(
                Method::DELETE,
                &format!("/api/v2/operator/invitations/{id}"),
                Some(&f.admin_cookie),
                json!({})
            )
            .await?
            .status,
            StatusCode::NO_CONTENT
        );
    }
    assert_error(
        &f.request(
            Method::POST,
            "/api/v2/operator/register/inspect",
            None,
            json!({"token":two["token"]}),
        )
        .await?,
        StatusCode::GONE,
        "OPERATOR_LINK_UNAVAILABLE",
    )?;
    Ok(())
}

#[tokio::test]
async fn reset_replaces_target_grants_preserves_sessions_until_success_and_never_logs_in()
-> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = Fixture::new().await?;
    let viewer = f.signup("viewer", "viewer").await?;
    let viewer_cookie = login(&f.app, "viewer", PASSWORD).await?;
    let id = viewer["operator_id"].as_str().ok_or("missing id")?;
    let old = f.recovery(id).await?;
    assert_eq!(old["username"], "viewer");
    assert_eq!(
        f.request(
            Method::GET,
            "/api/v2/session",
            Some(&viewer_cookie),
            json!({})
        )
        .await?
        .status,
        StatusCode::OK
    );
    let new = f.recovery(id).await?;
    assert_error(
        &f.request(
            Method::POST,
            "/api/v2/operator/password/reset",
            None,
            reset_form(&old),
        )
        .await?,
        StatusCode::GONE,
        "OPERATOR_LINK_UNAVAILABLE",
    )?;
    let inspect = f
        .request(
            Method::POST,
            "/api/v2/operator/password/reset/inspect",
            None,
            json!({"token":new["token"]}),
        )
        .await?;
    assert_eq!(
        serde_json::from_slice::<Value>(&inspect.body)?,
        json!({"username":"viewer"})
    );
    let mut invalid = reset_form(&new);
    invalid["operator_id"] = json!(f.admin_id);
    assert_error(
        &f.request(
            Method::POST,
            "/api/v2/operator/password/reset",
            None,
            invalid,
        )
        .await?,
        StatusCode::BAD_REQUEST,
        "INVALID_REQUEST",
    )?;
    let response = f
        .request(
            Method::POST,
            "/api/v2/operator/password/reset",
            None,
            reset_form(&new),
        )
        .await?;
    assert_eq!(response.status, StatusCode::NO_CONTENT);
    assert!(!response.headers.contains_key(header::SET_COOKIE));
    assert_no_store(&response);
    assert_eq!(
        f.request(
            Method::GET,
            "/api/v2/session",
            Some(&viewer_cookie),
            json!({})
        )
        .await?
        .status,
        StatusCode::UNAUTHORIZED
    );
    assert_error(
        &f.request(
            Method::POST,
            "/api/v2/operator/password/reset",
            None,
            reset_form(&new),
        )
        .await?,
        StatusCode::GONE,
        "OPERATOR_LINK_UNAVAILABLE",
    )?;
    login(&f.app, "viewer", NEW_PASSWORD).await?;
    Ok(())
}

#[tokio::test]
async fn own_password_error_keeps_the_session_and_success_clears_every_session() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    for role in ["admin", "viewer"] {
        let f = Fixture::new().await?;
        let anonymous_change = f
            .request(
                Method::POST,
                "/api/v2/operator/password/change",
                None,
                change_form(),
            )
            .await?;
        assert_error(
            &anonymous_change,
            StatusCode::UNAUTHORIZED,
            "AUTHENTICATION_FAILED",
        )?;
        let account = f.signup("target", role).await?;
        let one = login(&f.app, "target", PASSWORD).await?;
        let two = login(&f.app, "target", PASSWORD).await?;
        let mut invalid = change_form();
        invalid["current_password"] = json!("wrong");
        assert_error(
            &f.request(
                Method::POST,
                "/api/v2/operator/password/change",
                Some(&one),
                invalid,
            )
            .await?,
            StatusCode::BAD_REQUEST,
            "OPERATOR_CURRENT_PASSWORD_INVALID",
        )?;
        assert_eq!(
            f.request(Method::GET, "/api/v2/session", Some(&one), json!({}))
                .await?
                .status,
            StatusCode::OK
        );
        let recovery = f
            .recovery(account["operator_id"].as_str().ok_or("missing id")?)
            .await?;
        let response = f
            .request(
                Method::POST,
                "/api/v2/operator/password/change",
                Some(&one),
                change_form(),
            )
            .await?;
        assert_eq!(response.status, StatusCode::NO_CONTENT);
        assert!(
            response.headers[header::SET_COOKIE]
                .to_str()?
                .contains("Max-Age=0")
        );
        for cookie in [&one, &two] {
            assert_eq!(
                f.request(Method::GET, "/api/v2/session", Some(cookie), json!({}))
                    .await?
                    .status,
                StatusCode::UNAUTHORIZED
            );
        }
        assert_error(
            &f.request(
                Method::POST,
                "/api/v2/operator/password/reset/inspect",
                None,
                json!({"token":recovery["token"]}),
            )
            .await?,
            StatusCode::GONE,
            "OPERATOR_LINK_UNAVAILABLE",
        )?;
        login(&f.app, "target", NEW_PASSWORD).await?;
    }
    Ok(())
}

#[tokio::test]
async fn link_forms_require_logout_and_link_failures_do_not_end_a_different_login() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = Fixture::new().await?;
    let invitation = f.invite("viewer").await?;
    let recovery = f.recovery(&f.admin_id.to_string()).await?;
    for (path, body) in [
        (
            "/api/v2/operator/register/inspect",
            json!({"token":invitation["token"]}),
        ),
        (
            "/api/v2/operator/register",
            registration(&invitation, "new"),
        ),
        (
            "/api/v2/operator/password/reset/inspect",
            json!({"token":recovery["token"]}),
        ),
        ("/api/v2/operator/password/reset", reset_form(&recovery)),
    ] {
        assert_error(
            &f.request(Method::POST, path, Some(&f.admin_cookie), body)
                .await?,
            StatusCode::CONFLICT,
            "OPERATOR_LOGOUT_REQUIRED",
        )?;
    }
    for path in [
        "/api/v2/operator/register/inspect",
        "/api/v2/operator/password/reset/inspect",
    ] {
        let response = f
            .request(
                Method::POST,
                path,
                None,
                json!({"token":"bad-token-canary"}),
            )
            .await?;
        assert_error(&response, StatusCode::GONE, "OPERATOR_LINK_UNAVAILABLE")?;
        assert!(!response.headers.contains_key(header::SET_COOKIE));
    }
    assert_eq!(
        f.request(
            Method::GET,
            "/api/v2/session",
            Some(&f.admin_cookie),
            json!({})
        )
        .await?
        .status,
        StatusCode::OK
    );
    assert_eq!(
        f.request(
            Method::DELETE,
            "/api/v2/session",
            Some(&f.admin_cookie),
            json!({})
        )
        .await?
        .status,
        StatusCode::NO_CONTENT
    );
    // The browser can carry the now-stale cookie through the anonymous flow.
    assert_eq!(
        f.request(
            Method::POST,
            "/api/v2/operator/register",
            Some(&f.admin_cookie),
            registration(&invitation, "new")
        )
        .await?
        .status,
        StatusCode::CREATED
    );
    Ok(())
}

#[tokio::test]
async fn last_admin_self_demotion_and_self_deletion_are_visible_over_http() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = Fixture::new().await?;
    f.invite("admin").await?;
    let path = format!("/api/v2/operator/accounts/{}", f.admin_id);
    for method in [Method::PATCH, Method::DELETE] {
        assert_error(
            &f.request(
                method,
                &path,
                Some(&f.admin_cookie),
                json!({"role":"viewer"}),
            )
            .await?,
            StatusCode::CONFLICT,
            "OPERATOR_LAST_ADMIN",
        )?;
    }
    let second = f.signup("second", "admin").await?;
    let cookie = login(&f.app, "second", PASSWORD).await?;
    let second_path = format!(
        "/api/v2/operator/accounts/{}",
        second["operator_id"].as_str().ok_or("missing id")?
    );
    assert_eq!(
        f.request(
            Method::PATCH,
            &second_path,
            Some(&cookie),
            json!({"role":"viewer"})
        )
        .await?
        .status,
        StatusCode::NO_CONTENT
    );
    let session = f
        .request(Method::GET, "/api/v2/session", Some(&cookie), json!({}))
        .await?;
    assert_eq!(
        serde_json::from_slice::<Value>(&session.body)?["role"],
        "viewer"
    );
    assert_error(
        &f.request(
            Method::GET,
            "/api/v2/operator/accounts",
            Some(&cookie),
            json!({}),
        )
        .await?,
        StatusCode::FORBIDDEN,
        "AUTHORIZATION_DENIED",
    )?;
    assert_eq!(
        f.request(
            Method::PATCH,
            &second_path,
            Some(&f.admin_cookie),
            json!({"role":"admin"})
        )
        .await?
        .status,
        StatusCode::NO_CONTENT
    );
    let deleted = f
        .request(Method::DELETE, &second_path, Some(&cookie), json!({}))
        .await?;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT);
    assert!(
        deleted.headers[header::SET_COOKIE]
            .to_str()?
            .contains("Max-Age=0")
    );
    assert_eq!(
        f.request(Method::GET, "/api/v2/session", Some(&cookie), json!({}))
            .await?
            .status,
        StatusCode::UNAUTHORIZED
    );
    assert_error(
        &f.request(
            Method::DELETE,
            &second_path,
            Some(&f.admin_cookie),
            json!({}),
        )
        .await?,
        StatusCode::NOT_FOUND,
        "RESOURCE_NOT_FOUND",
    )?;
    Ok(())
}

#[tokio::test]
async fn ids_roles_and_secret_json_fields_use_closed_contracts() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = Fixture::new().await?;
    for id in [
        "not-a-uuid".to_owned(),
        Uuid::nil().to_string(),
        f.admin_id.to_string().replace('-', ""),
        f.admin_id.to_string().to_uppercase(),
    ] {
        assert_error(
            &f.request(
                Method::PATCH,
                &format!("/api/v2/operator/accounts/{id}"),
                Some(&f.admin_cookie),
                json!({"role":"admin"}),
            )
            .await?,
            StatusCode::BAD_REQUEST,
            "INVALID_REQUEST",
        )?;
    }
    for body in [
        json!({"role":"owner"}),
        json!({"role":"admin","username":"renamed"}),
        json!({}),
        json!({"role":null}),
    ] {
        assert_error(
            &f.request(
                Method::POST,
                "/api/v2/operator/invitations",
                Some(&f.admin_cookie),
                body,
            )
            .await?,
            StatusCode::BAD_REQUEST,
            "INVALID_REQUEST",
        )?;
    }
    for raw in [
        "{",
        r#"{"token":false}"#,
        r#"{"token":"secret","token":"duplicate"}"#,
        r#"{"token":"secret","extra":1}"#,
    ] {
        assert_error(
            &send(
                &f.app,
                Method::POST,
                "/api/v2/operator/register/inspect",
                None,
                raw.to_owned(),
            )
            .await?,
            StatusCode::BAD_REQUEST,
            "INVALID_REQUEST",
        )?;
    }
    let request: OperatorRegistrationRequest = serde_json::from_value(
        json!({"token":"token-canary", "username":"public", "password":"password-canary", "password_confirmation":"confirmation-canary"}),
    )?;
    let debug = format!("{request:?}");
    for secret in ["token-canary", "password-canary", "confirmation-canary"] {
        assert!(!debug.contains(secret));
    }
    Ok(())
}

#[tokio::test]
async fn maximum_escaped_password_forms_fit() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = Fixture::new().await?;
    let maximum = "a".repeat(1022) + "1!";
    seed_operator(&f.database.database, "maximum", &maximum).await?;
    let cookie = login(&f.app, "maximum", &maximum).await?;
    let mut escaped = String::new();
    for byte in maximum.bytes() {
        use std::fmt::Write as _;
        write!(escaped, "\\u{byte:04x}")?;
    }
    let body = format!(
        r#"{{"current_password":"{escaped}","password":"{escaped}","password_confirmation":"{escaped}"}}"#
    );
    assert!(body.len() > 16 * 1024);
    let response = send(
        &f.app,
        Method::POST,
        "/api/v2/operator/password/change",
        Some(&cookie),
        body,
    )
    .await?;
    assert_eq!(response.status, StatusCode::NO_CONTENT);
    let invite = f.invite("viewer").await?;
    let username = "u".repeat(128);
    let body = format!(
        r#"{{"token":{},"username":"{username}","password":"{escaped}","password_confirmation":"{escaped}"}}"#,
        invite["token"]
    );
    assert_eq!(
        send(
            &f.app,
            Method::POST,
            "/api/v2/operator/register",
            None,
            body
        )
        .await?
        .status,
        StatusCode::CREATED
    );
    Ok(())
}

#[tokio::test]
async fn capacity_rejection_is_bounded_and_keeps_existing_sessions() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = Fixture::new().await?;
    let invite = f.invite("viewer").await?;
    let recovery = f.recovery(&f.admin_id.to_string()).await?;
    let held = occupy_sign_in_capacity().await;
    for (path, cookie, body) in [
        (
            "/api/v2/operator/register",
            None,
            registration(&invite, "new"),
        ),
        (
            "/api/v2/operator/password/reset",
            None,
            reset_form(&recovery),
        ),
        (
            "/api/v2/operator/password/change",
            Some(f.admin_cookie.as_str()),
            change_form(),
        ),
    ] {
        let response = f.request(Method::POST, path, cookie, body).await?;
        assert_error(
            &response,
            StatusCode::SERVICE_UNAVAILABLE,
            "SERVICE_UNAVAILABLE",
        )?;
        assert_eq!(response.headers[header::RETRY_AFTER], "1");
    }
    // Anonymous capacity rejection must precede the authorization DB lookup.
    let connections = f.database.database.test_exhaust_pool();
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        f.request(
            Method::POST,
            "/api/v2/operator/register",
            None,
            registration(&invite, "new"),
        ),
    )
    .await??;
    assert_eq!(response.status, StatusCode::SERVICE_UNAVAILABLE);
    drop(connections);
    drop(held);
    assert_eq!(
        f.request(
            Method::GET,
            "/api/v2/session",
            Some(&f.admin_cookie),
            json!({})
        )
        .await?
        .status,
        StatusCode::OK
    );
    assert_eq!(
        f.state
            .operator()
            .inspect_invitation(invite["token"].as_str().ok_or("missing token")?.to_owned())
            .await?
            .role,
        crate::component::operator::OperatorRole::Viewer
    );
    Ok(())
}

#[tokio::test]
async fn infrastructure_errors_are_redacted_and_do_not_consume_grants() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = Fixture::new().await?;
    let invite = f.invite("viewer").await?;
    f.database.database.write(|tx| {
        use diesel::connection::SimpleConnection;
        tx.connection().batch_execute("CREATE TRIGGER reject_invite_consumption BEFORE DELETE ON operator_invitations BEGIN SELECT RAISE(ABORT, 'private-sql-error-canary'); END")
            .map_err(|_|crate::db::PersistenceError::OperationFailed)
    }).await.map_err(crate::db::TransactionError::into_error)?;
    let response = f
        .request(
            Method::POST,
            "/api/v2/operator/register",
            None,
            registration(&invite, "new"),
        )
        .await?;
    assert_error(
        &response,
        StatusCode::INTERNAL_SERVER_ERROR,
        "INTERNAL_ERROR",
    )?;
    let body = String::from_utf8(response.body)?;
    for secret in [
        "private-sql-error-canary",
        PASSWORD,
        invite["token"].as_str().ok_or("missing token")?,
    ] {
        assert!(!body.contains(secret));
    }
    assert_eq!(
        f.request(
            Method::POST,
            "/api/v2/operator/register/inspect",
            None,
            json!({"token":invite["token"]})
        )
        .await?
        .status,
        StatusCode::OK
    );
    let accounts = f
        .request(
            Method::GET,
            "/api/v2/operator/accounts",
            Some(&f.admin_cookie),
            json!({}),
        )
        .await?;
    assert_eq!(
        serde_json::from_slice::<Value>(&accounts.body)?
            .as_array()
            .ok_or("not a list")?
            .len(),
        1
    );
    Ok(())
}

#[tokio::test]
async fn secret_forms_and_issued_links_do_not_escape_into_trace_logs() -> TestResult {
    use crate::{
        config::LogLevel,
        logging::tests::{CapturedLogs, SubscriberTestGuard},
    };
    use tracing::instrument::WithSubscriber as _;
    // Match the existing HTTP logging tests: subscriber before password work.
    let _subscriber = SubscriberTestGuard::acquire();
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = Fixture::new().await?;
    let captured = CapturedLogs::default();
    let subscriber = captured.subscriber(LogLevel::Trace);
    let (token,recovery)=async {
        let invite=f.invite("viewer").await?;
        let token=invite["token"].as_str().ok_or("missing token")?.to_owned();
        f.request(Method::POST,"/api/v2/operator/register/inspect",None,json!({"token":token})).await?;
        let response=f.request(Method::POST,"/api/v2/operator/register",None,registration(&invite,"new")).await?;
        let account:Value=serde_json::from_slice(&response.body)?;
        let recovery=f.recovery(account["operator_id"].as_str().ok_or("missing id")?).await?;
        f.request(Method::POST,"/api/v2/operator/password/reset",None,reset_form(&recovery)).await?;
        f.request(Method::POST,"/api/v2/operator/password/change",Some(&f.admin_cookie),json!({"current_password":"wrong-password-canary","password":NEW_PASSWORD,"password_confirmation":NEW_PASSWORD})).await?;
        Ok::<_,Box<dyn std::error::Error>>((token,recovery["token"].as_str().ok_or("missing token")?.to_owned()))
    }.with_subscriber(subscriber).await?;
    let text = captured.text().map_err(|()| "log capture failed")?;
    for secret in [
        &token,
        &recovery,
        PASSWORD,
        NEW_PASSWORD,
        "wrong-password-canary",
    ] {
        assert!(!text.contains(secret));
    }
    assert!(text.contains("operator_current_password_invalid"));
    Ok(())
}

fn held_management_body(
    bytes: &'static [u8],
) -> (
    Body,
    tokio::sync::oneshot::Receiver<()>,
    tokio::sync::oneshot::Sender<()>,
) {
    use std::{future::Future as _, pin::Pin, task::Poll};
    use tokio::sync::oneshot;
    let (entered, polled) = oneshot::channel();
    let (release, mut released) = oneshot::channel();
    let mut entered = Some(entered);
    let mut bytes = Some(axum::body::Bytes::from_static(bytes));
    let body = Body::from_stream(futures_util::stream::poll_fn(move |cx| {
        if let Some(entered) = entered.take() {
            let _ = entered.send(());
        }
        if bytes.is_none() {
            return Poll::Ready(None);
        }
        match Pin::new(&mut released).poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(_) => Poll::Ready(bytes.take().map(Ok::<_, std::io::Error>)),
        }
    }));
    (body, polled, release)
}

#[tokio::test]
async fn authorized_account_change_finishes_after_caller_demotion_or_deletion() -> TestResult {
    use std::time::Duration;
    use tower::ServiceExt as _;

    let _guard = PasswordVerificationTestGuard::acquire().await;
    for delete in [false, true] {
        let f = Fixture::new().await?;
        let second = f.signup("second", "admin").await?;
        let cookie = login(&f.app, "second", PASSWORD).await?;
        let target = f.signup("target", "viewer").await?;
        let second_id = second["operator_id"].as_str().ok_or("missing id")?;
        let path = format!(
            "/api/v2/operator/accounts/{}",
            target["operator_id"].as_str().ok_or("missing id")?
        );
        let (body, polled, release) = held_management_body(br#"{"role":"admin"}"#);
        let pending = tokio::spawn(
            f.app.clone().oneshot(
                Request::builder()
                    .method(Method::PATCH)
                    .uri(&path)
                    .header(header::COOKIE, &cookie)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(body)?,
            ),
        );
        // Body polling proves both real authorization middleware layers completed.
        tokio::time::timeout(Duration::from_secs(5), polled).await??;
        let changed = f
            .request(
                if delete {
                    Method::DELETE
                } else {
                    Method::PATCH
                },
                &format!("/api/v2/operator/accounts/{second_id}"),
                Some(&f.admin_cookie),
                json!({"role":"viewer"}),
            )
            .await?;
        assert_eq!(changed.status, StatusCode::NO_CONTENT);
        let denied = drive(
            &f.app,
            Request::builder()
                .method(Method::PATCH)
                .uri(&path)
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(unread_management_body())?,
        )
        .await?;
        assert_eq!(
            denied.status,
            if delete {
                StatusCode::UNAUTHORIZED
            } else {
                StatusCode::FORBIDDEN
            }
        );
        release.send(()).map_err(|()| "body release")?;
        assert_eq!(pending.await??.status(), StatusCode::NO_CONTENT);
        let accounts = f.state.operator().list_accounts().await?;
        assert_eq!(
            accounts
                .iter()
                .find(|account| account.username == "target")
                .ok_or("missing target")?
                .role,
            crate::component::operator::OperatorRole::Admin
        );
    }
    Ok(())
}

#[tokio::test]
async fn authorized_invitation_issuance_rechecks_persisted_issuer_not_caller_role() -> TestResult {
    use std::time::Duration;
    use tower::ServiceExt as _;

    let _guard = PasswordVerificationTestGuard::acquire().await;
    for delete in [false, true] {
        let f = Fixture::new().await?;
        let second = f.signup("second", "admin").await?;
        let cookie = login(&f.app, "second", PASSWORD).await?;
        let second_id = second["operator_id"].as_str().ok_or("missing id")?;
        let (body, polled, release) = held_management_body(br#"{"role":"viewer"}"#);
        let pending = tokio::spawn(
            f.app.clone().oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v2/operator/invitations")
                    .header(header::COOKIE, &cookie)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(body)?,
            ),
        );
        tokio::time::timeout(Duration::from_secs(5), polled).await??;
        let changed = f
            .request(
                if delete {
                    Method::DELETE
                } else {
                    Method::PATCH
                },
                &format!("/api/v2/operator/accounts/{second_id}"),
                Some(&f.admin_cookie),
                json!({"role":"viewer"}),
            )
            .await?;
        assert_eq!(changed.status, StatusCode::NO_CONTENT);
        release.send(()).map_err(|()| "body release")?;
        let response = pending.await??;
        assert_eq!(response.status(), StatusCode::GONE);
        let body: Value =
            serde_json::from_slice(&axum::body::to_bytes(response.into_body(), usize::MAX).await?)?;
        assert_eq!(body["code"], "OPERATOR_LINK_UNAVAILABLE");
        assert!(f.state.operator().list_invitations().await?.is_empty());
    }
    Ok(())
}
