//! Real Server TLS/router, wrapper and pinned Typst integration; no host deployment edits.
use std::{
    net::{Ipv4Addr, SocketAddr},
    os::unix::fs::PermissionsExt as _,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse as _, Response},
    serve::Listener as _,
};
use diesel::{
    QueryableByName, RunQueryDsl as _, sql_query,
    sql_types::{Binary, Text},
};
use natsume_print_domjudge::{Input, Runtime, config::Config, wire::State};
use serde_json::{Value, json};
use tokio::{task::JoinHandle, time::Instant};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{
    LOGIN_NAME, PASSWORD, PasswordVerificationTestGuard, TestDatabase, drive, header_text,
    login_request, router, seed_operator, server_state, unused_web_root,
};
use crate::{
    db::PersistenceError,
    tls::{TlsListener, tests::TestIdentity},
};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[derive(QueryableByName)]
struct StoredDocument {
    #[diesel(sql_type = Binary)]
    pdf: Vec<u8>,
}

struct RunningServer(JoinHandle<std::io::Result<()>>);

impl Drop for RunningServer {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn listen(
    application: axum::Router,
    identity: &TestIdentity,
) -> Result<(RunningServer, String), Box<dyn std::error::Error + Send + Sync>> {
    let listener = TlsListener::bind(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        identity.certificate_path(),
        identity.private_key_path(),
    )
    .await
    .map_err(|_| "TLS listener")?;
    let origin = format!("https://{}", listener.local_addr()?);
    let task = tokio::spawn(async move { axum::serve(listener, application).await });
    Ok((RunningServer(task), origin))
}

async fn run(
    config: &Config,
    runtime: &Runtime,
    input: &Input,
) -> natsume_print_domjudge::Result<natsume_print_domjudge::wire::Fact> {
    natsume_print_domjudge::run(
        config,
        runtime,
        input,
        Instant::now(),
        &CancellationToken::new(),
    )
    .await
}

async fn hide_committed_response(
    axum::extract::State(hide): axum::extract::State<Arc<AtomicBool>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let upload = request.method() == Method::PUT && request.uri().path().ends_with("/document");
    let registration =
        request.method() == Method::POST && request.uri().path() == "/api/v2/print/requests";
    let response = next.run(request).await;
    if ((upload && response.status() == StatusCode::OK)
        || (registration && response.status() == StatusCode::CONFLICT))
        && hide.swap(false, Ordering::SeqCst)
    {
        // Simulate a gateway losing an authoritative admission or conflict response.
        StatusCode::BAD_GATEWAY.into_response()
    } else {
        response
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
#[ignore = "requires deno task prepare print-domjudge and pdftotext"]
async fn real_typst_https_admission_replay_lost_response_restart_and_revocation() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let runtime_root =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/print-domjudge/runtime");
    let runtime = Runtime {
        compiler: runtime_root.join("typst"),
        template: runtime_root.join("template.typ"),
        fonts: runtime_root.join("fonts"),
    };
    assert!(
        runtime.compiler.is_file(),
        "prepare verified Typst runtime first"
    );
    let database = TestDatabase::new().await?;
    seed_operator(&database.database, LOGIN_NAME, PASSWORD).await?;
    let account_id = Uuid::now_v7().to_string();
    database
        .database
        .write(move |tx| {
            sql_query("INSERT INTO accounts VALUES (?, 'team-1', 1)")
                .bind::<Text, _>(&account_id)
                .execute(tx.connection())?;
            sql_query("INSERT INTO organizations VALUES (1,'学校','','School','CHN')")
                .execute(tx.connection())?;
            sql_query("INSERT INTO teams VALUES (?,1,'中文队伍','Team','participant')")
                .bind::<Text, _>(&account_id)
                .execute(tx.connection())?;
            Ok::<_, diesel::result::Error>(())
        })
        .await
        .map_err(|e| format!("seed: {e:?}"))?;
    let application = router(server_state(database.database.clone())?, unused_web_root());
    let login = drive(&application, login_request(LOGIN_NAME, PASSWORD)?).await?;
    let cookie = header_text(&login.headers, &header::SET_COOKIE)?
        .split(';')
        .next()
        .ok_or("cookie")?
        .to_owned();
    let issued = drive(
        &application,
        Request::builder()
            .method(Method::POST)
            .uri("/api/v2/print/sources")
            .header(header::COOKIE, &cookie)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json!({"name":"real-wrapper"}).to_string()))?,
    )
    .await?;
    assert_eq!(issued.status, StatusCode::OK);
    let issued: Value = serde_json::from_slice(&issued.body)?;
    let source_id = issued["source_id"].as_str().ok_or("source_id")?.to_owned();
    let identity = TestIdentity::new(Ipv4Addr::LOCALHOST).map_err(|_| "TLS identity")?;
    let hide = Arc::new(AtomicBool::new(false));
    let tested = application.clone().layer(middleware::from_fn_with_state(
        hide.clone(),
        hide_committed_response,
    ));
    let (_server, origin) = listen(tested, &identity).await?;
    let private = tempfile::tempdir()?;
    let token = private.path().join("token");
    std::fs::write(&token, issued["token"].as_str().ok_or("token")?)?;
    std::fs::set_permissions(&token, std::fs::Permissions::from_mode(0o600))?;
    let config_path = private.path().join("config.toml");
    std::fs::write(
        &config_path,
        format!(
            "server_url = {origin:?}\ntoken_file = {:?}\nca_certificate = {:?}\n",
            token,
            identity.directory_path().join("local-origin-ca.crt")
        ),
    )?;
    let mut config = Config::load(&config_path)?;
    let source = private.path().join("source.cpp");
    std::fs::write(
        &source,
        format!(
            "// 中文源码 #panic(\"not executable\")\n{}FINAL_SOURCE_MARKER",
            "abcdefghij".repeat(1000)
        ),
    )?;
    let mut input = Input {
        file: source,
        account: "team-1".to_owned(),
        original: "main.cpp".to_owned(),
        request_id: Uuid::now_v7().to_string(),
    };
    let first = run(&config, &runtime, &input).await?;
    assert_eq!(first.state, State::Ready);
    let first_admission = first.admission.as_ref().ok_or("admission")?;
    assert_eq!(
        (first_admission.ahead_tasks, first_admission.ahead_pages),
        (0, 0)
    );
    assert_eq!(first.team_name, "中文队伍");
    let first_document = first.document.as_ref().ok_or("document")?;
    let stored_id = input.request_id.clone();
    let pdf = database
        .database
        .read(move |tx| {
            sql_query("SELECT pdf FROM print_documents WHERE request_id = ?")
                .bind::<Text, _>(stored_id)
                .get_result::<StoredDocument>(tx.connection())
                .map_err(|_| PersistenceError::OperationFailed)
        })
        .await
        .map_err(|e| format!("document: {e:?}"))?
        .pdf;
    assert_eq!(pdf.len() as u64, first_document.bytes);
    let pdf_path = private.path().join("admitted.pdf");
    std::fs::write(&pdf_path, &pdf)?;
    let text = std::process::Command::new("pdftotext")
        .arg("-layout")
        .arg(&pdf_path)
        .arg("-")
        .output()?;
    assert!(text.status.success());
    let text = String::from_utf8(text.stdout)?
        .replace('\u{200b}', "")
        .replace('\u{a0}', " ");
    assert!(text.contains("中文队伍") && text.contains("中文源码"));
    let source_text = text
        .split_once("abcdefghij")
        .ok_or("source missing")?
        .1
        .split_once("FINAL_SOURCE_MARKER")
        .ok_or("final source marker missing")?
        .0
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    assert_eq!(source_text, "abcdefghij".repeat(999));
    let unavailable_runtime = Runtime {
        compiler: private.path().join("must-not-execute"),
        ..runtime.clone()
    };
    let replay = run(&config, &unavailable_runtime, &input).await?;
    assert_eq!(
        replay.admission.as_ref().ok_or("replay")?.ready_sequence,
        first_admission.ready_sequence
    );
    input.request_id = Uuid::now_v7().to_string();
    hide.store(true, Ordering::SeqCst);
    let second = run(&config, &runtime, &input).await?;
    assert!(
        !hide.load(Ordering::SeqCst),
        "committed response was hidden"
    );
    let second_admission = second.admission.as_ref().ok_or("second")?;
    assert_eq!(
        (second_admission.ahead_tasks, second_admission.ahead_pages),
        (1, first_document.pages)
    );
    assert!(second_admission.ready_sequence > first_admission.ready_sequence);
    // A hidden registration conflict must not make an old ready task appear successful.
    input.original = "different.cpp".to_owned();
    hide.store(true, Ordering::SeqCst);
    assert_eq!(
        run(&config, &unavailable_runtime, &input).await.err(),
        Some(natsume_print_domjudge::Error::Rejected(409))
    );
    assert!(!hide.load(Ordering::SeqCst));
    input.original = "main.cpp".to_owned();
    let wrong_identity = TestIdentity::new(Ipv4Addr::LOCALHOST).map_err(|_| "wrong trust root")?;
    config.ca_certificate = Some(wrong_identity.directory_path().join("local-origin-ca.crt"));
    assert_eq!(
        run(&config, &unavailable_runtime, &input).await.err(),
        Some(natsume_print_domjudge::Error::Ambiguous)
    );
    config.ca_certificate = Some(identity.directory_path().join("local-origin-ca.crt"));
    let reopened = crate::db::Database::connect_and_migrate(&crate::db::DatabaseConfig::new(
        &database.path,
        false,
    ))
    .await?;
    let restarted_application = router(server_state(reopened)?, unused_web_root());
    let (_restarted_server, origin) = listen(restarted_application, &identity).await?;
    config.server_url = origin;
    let after_reopen = run(&config, &unavailable_runtime, &input).await?;
    assert_eq!(
        after_reopen
            .admission
            .as_ref()
            .ok_or("reopened")?
            .ready_sequence,
        second_admission.ready_sequence
    );
    let revoked = drive(
        &application,
        Request::builder()
            .method(Method::DELETE)
            .uri(format!("/api/v2/print/sources/{source_id}"))
            .header(header::COOKIE, cookie)
            .body(Body::empty())?,
    )
    .await?;
    assert_eq!(revoked.status, StatusCode::NO_CONTENT);
    assert_eq!(
        run(&config, &unavailable_runtime, &input).await.err(),
        Some(natsume_print_domjudge::Error::Rejected(401))
    );
    Ok(())
}
