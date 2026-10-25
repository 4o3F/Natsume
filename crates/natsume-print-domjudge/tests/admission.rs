#![cfg(target_os = "linux")]

use std::{
    error::Error as StdError,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use natsume_print_domjudge::{Error, Input, Runtime, config::Config, run, wire::State};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    task::JoinHandle,
    time::Instant,
};
use tokio_rustls::TlsAcceptor;
use tokio_util::sync::CancellationToken;

type TestResult = Result<(), Box<dyn StdError>>;
const ID: &str = "01900000-0000-7000-8000-000000000001";
const ACCOUNT_ID: &str = "01900000-0000-7000-8000-000000000002";
const PDF: &[u8] = b"%PDF-1.7\nmock document\n";

fn fact(state: &str) -> Value {
    json!({
        "request_id": ID, "account_id": ACCOUNT_ID,
        "account": "Team Exact", "team_name": "测试队伍", "seat_label": null,
        "state": state,
        "document": if state == "ready" { json!({"sha256": hex::encode(Sha256::digest(PDF)), "bytes": PDF.len(), "pages": 2}) } else { Value::Null },
        "admission": if state == "ready" { json!({"ready_sequence": 3, "ahead_tasks": 1, "ahead_pages": 5, "in_progress_tasks": 0}) } else { Value::Null },
        "generation_failure": if state == "generation_failed" { json!("compile_failed") } else { Value::Null },
    })
}

#[derive(Clone)]
struct Request {
    head: String,
    body: Vec<u8>,
}

enum Reply {
    Json(Value),
    Lost,
    Status(u16, String),
    Oversized,
    Stall,
}

struct Server {
    origin: String,
    ca: String,
    seen: Arc<Mutex<Vec<Request>>>,
    task: JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn server(
    replies: Vec<Reply>,
    marker: Option<PathBuf>,
    dns: &str,
) -> Result<Server, Box<dyn StdError>> {
    let cert = rcgen::generate_simple_self_signed(vec![dns.to_owned()])?;
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_protocol_versions(&[&rustls::version::TLS13])?
    .with_no_client_auth()
    .with_single_cert(
        vec![cert.cert.der().clone()],
        rustls::pki_types::PrivatePkcs8KeyDer::from(cert.signing_key.serialize_der()).into(),
    )?;
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let origin = format!("https://{}", listener.local_addr()?);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let requests = seen.clone();
    let task = tokio::spawn(async move {
        for reply in replies {
            let Ok((socket, _)) = listener.accept().await else {
                return;
            };
            let Ok(mut tls) = acceptor.accept(socket).await else {
                return;
            };
            let mut bytes = Vec::new();
            let header_end = loop {
                let mut buffer = [0; 4096];
                let Ok(n) = tls.read(&mut buffer).await else {
                    return;
                };
                if n == 0 {
                    return;
                }
                bytes.extend_from_slice(&buffer[..n]);
                if let Some(index) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    break index + 4;
                }
                assert!(bytes.len() < 16 * 1024);
            };
            let head = String::from_utf8_lossy(&bytes[..header_end]).to_string();
            let length: usize = head
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(str::trim)
                        .and_then(|n| n.parse().ok())
                })
                .unwrap_or(0);
            while bytes.len() - header_end < length {
                let mut buffer = [0; 4096];
                let Ok(n) = tls.read(&mut buffer).await else {
                    return;
                };
                if n == 0 {
                    return;
                }
                bytes.extend_from_slice(&buffer[..n]);
                assert!(bytes.len() < 512 * 1024);
            }
            if head.starts_with("POST /api/v2/print/requests HTTP")
                && let Some(marker) = &marker
            {
                assert!(!marker.exists(), "compiler ran before registration");
            }
            match requests.lock() {
                Ok(mut requests) => requests.push(Request {
                    head,
                    body: bytes[header_end..].to_vec(),
                }),
                Err(_) => return,
            }
            let no_length = matches!(&reply, Reply::Oversized);
            let (status, extra, body) = match reply {
                Reply::Json(value) => (200, String::new(), value.to_string()),
                Reply::Lost => continue,
                Reply::Status(status, extra) => (status, extra, String::new()),
                Reply::Oversized => (200, String::new(), " ".repeat(65537)),
                Reply::Stall => std::future::pending().await,
            };
            let length = if no_length {
                String::new()
            } else {
                format!("Content-Length: {}\r\n", body.len())
            };
            let response = format!(
                "HTTP/1.1 {status} response\r\nContent-Type: application/json\r\n{length}Connection: close\r\n{extra}\r\n{body}"
            );
            let _ = tls.write_all(response.as_bytes()).await;
            let _ = tls.shutdown().await;
        }
    });
    Ok(Server {
        origin,
        ca: cert.cert.pem(),
        seen,
        task,
    })
}

