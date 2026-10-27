use std::collections::BTreeSet;

use axum::{
    Router,
    body::Body,
    extract::ConnectInfo,
    http::{Method, Request, StatusCode, header},
    middleware,
    routing::post,
};
use diesel::{ExpressionMethods as _, QueryDsl as _, RunQueryDsl as _};
use tower::ServiceExt as _;

use super::{
    Captured, LOGIN_NAME, PASSWORD, PasswordVerificationTestGuard, SupportFailure, TestDatabase,
    drive, header_text, login_request, router, seed_operator, server_state, unused_web_root,
};
use crate::tls::PeerAddress;

type TestResult = Result<(), crate::test_error::TestError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Principal {
    Public,
    Operator,
    PrintSource,
    PrintClient,
}

struct Route {
    principal: Principal,
    admin: bool,
    method: Method,
    path: &'static str,
}

const INVENTORY: &[Route] = &[
    Route {
        principal: Principal::Public,
        admin: false,
        method: Method::GET,
        path: "/device/control",
    },
    Route {
        principal: Principal::Public,
        admin: false,
        method: Method::GET,
        path: "/health",
    },
    Route {
        principal: Principal::Public,
        admin: false,
        method: Method::POST,
        path: "/operator/password/reset",
    },
    Route {
        principal: Principal::Public,
        admin: false,
        method: Method::POST,
        path: "/operator/password/reset/inspect",
    },
    Route {
        principal: Principal::Public,
        admin: false,
        method: Method::POST,
        path: "/operator/register",
    },
    Route {
        principal: Principal::Public,
        admin: false,
        method: Method::POST,
        path: "/operator/register/inspect",
    },
    Route {
        principal: Principal::Public,
        admin: false,
        method: Method::GET,
        path: "/organizations/{organization_id}/logo",
    },
    Route {
        principal: Principal::Public,
        admin: false,
        method: Method::POST,
        path: "/print/client/confirmation",
    },
    Route {
        principal: Principal::Public,
        admin: false,
        method: Method::POST,
        path: "/print/client/pair",
    },
    Route {
        principal: Principal::Public,
        admin: false,
        method: Method::DELETE,
        path: "/session",
    },
    Route {
        principal: Principal::Public,
        admin: false,
        method: Method::POST,
        path: "/session",
    },
    Route {
        principal: Principal::Operator,
        admin: false,
        method: Method::GET,
        path: "/accounts",
    },
    Route {
        principal: Principal::Operator,
        admin: false,
        method: Method::GET,
        path: "/bindings",
    },
    Route {
        principal: Principal::Operator,
        admin: false,
        method: Method::GET,
        path: "/devices",
    },
    Route {
        principal: Principal::Operator,
        admin: false,
        method: Method::GET,
        path: "/devices/{device_id}",
    },
    Route {
        principal: Principal::Operator,
        admin: true,
        method: Method::PATCH,
        path: "/devices/{device_id}",
    },
    Route {
        principal: Principal::Operator,
        admin: true,
        method: Method::DELETE,
        path: "/devices/{device_id}/binding",
    },
    Route {
        principal: Principal::Operator,
        admin: false,
        method: Method::GET,
        path: "/devices/{device_id}/convergence",
    },
    Route {
        principal: Principal::Operator,
        admin: false,
        method: Method::GET,
        path: "/devices/{device_id}/home",
    },
    Route {
        principal: Principal::Operator,
        admin: false,
        method: Method::GET,
        path: "/devices/{device_id}/session-control",
    },
    Route {
        principal: Principal::Operator,
        admin: false,
        method: Method::GET,
        path: "/enrollment-reviews",
    },
    Route {
        principal: Principal::Operator,
        admin: true,
        method: Method::POST,
        path: "/enrollment-reviews/{review_id}/actions/approve",
    },
    Route {
        principal: Principal::Operator,
        admin: true,
        method: Method::POST,
        path: "/enrollment-reviews/{review_id}/actions/deny",
    },
    Route {
        principal: Principal::Operator,
        admin: true,
        method: Method::GET,
        path: "/exports/domjudge",
    },
    Route {
        principal: Principal::Operator,
        admin: true,
        method: Method::GET,
        path: "/imports",
    },
    Route {
        principal: Principal::Operator,
        admin: true,
        method: Method::POST,
        path: "/imports",
    },
    Route {
        principal: Principal::Operator,
        admin: true,
        method: Method::GET,
        path: "/imports/template",
    },
    Route {
        principal: Principal::Operator,
        admin: true,
        method: Method::DELETE,
        path: "/imports/{import_id}",
    },
    Route {
        principal: Principal::Operator,
        admin: true,
        method: Method::POST,
        path: "/imports/{import_id}/actions/commit",
    },
    Route {
        principal: Principal::Operator,
        admin: true,
        method: Method::GET,
        path: "/imports/{import_id}/organizations",
    },
    Route {
        principal: Principal::Operator,
        admin: true,
        method: Method::GET,
        path: "/imports/{import_id}/organizations/{organization_id}/logo",
    },
    Route {
        principal: Principal::Operator,
        admin: true,
        method: Method::GET,
        path: "/operator/accounts",
    },
    Route {
        principal: Principal::Operator,
        admin: true,
        method: Method::DELETE,
        path: "/operator/accounts/{operator_id}",
    },
    Route {
        principal: Principal::Operator,
        admin: true,
        method: Method::PATCH,
        path: "/operator/accounts/{operator_id}",
    },
    Route {
        principal: Principal::Operator,
        admin: true,
        method: Method::POST,
        path: "/operator/accounts/{operator_id}/password-resets",
    },
    Route {
        principal: Principal::Operator,
        admin: true,
        method: Method::GET,
        path: "/operator/invitations",
    },
    Route {
        principal: Principal::Operator,
        admin: true,
        method: Method::POST,
        path: "/operator/invitations",
    },
    Route {
        principal: Principal::Operator,
        admin: true,
        method: Method::DELETE,
        path: "/operator/invitations/{invite_id}",
    },
    Route {
        principal: Principal::Operator,
        admin: true,
        method: Method::POST,
        path: "/operator/invitations/{invite_id}/actions/regenerate",
    },
    Route {
        principal: Principal::Operator,
        admin: false,
        method: Method::POST,
        path: "/operator/password/change",
    },
    Route {
        principal: Principal::Operator,
        admin: true,
        method: Method::GET,
        path: "/organizations",
    },
    Route {
        principal: Principal::Operator,
        admin: false,
        method: Method::GET,
        path: "/print/executions",
    },
    Route {
        principal: Principal::Operator,
        admin: true,
        method: Method::POST,
        path: "/print/executions/{ready_sequence}/requeue",
    },
    Route {
        principal: Principal::Operator,
        admin: true,
        method: Method::GET,
        path: "/print/printers",
    },
    Route {
        principal: Principal::Operator,
        admin: true,
        method: Method::POST,
        path: "/print/printers",
    },
    Route {
        principal: Principal::Operator,
        admin: true,
        method: Method::DELETE,
        path: "/print/printers/{printer_id}/binding",
    },
    Route {
        principal: Principal::Operator,
        admin: true,
        method: Method::POST,
        path: "/print/printers/{printer_id}/pairing-code",
    },
    Route {
        principal: Principal::Operator,
        admin: false,
        method: Method::GET,
        path: "/print/queue",
    },
    Route {
        principal: Principal::Operator,
        admin: true,
        method: Method::GET,
        path: "/print/sources",
    },
    Route {
        principal: Principal::Operator,
        admin: true,
        method: Method::POST,
        path: "/print/sources",
    },
    Route {
        principal: Principal::Operator,
        admin: true,
        method: Method::DELETE,
        path: "/print/sources/{source_id}",
    },
    Route {
        principal: Principal::Operator,
        admin: false,
        method: Method::GET,
        path: "/provisioning-window",
    },
    Route {
        principal: Principal::Operator,
        admin: true,
        method: Method::PUT,
        path: "/provisioning-window",
    },
    Route {
        principal: Principal::Operator,
        admin: false,
        method: Method::GET,
        path: "/seats",
    },
    Route {
        principal: Principal::Operator,
        admin: false,
        method: Method::GET,
        path: "/session",
    },
    Route {
        principal: Principal::Operator,
        admin: true,
        method: Method::POST,
        path: "/target-submissions",
    },
    Route {
        principal: Principal::PrintSource,
        admin: false,
        method: Method::POST,
        path: "/print/requests",
    },
    Route {
        principal: Principal::PrintSource,
        admin: false,
        method: Method::GET,
        path: "/print/requests/{request_id}",
    },
    Route {
        principal: Principal::PrintSource,
        admin: false,
        method: Method::PUT,
        path: "/print/requests/{request_id}/document",
    },
    Route {
        principal: Principal::PrintSource,
        admin: false,
        method: Method::POST,
        path: "/print/requests/{request_id}/generation-failure",
    },
    Route {
        principal: Principal::PrintClient,
        admin: false,
        method: Method::GET,
        path: "/print/client/attempts/current",
    },
    Route {
        principal: Principal::PrintClient,
        admin: false,
        method: Method::GET,
        path: "/print/client/attempts/{attempt_id}",
    },
    Route {
        principal: Principal::PrintClient,
        admin: false,
        method: Method::GET,
        path: "/print/client/attempts/{attempt_id}/document",
    },
    Route {
        principal: Principal::PrintClient,
        admin: false,
        method: Method::PUT,
        path: "/print/client/attempts/{attempt_id}/job",
    },
    Route {
        principal: Principal::PrintClient,
        admin: false,
        method: Method::POST,
        path: "/print/client/attempts/{attempt_id}/reports",
    },
    Route {
        principal: Principal::PrintClient,
        admin: false,
        method: Method::POST,
        path: "/print/client/claims",
    },
];

