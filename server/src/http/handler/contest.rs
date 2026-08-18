use axum::{
    Router,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Serialize;

use crate::audit::CorrelationId;

use super::super::{AppState, middleware};

pub(crate) mod account;
pub(crate) mod binding;
pub(crate) mod seat;

pub(in crate::http) fn routes(state: AppState) -> Router<AppState> {
    Router::new()
        .merge(seat::routes(state.clone()))
        .merge(account::routes(state.clone()))
        .merge(binding::routes(state))
}

fn current_facts_response<T: Serialize>(facts: &[T], correlation_id: CorrelationId) -> Response {
    let body = serde_json::to_string(&facts).unwrap_or_else(|_| {
        tracing::error!(
            correlation_id = %correlation_id.as_text(),
            "current facts response serialization invariant failed"
        );
        panic!("current facts response serialization invariant failed");
    });
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        body,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{Method, Request, StatusCode, header},
    };
    use serde_json::Value;
    use snafu::Snafu;
    use uuid::Uuid;

    use crate::{
        application::operator::{OperatorRole, sign_in, tests::PasswordVerificationTestGuard},
        audit::CorrelationId,
        db::{Database, contest as db_contest, device as db_device},
    };

    use super::super::super::{
        middleware::CORRELATION_ID_HEADER,
        router,
        tests::{
            Captured, SupportFailure, TestDatabase, drive, header_text, seed_operator,
            unused_vault_master_key, unused_web_root,
        },
    };

    const PASSWORD: &str = "contest-read-password-canary";
    const VAULT_POINTER_CANARY: &str = "vault-pointer-secret-storage-canary";
    const HARDWARE_ID_CANARY: &str = "machine-hardware-id-full-canary-7d58f1";
    const ROUTES: [&str; 4] = [
        "/api/v2/seats",
        "/api/v2/accounts",
        "/api/v2/devices",
        "/api/v2/bindings",
    ];

    #[tokio::test]
    async fn admin_and_viewer_read_exact_redacted_current_facts_without_writes()
    -> Result<(), TestFailure> {
        let fixture = TestDatabase::new().await?;
        db_device::tests::test_seed_current_facts(&fixture.database, HARDWARE_ID_CANARY)
            .await
            .map_err(|_| TestFailure::FixtureFailed)?;
        db_contest::tests::test_seed_current_facts(&fixture.database, VAULT_POINTER_CANARY)
            .await
            .map_err(|_| TestFailure::FixtureFailed)?;
        seed_contest_operator(&fixture.database, "contest-admin", OperatorRole::Admin).await?;
        seed_contest_operator(&fixture.database, "contest-viewer", OperatorRole::Viewer).await?;
        let admin_cookie = session_cookie(&fixture.database, "contest-admin").await?;
        let viewer_cookie = session_cookie(&fixture.database, "contest-viewer").await?;
        let application = router(
            fixture.database.clone(),
            unused_vault_master_key(),
            unused_web_root(),
        );
        let mut observer = db_contest::tests::test_observer(&fixture.path)
            .map_err(|_| TestFailure::DatabaseEvidenceFailed)?;
        let before = db_contest::tests::test_snapshot(&fixture.database, &mut observer)
            .await
            .map_err(|_| TestFailure::DatabaseEvidenceFailed)?;

        for (path, expected) in expected_current_facts() {
            let admin = drive(&application, request(path, Some(&admin_cookie))?).await?;
            let viewer = drive(&application, request(path, Some(&viewer_cookie))?).await?;
            verify_current_facts(path, &admin, &viewer, &expected)?;
        }

        let after = db_contest::tests::test_snapshot(&fixture.database, &mut observer)
            .await
            .map_err(|_| TestFailure::DatabaseEvidenceFailed)?;
        if after != before {
            return Err(TestFailure::ReadChangedPersistence);
        }
        Ok(())
    }

    #[tokio::test]
    async fn empty_current_fact_sets_return_empty_arrays() -> Result<(), TestFailure> {
        let fixture = TestDatabase::new().await?;
        seed_contest_operator(&fixture.database, "empty-admin", OperatorRole::Admin).await?;
        let cookie = session_cookie(&fixture.database, "empty-admin").await?;
        let application = router(
            fixture.database.clone(),
            unused_vault_master_key(),
            unused_web_root(),
        );
        for path in ROUTES {
            let response = drive(&application, request(path, Some(&cookie))?).await?;
            if response.status != StatusCode::OK
                || response.body != b"[]"
                || header_text(&response.headers, &header::CONTENT_TYPE)? != "application/json"
            {
                return Err(TestFailure::EmptySetContractChanged);
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn every_read_route_unifies_all_session_credential_failures() -> Result<(), TestFailure> {
        let fixture = TestDatabase::new().await?;
        seed_contest_operator(&fixture.database, "failure-admin", OperatorRole::Admin).await?;
        let mut expired_cookies = Vec::with_capacity(ROUTES.len());
        for _path in ROUTES {
            expired_cookies.push(session_cookie(&fixture.database, "failure-admin").await?);
        }
        db_contest::tests::test_expire_all_sessions(&fixture.database)
            .await
            .map_err(|_| TestFailure::FixtureFailed)?;
        let application = router(
            fixture.database.clone(),
            unused_vault_master_key(),
            unused_web_root(),
        );
        let mut expected = None;

        for (path, expired_cookie) in ROUTES.into_iter().zip(expired_cookies) {
            for cookie in [
                None,
                Some("__Secure-natsume_session=NOT-LOWERCASE-HEX"),
                Some(concat!(
                    "__Secure-natsume_session=",
                    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                )),
                Some(expired_cookie.as_str()),
            ] {
                let response = drive(&application, request(path, cookie)?).await?;
                let normalized = normalized_authentication_error_response(&response)?;
                if expected
                    .as_ref()
                    .is_some_and(|expected| expected != &normalized)
                {
                    return Err(TestFailure::AuthenticationFailuresDiverged);
                }
                expected.get_or_insert(normalized);
            }
        }
        Ok(())
    }

    fn expected_current_facts() -> [(&'static str, Value); 4] {
        [
            (
                "/api/v2/seats",
                serde_json::json!([
                    {"seat_id":"seat-a","seat_code":"A-01"},
                    {"seat_id":"seat-b","seat_code":"B-02"}
                ]),
            ),
            (
                "/api/v2/accounts",
                serde_json::json!([
                    {"account_id":"account-a","domjudge_username":"team-alpha","credential_revision":3},
                    {"account_id":"account-b","domjudge_username":"team-beta","credential_revision":7}
                ]),
            ),
            (
                "/api/v2/devices",
                serde_json::json!([
                    {"device_id":"01900000-0000-7000-8000-000000000001","state":"enrolled","hardware_identity_quality":"strong"},
                    {"device_id":"01900000-0000-7000-8000-000000000002","state":"disabled","hardware_identity_quality":"medium"}
                ]),
            ),
            (
                "/api/v2/bindings",
                serde_json::json!([
                    {"seat_id":"seat-a","device_id":"01900000-0000-7000-8000-000000000001","binding_revision":11},
                    {"seat_id":"seat-b","device_id":"01900000-0000-7000-8000-000000000002","binding_revision":11}
                ]),
            ),
        ]
    }

    fn verify_current_facts(
        path: &str,
        admin: &Captured,
        viewer: &Captured,
        expected: &Value,
    ) -> Result<(), TestFailure> {
        if admin.status != StatusCode::OK
            || viewer.status != StatusCode::OK
            || admin.body != viewer.body
            || header_text(&admin.headers, &header::CONTENT_TYPE)? != "application/json"
            || serde_json::from_slice::<Value>(&admin.body)
                .map_err(|_| TestFailure::ResponseJsonInvalid)?
                != *expected
        {
            return Err(TestFailure::CurrentFactsChanged);
        }
        let encoded =
            std::str::from_utf8(&admin.body).map_err(|_| TestFailure::ResponseBodyFailed)?;
        if encoded.contains(VAULT_POINTER_CANARY)
            || encoded.contains(HARDWARE_ID_CANARY)
            || encoded.contains("credential_vault_record_id")
            || encoded.contains("machine_hardware_id")
            || encoded.to_ascii_lowercase().contains("password")
            || (!ROUTES.contains(&path) || expected.as_array().is_none())
        {
            return Err(TestFailure::RedactedFactEscaped);
        }
        Ok(())
    }

    async fn seed_contest_operator(
        database: &Database,
        login_name: &str,
        role: OperatorRole,
    ) -> Result<(), TestFailure> {
        seed_operator(database, login_name, role, PASSWORD).await?;
        Ok(())
    }

    async fn session_cookie(database: &Database, login_name: &str) -> Result<String, TestFailure> {
        let _verification_guard = PasswordVerificationTestGuard::acquire().await;
        let session = sign_in(
            database,
            CorrelationId::from_uuid(Uuid::now_v7()),
            login_name,
            PASSWORD.to_owned(),
        )
        .await
        .map_err(|_| TestFailure::FixtureFailed)?;
        Ok(format!(
            "__Secure-natsume_session={}",
            session.credential().to_wire().expose()
        ))
    }

    fn request(path: &str, cookie: Option<&str>) -> Result<Request<Body>, TestFailure> {
        let mut request = Request::builder().method(Method::GET).uri(path);
        if let Some(cookie) = cookie {
            request = request.header(header::COOKIE, cookie);
        }
        request
            .body(Body::empty())
            .map_err(|_| TestFailure::RequestBuildFailed)
    }

    fn normalized_authentication_error_response(
        response: &Captured,
    ) -> Result<String, TestFailure> {
        normalized_error_response(
            response,
            StatusCode::UNAUTHORIZED,
            "Unauthorized",
            "AUTHENTICATION_FAILED",
        )
        .map_err(|_| TestFailure::AuthenticationErrorResponseChanged)
    }

    fn normalized_error_response(
        response: &Captured,
        status: StatusCode,
        title: &str,
        code: &str,
    ) -> Result<String, TestFailure> {
        if response.status != status
            || header_text(&response.headers, &header::CONTENT_TYPE)? != "application/json"
        {
            return Err(TestFailure::ErrorResponseChanged);
        }
        let correlation = header_text(&response.headers, &CORRELATION_ID_HEADER)?;
        let parsed = Uuid::parse_str(correlation).map_err(|_| TestFailure::ErrorResponseChanged)?;
        let mut value: Value =
            serde_json::from_slice(&response.body).map_err(|_| TestFailure::ResponseJsonInvalid)?;
        let object = value
            .as_object_mut()
            .ok_or(TestFailure::ErrorResponseChanged)?;
        if parsed.get_version_num() != 7
            || object.len() != 4
            || object.get("title").and_then(Value::as_str) != Some(title)
            || object.get("status").and_then(Value::as_u64) != Some(u64::from(status.as_u16()))
            || object.get("code").and_then(Value::as_str) != Some(code)
            || object.get("correlation_id").and_then(Value::as_str) != Some(correlation)
        {
            return Err(TestFailure::ErrorResponseChanged);
        }
        object.insert(
            "correlation_id".to_owned(),
            Value::String("NORMALIZED".to_owned()),
        );
        serde_json::to_string(&value).map_err(|_| TestFailure::ResponseJsonInvalid)
    }

    #[derive(Debug, Snafu)]
    enum TestFailure {
        #[snafu(display("an HTTP test helper failed"))]
        #[snafu(context(false))]
        Support { source: SupportFailure },
        #[snafu(display("the contest HTTP fixture failed"))]
        FixtureFailed,
        #[snafu(display("the contest HTTP request could not be built"))]
        RequestBuildFailed,
        #[snafu(display("the contest HTTP response body failed"))]
        ResponseBodyFailed,
        #[snafu(display("a contest HTTP response was not valid JSON"))]
        ResponseJsonInvalid,
        #[snafu(display("contest current facts changed at the HTTP boundary"))]
        CurrentFactsChanged,
        #[snafu(display("a redacted contest fact escaped the HTTP boundary"))]
        RedactedFactEscaped,
        #[snafu(display("a contest read changed persisted state"))]
        ReadChangedPersistence,
        #[snafu(display("an empty contest set did not return an empty array"))]
        EmptySetContractChanged,
        #[snafu(display("a contest authentication error response changed"))]
        AuthenticationErrorResponseChanged,
        #[snafu(display("contest authentication failures diverged"))]
        AuthenticationFailuresDiverged,
        #[snafu(display("contest database evidence could not be read"))]
        DatabaseEvidenceFailed,
        #[snafu(display("a contest authentication error response changed"))]
        ErrorResponseChanged,
    }
}