struct Fixture {
    root: TempDir,
    config: Config,
    runtime: Runtime,
    input: Input,
    marker: PathBuf,
}
impl Fixture {
    fn new(server: &Server, script: &str) -> Result<Self, Box<dyn StdError>> {
        let root = tempfile::tempdir()?;
        let token = root.path().join("token");
        std::fs::write(&token, format!("nps1.{ID}.{}\n", "ab".repeat(32)))?;
        std::fs::set_permissions(&token, std::fs::Permissions::from_mode(0o600))?;
        let ca = root.path().join("ca.crt");
        std::fs::write(&ca, &server.ca)?;
        let config_path = root.path().join("config.toml");
        std::fs::write(
            &config_path,
            format!(
                "server_url = {:?}\ntoken_file = {:?}\nca_certificate = {:?}\n",
                server.origin,
                token.to_str().ok_or("token path")?,
                ca.to_str().ok_or("CA path")?
            ),
        )?;
        let config = Config::load(&config_path)?;
        let source = root.path().join("input");
        std::fs::write(&source, "#panic(\"injection\")\n中文源码")?;
        let marker = root.path().join("compiled");
        let compiler = root.path().join("compiler");
        let marker_literal = serde_json::to_string(marker.to_str().ok_or("marker path")?)?;
        std::fs::write(
            &compiler,
            format!(
                "#!/usr/bin/python3\nfrom pathlib import Path\nPath({marker_literal}).write_text('ran')\n{script}\n"
            ),
        )?;
        std::fs::set_permissions(&compiler, std::fs::Permissions::from_mode(0o700))?;
        let template = root.path().join("template.typ");
        std::fs::write(&template, "fixed template")?;
        let runtime = Runtime {
            compiler,
            template,
            fonts: root.path().to_owned(),
        };
        let input = Input {
            file: source,
            account: "Team Exact".into(),
            original: "原始.cpp".into(),
            request_id: ID.into(),
        };
        Ok(Self {
            root,
            config,
            runtime,
            input,
            marker,
        })
    }
    async fn run(&self) -> natsume_print_domjudge::Result<natsume_print_domjudge::wire::Fact> {
        run(
            &self.config,
            &self.runtime,
            &self.input,
            Instant::now(),
            &CancellationToken::new(),
        )
        .await
    }
}
const COMPILE: &str = "Path('document.pdf').write_bytes(b'%PDF-1.7\\nmock document\\n')";

#[tokio::test]
async fn executable_reports_ready_result_through_tracing_stdout() -> TestResult {
    let peer = server(vec![Reply::Json(fact("ready"))], None, "127.0.0.1").await?;
    let fixture = Fixture::new(&peer, COMPILE)?;
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_natsume-print-domjudge"))
        .arg("--config")
        .arg(fixture.root.path().join("config.toml"))
        .arg("--file")
        .arg(&fixture.input.file)
        .arg("--account")
        .arg(&fixture.input.account)
        .arg("--original")
        .arg(&fixture.input.original)
        .arg("--request-id")
        .arg(ID)
        .output()
        .await?;

    let stdout = String::from_utf8(output.stdout)?;
    assert!(output.status.success(), "{stdout}");
    assert!(stdout.contains("INFO"), "{stdout}");
    assert!(stdout.contains("已入队，尚未打印完成"), "{stdout}");
    assert!(stdout.contains(ID), "{stdout}");
    assert!(output.stderr.is_empty());
    assert!(!stdout.contains('\u{1b}'));
    assert!(!fixture.marker.exists());
    Ok(())
}