fn path(template: &str) -> String {
    template
        .split('/')
        .map(|segment| {
            if segment == "{organization_id}" {
                "INST-001"
            } else if segment == "{ready_sequence}" {
                "1"
            } else if segment.starts_with('{') {
                "01900000-0000-7000-8000-000000000001"
            } else {
                segment
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn request(route: &Route) -> Result<Request<Body>, axum::http::Error> {
    Request::builder()
        .method(route.method.clone())
        .uri(format!("/api/v2{}", path(route.path)))
        .header(header::CONTENT_TYPE, "application/json")
        .extension(ConnectInfo(PeerAddress(
            "192.0.2.1"
                .parse()
                .unwrap_or_else(|error| panic!("IP: {error}")),
        )))
        .body(Body::from("{}"))
}

pub(super) async fn mounted_route_responses(
    application: &Router,
) -> Result<Vec<(Captured, StatusCode)>, SupportFailure> {
    let mut responses = Vec::new();
    for route in INVENTORY {
        let expected = match (route.principal, route.method.clone(), route.path) {
            (Principal::Public, Method::GET, "/health") => StatusCode::OK,
            (Principal::Public, Method::DELETE, "/session") => StatusCode::NO_CONTENT,
            (Principal::Public, _, "/organizations/{organization_id}/logo") => {
                StatusCode::NOT_FOUND
            }
            (Principal::Public, _, "/print/client/confirmation") => StatusCode::UNAUTHORIZED,
            (Principal::Public, _, _) => StatusCode::BAD_REQUEST,
            _ => StatusCode::UNAUTHORIZED,
        };
        let response = drive(
            application,
            request(route).map_err(|source| {
                SupportFailure::from(crate::test_error::TestError::from(source))
            })?,
        )
        .await?;
        assert_eq!(
            response.status, expected,
            "{} {} {:?}",
            route.method, route.path, route.principal
        );
        responses.push((response, expected));
    }
    Ok(responses)
}

#[tokio::test]
async fn inventory_exactly_matches_every_group_path_and_method() -> TestResult {
    let fixture = TestDatabase::new().await?;
    let state = server_state(&fixture)?;
    let mut actual = BTreeSet::new();
    for (principal, routes) in [
        Principal::Public,
        Principal::Operator,
        Principal::PrintSource,
        Principal::PrintClient,
    ]
    .into_iter()
    .zip(super::super::route_groups(&state))
    {
        for (template, methods) in routes {
            let probe = Router::new()
                .route(
                    template,
                    methods.route_layer(middleware::from_fn(
                        |_request: axum::extract::Request, _next: axum::middleware::Next| async {
                            StatusCode::NO_CONTENT
                        },
                    )),
                )
                .with_state(state.clone());
            for method in [
                Method::GET,
                Method::POST,
                Method::PUT,
                Method::PATCH,
                Method::DELETE,
                Method::OPTIONS,
                Method::CONNECT,
                Method::TRACE,
            ] {
                let response = probe
                    .clone()
                    .oneshot(
                        Request::builder()
                            .method(method.clone())
                            .uri(path(template))
                            .body(Body::empty())?,
                    )
                    .await?;
                if response.status() == StatusCode::NO_CONTENT {
                    assert!(
                        actual.insert((principal, method.to_string(), template)),
                        "duplicate registration: {method} {template}"
                    );
                } else {
                    assert_eq!(
                        response.status(),
                        StatusCode::METHOD_NOT_ALLOWED,
                        "probe: {method} {template}"
                    );
                }
            }
        }
    }
    let expected = INVENTORY
        .iter()
        .map(|route| (route.principal, route.method.to_string(), route.path))
        .collect::<BTreeSet<_>>();
    assert_eq!(expected.len(), INVENTORY.len(), "duplicate inventory row");
    assert_eq!(actual, expected);
    Ok(())
}

#[tokio::test]
async fn every_principal_and_admin_boundary_is_enforced() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let fixture = TestDatabase::new().await?;
    let viewer = seed_operator(&fixture.database, LOGIN_NAME, PASSWORD).await?;
    let state = server_state(&fixture)?;
    let application = router(state.clone(), unused_web_root());
    let login = drive(&application, login_request(LOGIN_NAME, PASSWORD)?).await?;
    assert_eq!(login.status, StatusCode::OK);
    let cookie = header_text(&login.headers, &header::SET_COOKIE)?
        .split(';')
        .next()
        .ok_or(crate::test_error::TestError::missing("cookie"))?;
    fixture
        .database
        .write(move |transaction| {
            use crate::diesel_schema::operator_accounts;
            diesel::update(
                operator_accounts::table
                    .filter(operator_accounts::operator_id.eq(viewer.to_string())),
            )
            .set(operator_accounts::role.eq("viewer"))
            .execute(transaction.connection())
            .map(|_| ())
            .map_err(crate::db::PersistenceError::from)
        })
        .await?;
    let printer = state
        .print()
        .create_printer("inventory printer".to_owned())
        .await?;
    let code = state.print().issue_pairing_code(printer.printer_id).await?;
    let pending_client = state.print().pair_client(&code.code).await?;
    for route in INVENTORY {
        assert_principal_boundary(&application, route, cookie, &pending_client.token).await?;
    }
    for route in INVENTORY.iter().filter(|route| {
        route.path.starts_with("/operator/") && route.principal == Principal::Public
    }) {
        let response = drive(
            &application,
            Request::builder()
                .method(Method::GET)
                .uri(format!("/api/v2{}", route.path))
                .header(header::COOKIE, cookie)
                .body(Body::empty())?,
        )
        .await?;
        assert_eq!(
            response.status,
            StatusCode::METHOD_NOT_ALLOWED,
            "logged-out-only method fallback: {}",
            route.path
        );
        assert!(response.body.is_empty());
    }

    Ok(())
}

async fn assert_principal_boundary(
    application: &Router,
    route: &Route,
    cookie: &str,
    pending_client_token: &str,
) -> TestResult {
    let mut anonymous = request(route)?;
    if route.path == "/print/client/confirmation" {
        anonymous.headers_mut().insert(
            header::AUTHORIZATION,
            format!("Bearer {pending_client_token}").parse()?,
        );
    }
    let response = drive(application, anonymous).await?;
    if route.principal == Principal::Public {
        assert_ne!(
            response.status,
            StatusCode::UNAUTHORIZED,
            "public: {} {}",
            route.method,
            route.path
        );
        assert_ne!(
            response.status,
            StatusCode::METHOD_NOT_ALLOWED,
            "unmounted: {} {}",
            route.method,
            route.path
        );
    } else {
        assert_eq!(
            response.status,
            StatusCode::UNAUTHORIZED,
            "anonymous: {} {}",
            route.method,
            route.path
        );
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&response.body)?["code"],
            "AUTHENTICATION_FAILED"
        );
        let mut authenticated = request(route)?;
        authenticated
            .headers_mut()
            .insert(header::COOKIE, cookie.parse()?);
        let response = drive(application, authenticated).await?;
        if route.principal == Principal::Operator {
            if route.admin {
                assert_eq!(
                    response.status,
                    StatusCode::FORBIDDEN,
                    "viewer: {} {}",
                    route.method,
                    route.path
                );
            } else {
                assert_ne!(
                    response.status,
                    StatusCode::FORBIDDEN,
                    "viewer: {} {}",
                    route.method,
                    route.path
                );
                assert_ne!(
                    response.status,
                    StatusCode::UNAUTHORIZED,
                    "viewer: {} {}",
                    route.method,
                    route.path
                );
            }
        } else {
            assert_eq!(
                response.status,
                StatusCode::UNAUTHORIZED,
                "wrong principal: {} {}",
                route.method,
                route.path
            );
        }
    }
    assert!(!response.headers.contains_key("x-correlation-id"));
    if route.path.starts_with("/operator/") || route.path.starts_with("/print/") {
        assert_eq!(response.headers[header::CACHE_CONTROL], "no-store");
    }

    Ok(())
}

#[tokio::test]
async fn new_methods_inherit_each_groups_authentication_without_an_opt_in_wrapper() -> TestResult {
    let fixture = TestDatabase::new().await?;
    let state = server_state(&fixture)?;
    for group in [
        super::super::operator_group,
        super::super::source_group,
        super::super::client_group,
    ] {
        let application = group(
            state.clone(),
            vec![("/new-operation", post(|| async { StatusCode::NO_CONTENT }))],
        )
        .with_state(state.clone());
        let response = application
            .oneshot(Request::post("/new-operation").body(Body::empty())?)
            .await?;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    Ok(())
}

#[tokio::test]
async fn method_rejections_and_unmounted_paths_stay_outside_group_authentication() -> TestResult {
    let fixture = TestDatabase::new().await?;
    let application = router(server_state(&fixture)?, unused_web_root());
    for (principal, routes) in [
        Principal::Public,
        Principal::Operator,
        Principal::PrintSource,
        Principal::PrintClient,
    ]
    .into_iter()
    .zip(super::super::route_groups(&server_state(&fixture)?))
    {
        for (template, _) in routes {
            for method in [
                Method::GET,
                Method::POST,
                Method::PUT,
                Method::PATCH,
                Method::DELETE,
                Method::OPTIONS,
                Method::TRACE,
            ] {
                if INVENTORY
                    .iter()
                    .any(|route| route.path == template && route.method == method)
                {
                    continue;
                }
                let response = drive(
                    &application,
                    Request::builder()
                        .method(method.clone())
                        .uri(format!("/api/v2{}", path(template)))
                        .body(Body::empty())?,
                )
                .await?;
                assert_eq!(
                    response.status,
                    StatusCode::METHOD_NOT_ALLOWED,
                    "unsupported: {principal:?} {method} {template}"
                );
            }
            let response = drive(
                &application,
                super::request(Method::HEAD, &format!("/api/v2{}", path(template)), "")?,
            )
            .await?;
            assert_eq!(response.status, StatusCode::NOT_FOUND);
            assert!(response.body.is_empty());
        }
    }
    for path in [
        "/api/v2/unmounted",
        "/api/v2/operator/unmounted",
        "/api/v2/print/unmounted",
    ] {
        let response = drive(&application, super::request(Method::GET, path, "")?).await?;
        assert_eq!(response.status, StatusCode::NOT_FOUND);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&response.body)?["code"],
            "RESOURCE_NOT_FOUND"
        );
    }
    Ok(())
}