#[tokio::test]
async fn registration_precedes_generation_and_pdf_upload_uses_exact_identity() -> TestResult {
    let peer = server(
        vec![Reply::Json(fact("preparing")), Reply::Json(fact("ready"))],
        None,
        "127.0.0.1",
    )
    .await?;
    let fixture = Fixture::new(&peer, COMPILE)?;
    // Recreate with the fixture marker to assert the ordering at the HTTPS boundary.
    let actual = server(
        vec![Reply::Json(fact("preparing")), Reply::Json(fact("ready"))],
        Some(fixture.marker.clone()),
        "127.0.0.1",
    )
    .await?;
    let mut config = fixture.config;
    config.server_url.clone_from(&actual.origin);
    std::fs::write(
        config.ca_certificate.as_ref().ok_or("missing CA")?,
        &actual.ca,
    )?;
    let output = run(
        &config,
        &fixture.runtime,
        &fixture.input,
        Instant::now(),
        &CancellationToken::new(),
    )
    .await?;
    assert_eq!(output.state, State::Ready);
    let seen = actual.seen.lock().map_err(|_| "poisoned")?;
    assert_eq!(seen.len(), 2);
    let registration: Value = serde_json::from_slice(&seen[0].body)?;
    assert!(registration.get("protocol_version").is_none());
    assert_eq!(registration["account"], "Team Exact");
    assert_eq!(registration["original_filename"], "原始.cpp");
    assert_eq!(
        registration["source_sha256"],
        hex::encode(Sha256::digest(std::fs::read(&fixture.input.file)?))
    );
    assert!(
        seen[0]
            .head
            .contains(&format!("authorization: Bearer nps1.{ID}."))
    );
    assert!(
        seen[1]
            .head
            .starts_with(&format!("PUT /api/v2/print/requests/{ID}/document HTTP"))
    );
    assert!(seen[1].head.contains("content-type: application/pdf"));
    assert_eq!(seen[1].body, PDF);
    let message = natsume_print_domjudge::success_message(&output);
    assert!(message.contains("已入队，尚未打印完成"));
    assert!(message.contains("测试队伍"));
    assert!(message.contains("未分配"));
    Ok(())
}

#[tokio::test]
async fn lost_registration_response_is_verified_before_generation() -> TestResult {
    let peer = server(
        vec![
            Reply::Lost,
            Reply::Json(fact("preparing")),
            Reply::Json(fact("ready")),
        ],
        None,
        "127.0.0.1",
    )
    .await?;
    let fixture = Fixture::new(&peer, COMPILE)?;
    assert_eq!(fixture.run().await?.state, State::Ready);
    let seen = peer.seen.lock().map_err(|_| "poisoned")?;
    assert_eq!(seen.len(), 3);
    assert!(seen[1].head.starts_with("POST /api/v2/print/requests HTTP"));
    assert_eq!(seen[0].body, seen[1].body);
    Ok(())
}

#[tokio::test]
async fn lost_registration_conflict_cannot_accept_an_old_ready_request() -> TestResult {
    let peer = server(
        vec![Reply::Lost, Reply::Status(409, String::new())],
        None,
        "127.0.0.1",
    )
    .await?;
    let fixture = Fixture::new(&peer, COMPILE)?;
    assert_eq!(fixture.run().await.err(), Some(Error::Rejected(409)));
    assert!(!fixture.marker.exists());
    let seen = peer.seen.lock().map_err(|_| "poisoned")?;
    assert_eq!(seen.len(), 2);
    assert!(
        seen.iter()
            .all(|request| request.head.starts_with("POST /api/v2/print/requests HTTP"))
    );
    assert_eq!(seen[0].body, seen[1].body);
    Ok(())
}

#[tokio::test]
async fn lost_upload_response_keeps_the_same_id_and_requires_ready() -> TestResult {
    for state in ["ready", "preparing"] {
        let peer = server(
            vec![
                Reply::Json(fact("preparing")),
                Reply::Lost,
                Reply::Json(fact(state)),
            ],
            None,
            "127.0.0.1",
        )
        .await?;
        let fixture = Fixture::new(&peer, COMPILE)?;
        let result = fixture.run().await;
        if state == "ready" {
            assert_eq!(result?.request_id, ID);
        } else {
            assert_eq!(result.err(), Some(Error::Ambiguous));
        }
        let seen = peer.seen.lock().map_err(|_| "poisoned")?;
        assert_eq!(
            seen.iter().filter(|r| r.head.starts_with("POST ")).count(),
            1
        );
        assert!(
            seen[2]
                .head
                .starts_with(&format!("GET /api/v2/print/requests/{ID} HTTP"))
        );
    }
    Ok(())
}

#[tokio::test]
async fn generation_failure_is_reported_without_compiler_output() -> TestResult {
    let peer = server(
        vec![
            Reply::Json(fact("preparing")),
            Reply::Json(fact("generation_failed")),
        ],
        None,
        "127.0.0.1",
    )
    .await?;
    let fixture = Fixture::new(
        &peer,
        "import sys\nsys.stderr.write('SECRET source compiler diagnostic')\nsys.exit(1)",
    )?;
    let result = fixture.run().await;
    assert_eq!(
        result.err(),
        Some(Error::Generation(
            natsume_print_domjudge::wire::FailureCode::CompileFailed
        ))
    );
    let seen = peer.seen.lock().map_err(|_| "poisoned")?;
    assert_eq!(seen.len(), 2);
    assert_eq!(
        serde_json::from_slice::<Value>(&seen[1].body)?,
        json!({"code":"compile_failed"})
    );
    Ok(())
}

#[tokio::test]
async fn already_ready_retry_preserves_active_counts_without_running_compiler() -> TestResult {
    let mut ready = fact("ready");
    ready["admission"]["in_progress_tasks"] = json!(2);
    let peer = server(vec![Reply::Json(ready)], None, "127.0.0.1").await?;
    let fixture = Fixture::new(&peer, "raise Exception('must not run')")?;
    let result = fixture.run().await?;
    assert_eq!(result.state, State::Ready);
    assert_eq!(
        result
            .admission
            .ok_or("missing admission")?
            .in_progress_tasks,
        2
    );
    assert!(!fixture.marker.exists());
    Ok(())
}

#[tokio::test]
async fn rejected_registration_never_runs_compiler() -> TestResult {
    let peer = server(vec![Reply::Status(401, String::new())], None, "127.0.0.1").await?;
    let fixture = Fixture::new(&peer, COMPILE)?;
    assert_eq!(fixture.run().await.err(), Some(Error::Rejected(401)));
    assert!(!fixture.marker.exists());
    Ok(())
}

#[tokio::test]
async fn redirect_cannot_forward_token_to_another_server() -> TestResult {
    let other = TcpListener::bind("127.0.0.1:0").await?;
    let peer = server(
        vec![
            Reply::Status(
                307,
                format!("Location: https://{}/stolen\r\n", other.local_addr()?),
            ),
            Reply::Status(404, String::new()),
        ],
        None,
        "127.0.0.1",
    )
    .await?;
    let fixture = Fixture::new(&peer, COMPILE)?;
    assert_eq!(fixture.run().await.err(), Some(Error::Ambiguous));
    assert!(!fixture.marker.exists());
    assert!(
        tokio::time::timeout(Duration::from_millis(50), other.accept())
            .await
            .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn strict_tls_rejects_wrong_ca_and_hostname() -> TestResult {
    for wrong_ca in [false, true] {
        let peer = server(
            vec![Reply::Json(fact("preparing"))],
            None,
            if wrong_ca {
                "127.0.0.1"
            } else {
                "other.invalid"
            },
        )
        .await?;
        let fixture = Fixture::new(&peer, COMPILE)?;
        if wrong_ca {
            let unrelated = rcgen::generate_simple_self_signed(vec!["127.0.0.1".into()])?;
            std::fs::write(
                fixture.config.ca_certificate.as_ref().ok_or("missing CA")?,
                unrelated.cert.pem(),
            )?;
        }
        assert_eq!(fixture.run().await.err(), Some(Error::Ambiguous));
        assert!(!fixture.marker.exists());
        assert!(peer.seen.lock().map_err(|_| "poisoned")?.is_empty());
    }
    Ok(())
}

#[tokio::test]
async fn malformed_or_incomplete_ready_never_reports_success() -> TestResult {
    let mut wrong_id = fact("ready");
    wrong_id["request_id"] = json!(ACCOUNT_ID);
    let mut no_admission = fact("ready");
    no_admission["admission"] = Value::Null;
    let mut missing_null = fact("ready");
    missing_null
        .as_object_mut()
        .ok_or("object")?
        .remove("seat_label");
    let mut wrong_hash = fact("ready");
    wrong_hash["document"]["sha256"] = json!("A".repeat(64));
    let mut extra_version = fact("ready");
    extra_version["protocol_version"] = json!(1);
    for invalid in [
        wrong_id,
        no_admission,
        missing_null,
        wrong_hash,
        extra_version,
    ] {
        let peer = server(
            vec![Reply::Json(invalid), Reply::Status(404, String::new())],
            None,
            "127.0.0.1",
        )
        .await?;
        let fixture = Fixture::new(&peer, COMPILE)?;
        assert_eq!(fixture.run().await.err(), Some(Error::Ambiguous));
        assert!(!fixture.marker.exists());
    }
    Ok(())
}

#[tokio::test]
async fn oversized_response_is_bounded_and_does_not_start_generation() -> TestResult {
    let peer = server(
        vec![Reply::Oversized, Reply::Status(404, String::new())],
        None,
        "127.0.0.1",
    )
    .await?;
    let fixture = Fixture::new(&peer, COMPILE)?;
    assert_eq!(fixture.run().await.err(), Some(Error::Ambiguous));
    assert!(!fixture.marker.exists());
    Ok(())
}

#[tokio::test]
async fn unsafe_sources_and_display_paths_are_rejected_before_network() -> TestResult {
    let peer = server(vec![], None, "127.0.0.1").await?;
    let mut fixture = Fixture::new(&peer, COMPILE)?;
    let original_file = fixture.input.file.clone();
    let link = fixture.root.path().join("link");
    std::os::unix::fs::symlink(&original_file, &link)?;
    fixture.input.file = link;
    assert_eq!(fixture.run().await.err(), Some(Error::UnsafeFile));
    fixture.input.file = fixture.root.path().to_owned();
    assert_eq!(fixture.run().await.err(), Some(Error::UnsafeFile));
    fixture.input.file = "/dev/null".into();
    assert_eq!(fixture.run().await.err(), Some(Error::UnsafeFile));
    let fifo = fixture.root.path().join("fifo");
    rustix::fs::mkfifoat(
        rustix::fs::CWD,
        &fifo,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
    )?;
    fixture.input.file = fifo;
    assert_eq!(fixture.run().await.err(), Some(Error::UnsafeFile));
    fixture.input.file = original_file;
    std::fs::write(&fixture.input.file, [0xff])?;
    assert_eq!(fixture.run().await.err(), Some(Error::InvalidInput));
    std::fs::write(&fixture.input.file, vec![b'x'; 256 * 1024 + 1])?;
    assert_eq!(fixture.run().await.err(), Some(Error::UnsafeFile));
    std::fs::write(&fixture.input.file, "source")?;
    for original in ["../../x", "file\\path", "line\nfeed", "nul\0"] {
        fixture.input.original = original.into();
        assert_eq!(fixture.run().await.err(), Some(Error::InvalidInput));
    }
    Ok(())
}

#[test]
fn deployment_configuration_and_token_permissions_fail_closed() -> TestResult {
    let root = tempfile::tempdir()?;
    for origin in [
        "http://localhost",
        "https://user@localhost",
        "https://@localhost",
        "https://localhost/path",
        "https://localhost/.",
        "https://localhost?x",
        "https://localhost#x",
        "https://localhost\\evil",
    ] {
        let path = root.path().join("config");
        std::fs::write(
            &path,
            format!("server_url={origin:?}\ntoken_file='/tmp/token'\n"),
        )?;
        assert!(Config::load(&path).is_err(), "{origin}");
    }
    Ok(())
}

#[tokio::test]
async fn credential_world_readability_and_symlink_are_rejected() -> TestResult {
    let peer = server(vec![], None, "127.0.0.1").await?;
    let fixture = Fixture::new(&peer, COMPILE)?;
    std::fs::set_permissions(
        &fixture.config.token_file,
        std::fs::Permissions::from_mode(0o644),
    )?;
    assert_eq!(fixture.run().await.err(), Some(Error::Configuration));
    let original = fixture.root.path().join("actual-token");
    std::fs::rename(&fixture.config.token_file, &original)?;
    std::os::unix::fs::symlink(original, &fixture.config.token_file)?;
    assert_eq!(fixture.run().await.err(), Some(Error::UnsafeFile));
    Ok(())
}

#[tokio::test]
async fn startup_time_is_charged_to_whole_wrapper_deadline() -> TestResult {
    let peer = server(vec![], None, "127.0.0.1").await?;
    let fixture = Fixture::new(&peer, COMPILE)?;
    let result = run(
        &fixture.config,
        &fixture.runtime,
        &fixture.input,
        Instant::now() - Duration::from_secs(5),
        &CancellationToken::new(),
    )
    .await;
    assert_eq!(result.err(), Some(Error::Deadline));
    assert!(!fixture.marker.exists());
    Ok(())
}

#[tokio::test]
async fn upload_ready_requires_exact_document_and_registration_snapshots() -> TestResult {
    let mut wrong_hash = fact("ready");
    wrong_hash["document"]["sha256"] = json!("ab".repeat(32));
    let mut wrong_team = fact("ready");
    wrong_team["team_name"] = json!("different snapshot");
    for invalid in [wrong_hash, wrong_team] {
        let peer = server(
            vec![Reply::Json(fact("preparing")), Reply::Json(invalid)],
            None,
            "127.0.0.1",
        )
        .await?;
        let fixture = Fixture::new(&peer, COMPILE)?;
        assert_eq!(fixture.run().await.err(), Some(Error::Ambiguous));
    }
    Ok(())
}

#[tokio::test]
async fn too_large_pdf_is_reported_without_upload() -> TestResult {
    let mut failed = fact("generation_failed");
    failed["generation_failure"] = json!("document_too_large");
    let peer = server(
        vec![Reply::Json(fact("preparing")), Reply::Json(failed)],
        None,
        "127.0.0.1",
    )
    .await?;
    let fixture = Fixture::new(
        &peer,
        "with open('document.pdf', 'wb') as output:\n output.truncate(8*1024*1024+1)",
    )?;
    assert_eq!(
        fixture.run().await.err(),
        Some(Error::Generation(
            natsume_print_domjudge::wire::FailureCode::DocumentTooLarge
        ))
    );
    let requests = peer.seen.lock().map_err(|_| "poisoned")?;
    assert_eq!(requests.len(), 2);
    assert!(requests[1].head.contains("/generation-failure HTTP"));
    assert_eq!(
        serde_json::from_slice::<Value>(&requests[1].body)?["code"],
        "document_too_large"
    );
    Ok(())
}

#[tokio::test]
async fn lost_generation_failure_report_does_not_create_an_outbox_or_retry() -> TestResult {
    let peer = server(
        vec![Reply::Json(fact("preparing")), Reply::Lost],
        None,
        "127.0.0.1",
    )
    .await?;
    let fixture = Fixture::new(&peer, "raise Exception('bounded private error')")?;
    assert_eq!(
        fixture.run().await.err(),
        Some(Error::Generation(
            natsume_print_domjudge::wire::FailureCode::CompileFailed
        ))
    );
    assert_eq!(peer.seen.lock().map_err(|_| "poisoned")?.len(), 2);
    Ok(())
}

#[tokio::test]
async fn network_and_verification_share_the_same_total_deadline() -> TestResult {
    let peer = server(vec![Reply::Stall], None, "127.0.0.1").await?;
    let fixture = Fixture::new(&peer, COMPILE)?;
    let marker = fixture.marker.clone();
    let handle = tokio::spawn(async move { fixture.run().await });
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if peer.seen.lock().is_ok_and(|requests| !requests.is_empty()) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await?;
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(5)).await;
    assert_eq!(handle.await?.err(), Some(Error::Ambiguous));
    assert!(!marker.exists());
    assert_eq!(peer.seen.lock().map_err(|_| "poisoned")?.len(), 1);
    Ok(())
}
