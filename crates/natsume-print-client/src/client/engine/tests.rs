use std::{
    collections::VecDeque,
    fmt::Debug,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::Duration,
};

use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::TcpListener,
};
use tokio_rustls::TlsAcceptor;
use tokio_util::task::AbortOnDropHandle;

use super::super::{
    config::Config,
    wire::{AttemptState, Pairing},
};
use super::*;

mod cases;
mod pairing_cases;

const PDF: &[u8] = b"%PDF-1.7 immutable test document";
const CLIENT: &str = "01900000-0000-7000-8000-000000000001";
const PRINTER: &str = "01900000-0000-7000-8000-000000000002";
const ATTEMPT: &str = "01900000-0000-7000-8000-000000000003";

fn must<T, E: Debug>(result: std::result::Result<T, E>) -> T {
    result.unwrap_or_else(|error| panic!("print client fixture failed: {error:?}"))
}

fn binding() -> Binding {
    Binding {
        printer_id: PRINTER.to_owned(),
        client_id: CLIENT.to_owned(),
        confirmed: true,
    }
}

fn attempt() -> Attempt {
    Attempt {
        attempt_id: ATTEMPT.to_owned(),
        ready_sequence: 1,
        printer_id: PRINTER.to_owned(),
        client_id: CLIENT.to_owned(),
        attempt_number: 1,
        queue_name: "Test queue".to_owned(),
        label: format!("Natsume/{ATTEMPT}"),
        state: AttemptState::Preparing,
        claimed_at_unix_ms: 1,
        prepare_expires_at_unix_ms: 120_001,
        lease_expires_at_unix_ms: 60_001,
        last_contact_at_unix_ms: 1,
        last_observed_at_unix_ms: None,
        job_id: None,
        finished_at_unix_ms: None,
        document_sha256: hex::encode(Sha256::digest(PDF)),
        document_bytes: PDF.len() as u64,
        document_pages: 1,
    }
}

fn registered() -> Attempt {
    Attempt {
        state: AttemptState::Observing,
        job_id: Some(42),
        ..attempt()
    }
}

fn terminal(state: AttemptState) -> Attempt {
    Attempt {
        state,
        finished_at_unix_ms: Some(100),
        ..registered()
    }
}

enum Reply {
    Json(Value),
    AfterDurableJob(Value, std::path::PathBuf),
    Pdf(Vec<u8>),
    Lost,
    Status(u16),
    Redirect(String),
    Oversized,
}

#[derive(Debug)]
struct Seen {
    method: String,
    path: String,
    body: Value,
    authenticated: bool,
}

struct Https {
    origin: String,
    ca: String,
    seen: Arc<Mutex<Vec<Seen>>>,
    _task: AbortOnDropHandle<()>,
}

fn tls_identity() -> (TlsAcceptor, String) {
    let cert = must(rcgen::generate_simple_self_signed(vec![
        "127.0.0.1".to_owned(),
    ]));
    let tls = must(
        must(
            rustls::ServerConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_protocol_versions(&[&rustls::version::TLS13]),
        )
        .with_no_client_auth()
        .with_single_cert(
            vec![cert.cert.der().clone()],
            rustls::pki_types::PrivatePkcs8KeyDer::from(cert.signing_key.serialize_der()).into(),
        ),
    );
    (TlsAcceptor::from(Arc::new(tls)), cert.cert.pem())
}

async fn https(replies: Vec<Reply>) -> Https {
    let (acceptor, ca) = tls_identity();
    let listener = must(TcpListener::bind("127.0.0.1:0").await);
    let origin = format!("https://{}", must(listener.local_addr()));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let requests = Arc::clone(&seen);
    let task = tokio::spawn(async move {
        for reply in replies {
            let Ok((socket, _)) = listener.accept().await else {
                return;
            };
            let Ok(mut socket) = acceptor.accept(socket).await else {
                return;
            };
            let mut bytes = Vec::new();
            let end = loop {
                let mut buffer = [0; 4096];
                let count = must(socket.read(&mut buffer).await);
                if count == 0 {
                    return;
                }
                bytes.extend_from_slice(&buffer[..count]);
                if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                    break end + 4;
                }
                assert!(bytes.len() < 32 * 1024);
            };
            let head = String::from_utf8_lossy(&bytes[..end]).to_string();
            let length = head
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .and_then(|value| value.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);
            while bytes.len() - end < length {
                let mut buffer = [0; 4096];
                let count = must(socket.read(&mut buffer).await);
                if count == 0 {
                    return;
                }
                bytes.extend_from_slice(&buffer[..count]);
                assert!(bytes.len() < 1024 * 1024);
            }
            let mut line = head.split_ascii_whitespace();
            let method = line.next().unwrap_or_default().to_owned();
            let path = line.next().unwrap_or_default().to_owned();
            let authenticated = head.lines().any(|line| {
                line.to_ascii_lowercase()
                    .starts_with("authorization: bearer npc1.")
            });
            let body = if length == 0 {
                Value::Null
            } else {
                must(serde_json::from_slice(&bytes[end..]))
            };
            must(requests.lock()).push(Seen {
                method,
                path,
                body,
                authenticated,
            });
            let (status, extra, body) = match reply {
                Reply::Json(value) => (200, String::new(), must(serde_json::to_vec(&value))),
                Reply::AfterDurableJob(value, path) => {
                    let journal = must(std::fs::read_to_string(path));
                    let record: Value = must(serde_json::from_str(
                        journal.lines().last().unwrap_or_default(),
                    ));
                    assert_eq!(record["state"]["work"]["phase"], "observing");
                    assert_eq!(record["state"]["work"]["job"]["job_id"], 42);
                    (200, String::new(), must(serde_json::to_vec(&value)))
                }
                Reply::Pdf(bytes) => (200, String::new(), bytes),
                Reply::Lost => continue,
                Reply::Status(status) => (status, String::new(), Vec::new()),
                Reply::Redirect(target) => (307, format!("Location: {target}\r\n"), Vec::new()),
                Reply::Oversized => (200, String::new(), vec![b' '; 512 * 1024 + 1]),
            };
            let head = format!(
                "HTTP/1.1 {status} response\r\nContent-Length: {}\r\nConnection: close\r\n{extra}\r\n",
                body.len()
            );
            if socket.write_all(head.as_bytes()).await.is_err() {
                continue;
            }
            let _ = socket.write_all(&body).await;
            let _ = socket.shutdown().await;
        }
    });
    Https {
        origin,
        ca,
        seen,
        _task: AbortOnDropHandle::new(task),
    }
}

fn config(directory: &tempfile::TempDir, server: &Https) -> Config {
    let ca = directory.path().join("ca.pem");
    must(std::fs::write(&ca, &server.ca));
    Config {
        server_url: server.origin.clone(),
        queue_name: "Test queue".to_owned(),
        state_directory: directory.path().join("state"),
        ca_certificate: Some(ca),
    }
}

fn api(config: &Config) -> Api {
    let mut api = must(Api::new(config));
    api.authenticate(format!("npc1.{CLIENT}.{}", "a".repeat(64)));
    api
}

#[derive(Default)]
struct FakePrinter {
    submissions: Arc<AtomicUsize>,
    pages: Arc<AtomicBool>,
    observations: Mutex<VecDeque<JobObservation>>,
    busy_once: AtomicBool,
    release: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
}

impl Printer for FakePrinter {
    fn ready(&self, _: &str) -> crate::Result<bool> {
        Ok(true)
    }
    fn observe(&self, _: &str, job: u32, label: &str) -> crate::Result<JobObservation> {
        assert_eq!(job, 42);
        assert_eq!(label, format!("Natsume/{ATTEMPT}"));
        Ok(must(self.observations.lock())
            .pop_front()
            .unwrap_or(JobObservation::Pending))
    }
    fn submit(
        &self,
        _: &str,
        document: &Document,
        started: impl FnOnce(u32) -> crate::Result<()>,
    ) -> crate::Result<u32> {
        if self.busy_once.swap(false, Ordering::SeqCst) {
            return crate::UnavailableSnafu.fail();
        }
        self.submissions.fetch_add(1, Ordering::SeqCst);
        assert_eq!(document.bytes(), PDF);
        started(42)?;
        if let Some(release) = must(self.release.lock()).take() {
            must(release.recv_timeout(Duration::from_secs(5)));
        }
        self.pages.store(true, Ordering::SeqCst);
        Ok(42)
    }
}

fn engine(config: &Config, work: Work, paused: bool, printer: FakePrinter) -> Engine<FakePrinter> {
    let mut store = must(Store::open(config));
    let mut state = store.state.clone();
    state.binding = Some(binding());
    state.work = work;
    state.paused = paused;
    must(store.save(state));
    must(Engine::new(store, api(config), printer))
}

async fn submission_finished(engine: &mut Engine<FakePrinter>) -> Result<()> {
    must(
        tokio::time::timeout(Duration::from_secs(5), async {
            while !engine
                .submitting
                .as_ref()
                .is_some_and(JoinHandle::is_finished)
            {
                tokio::task::yield_now().await;
            }
        })
        .await,
    );
    engine.finish_submission().await
}

#[tokio::test]
async fn pause_is_persistent_and_does_not_cancel_existing_work() {
    let server = https(vec![
        Reply::Json(json!(registered())),
        Reply::Json(json!(registered())),
    ])
    .await;
    let directory = must(tempfile::tempdir());
    let config = config(&directory, &server);
    let mut client = engine(
        &config,
        Work::Observing {
            attempt: registered(),
            job: attempt().job(42),
        },
        true,
        FakePrinter::default(),
    );
    must(client.step(false).await);
    assert!(must(locked(&client.store)).state.paused);
    drop(client);
    let store = must(Store::open(&config));
    assert!(store.state.paused);
    assert!(matches!(store.state.work, Work::Observing { .. }));
    assert_eq!(must(server.seen.lock()).len(), 2);
}

#[tokio::test]
async fn corrupt_or_missing_records_do_not_reset_to_idle() {
    let server = https(Vec::new()).await;
    let directory = must(tempfile::tempdir());
    let config = config(&directory, &server);
    let store = must(Store::open(&config));
    assert!(store.state.paused);
    assert!(
        Store::open(&config).is_err(),
        "single-instance lock must be exclusive"
    );
    drop(store);
    let path = config.state_directory.join("state.jsonl");
    let original = must(std::fs::read(&path));
    let mut envelope: Value = must(serde_json::from_slice(&original));
    envelope["state"]["paused"] = json!(false);
    must(std::fs::write(&path, must(serde_json::to_vec(&envelope))));
    assert!(Store::open(&config).is_err());
    must(std::fs::remove_file(path));
    assert!(Store::open(&config).is_err());
}

#[tokio::test]
async fn interrupted_start_without_a_job_id_never_resubmits() {
    let server = https(Vec::new()).await;
    let directory = must(tempfile::tempdir());
    let config = config(&directory, &server);
    let mut store = must(Store::open(&config));
    let mut state = store.state.clone();
    state.binding = Some(binding());
    state.work = Work::Starting { attempt: attempt() };
    must(store.save(state));
    drop(store);
    assert!(
        Engine::new(
            must(Store::open(&config)),
            api(&config),
            FakePrinter::default()
        )
        .is_err()
    );
    assert!(must(server.seen.lock()).is_empty());
}

#[tokio::test]
async fn lost_claim_reuses_the_persisted_request_id() {
    let server = https(vec![Reply::Lost, Reply::Json(Value::Null)]).await;
    let directory = must(tempfile::tempdir());
    let config = config(&directory, &server);
    let id = Uuid::now_v7().to_string();
    let mut client = engine(
        &config,
        Work::Claiming {
            request_id: id.clone(),
        },
        true,
        FakePrinter::default(),
    );
    assert!(matches!(
        client.step(false).await,
        Err(ClientError::Transport { .. })
    ));
    drop(client);
    let mut client = must(Engine::new(
        must(Store::open(&config)),
        api(&config),
        FakePrinter::default(),
    ));
    must(client.step(false).await);
    assert_eq!(must(locked(&client.store)).state.work, Work::Idle);
    let seen = must(server.seen.lock());
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].body, seen[1].body);
    assert_eq!(seen[0].body["request_id"], id);
    assert!(seen.iter().all(|request| request.authenticated));
}

#[tokio::test]
async fn first_page_requires_durable_job_identity_and_server_confirmation() {
    let directory = must(tempfile::tempdir());
    let server = https(vec![
        Reply::Json(json!(attempt())),
        Reply::Pdf(PDF.to_vec()),
        Reply::AfterDurableJob(
            json!(registered()),
            directory.path().join("state/state.jsonl"),
        ),
    ])
    .await;
    let config = config(&directory, &server);
    let printer = FakePrinter::default();
    let pages = Arc::clone(&printer.pages);
    let mut client = engine(
        &config,
        Work::Preparing { attempt: attempt() },
        false,
        printer,
    );
    must(client.step(false).await);
    must(submission_finished(&mut client).await);
    assert!(pages.load(Ordering::SeqCst));
    assert!(
        matches!(must(locked(&client.store)).state.work, Work::Observing { ref job, .. } if job.job_id == 42)
    );
    drop(client);
    assert!(
        matches!(must(Store::open(&config)).state.work, Work::Observing { ref job, .. } if job.job_id == 42)
    );
    let seen = must(server.seen.lock());
    assert_eq!(seen[2].method, "PUT");
    assert_eq!(seen[2].body["job_id"], 42);
}

#[tokio::test]
async fn lost_job_confirmation_does_not_send_pages_or_submit_again() {
    let server = https(vec![
        Reply::Json(json!(attempt())),
        Reply::Pdf(PDF.to_vec()),
        Reply::Lost,
    ])
    .await;
    let directory = must(tempfile::tempdir());
    let config = config(&directory, &server);
    let printer = FakePrinter::default();
    let submissions = Arc::clone(&printer.submissions);
    let pages = Arc::clone(&printer.pages);
    let mut client = engine(
        &config,
        Work::Preparing { attempt: attempt() },
        false,
        printer,
    );
    must(client.step(false).await);
    must(submission_finished(&mut client).await);
    assert!(!pages.load(Ordering::SeqCst));
    assert_eq!(submissions.load(Ordering::SeqCst), 1);
    assert!(matches!(
        must(locked(&client.store)).state.work,
        Work::Reporting {
            report: Report::Unknown,
            ..
        }
    ));
}

#[tokio::test]
async fn restart_only_observes_a_saved_job_and_never_submits_it_again() {
    let server = https(vec![
        Reply::Json(json!(registered())),
        Reply::Json(json!(terminal(AttemptState::Printed))),
    ])
    .await;
    let directory = must(tempfile::tempdir());
    let config = config(&directory, &server);
    let printer = FakePrinter::default();
    must(printer.observations.lock()).push_back(JobObservation::SpoolerReportedPrinted);
    let submissions = Arc::clone(&printer.submissions);
    let mut client = engine(
        &config,
        Work::Observing {
            attempt: registered(),
            job: attempt().job(42),
        },
        false,
        printer,
    );
    must(client.step(false).await);
    assert!(matches!(
        must(locked(&client.store)).state.work,
        Work::Reporting {
            report: Report::Printed(_),
            ..
        }
    ));
    drop(client);
    let mut client = must(Engine::new(
        must(Store::open(&config)),
        api(&config),
        FakePrinter::default(),
    ));
    must(client.step(false).await);
    assert_eq!(must(locked(&client.store)).state.work, Work::Idle);
    assert_eq!(submissions.load(Ordering::SeqCst), 0);
    assert_eq!(must(server.seen.lock())[1].body["kind"], "printed");
}

#[tokio::test]
async fn completed_result_is_retried_after_a_lost_response_without_reprinting() {
    let server = https(vec![
        Reply::Lost,
        Reply::Json(json!(terminal(AttemptState::Printed))),
    ])
    .await;
    let directory = must(tempfile::tempdir());
    let config = config(&directory, &server);
    let mut client = engine(
        &config,
        Work::Reporting {
            attempt: registered(),
            report: Report::Printed(attempt().job(42)),
        },
        false,
        FakePrinter::default(),
    );
    assert!(client.step(false).await.is_err());
    assert!(matches!(
        must(locked(&client.store)).state.work,
        Work::Reporting { .. }
    ));
    must(client.step(false).await);
    let seen = must(server.seen.lock());
    assert_eq!(seen[0].body, seen[1].body);
    assert!(
        seen.iter()
            .all(|request| request.path.ends_with("/reports"))
    );
}

#[tokio::test]
async fn missing_deleted_and_reused_job_ids_are_unknown_not_success() {
    for observation in [
        JobObservation::Missing,
        JobObservation::Deleted,
        JobObservation::IdentityMismatch,
    ] {
        let server = https(vec![Reply::Json(json!(registered()))]).await;
        let directory = must(tempfile::tempdir());
        let config = config(&directory, &server);
        let printer = FakePrinter::default();
        must(printer.observations.lock()).push_back(observation);
        let mut client = engine(
            &config,
            Work::Observing {
                attempt: registered(),
                job: attempt().job(42),
            },
            false,
            printer,
        );
        must(client.step(false).await);
        assert!(matches!(
            must(locked(&client.store)).state.work,
            Work::Reporting {
                report: Report::Unknown,
                ..
            }
        ));
    }
}

#[tokio::test]
async fn sent_to_device_and_faulted_matching_jobs_keep_observing() {
    for observation in [JobObservation::SentToDevice, JobObservation::NeedsAttention] {
        let server = https(vec![
            Reply::Json(json!(registered())),
            Reply::Json(json!(registered())),
        ])
        .await;
        let directory = must(tempfile::tempdir());
        let config = config(&directory, &server);
        let printer = FakePrinter::default();
        must(printer.observations.lock()).push_back(observation);
        let mut client = engine(
            &config,
            Work::Observing {
                attempt: registered(),
                job: attempt().job(42),
            },
            false,
            printer,
        );
        must(client.step(false).await);
        assert!(matches!(
            must(locked(&client.store)).state.work,
            Work::Observing { .. }
        ));
        assert_eq!(must(server.seen.lock())[1].body["kind"], "observed");
    }
}

#[tokio::test]
async fn terminal_server_attempt_clears_local_work_without_observing_or_submitting() {
    let server = https(vec![Reply::Json(json!(terminal(AttemptState::Unknown)))]).await;
    let directory = must(tempfile::tempdir());
    let config = config(&directory, &server);
    let mut client = engine(
        &config,
        Work::Observing {
            attempt: registered(),
            job: attempt().job(42),
        },
        false,
        FakePrinter::default(),
    );
    must(client.step(false).await);
    assert_eq!(must(locked(&client.store)).state.work, Work::Idle);
}

#[tokio::test]
async fn paused_or_stopping_clients_do_not_start_a_new_claim() {
    let server = https(Vec::new()).await;
    let directory = must(tempfile::tempdir());
    let config = config(&directory, &server);
    let mut client = engine(&config, Work::Idle, true, FakePrinter::default());
    must(client.step(false).await);
    must(locked(&client.store))
        .pause(false)
        .unwrap_or_else(|error| panic!("resume failed: {error}"));
    must(client.step(true).await);
    assert!(must(server.seen.lock()).is_empty());
}

#[tokio::test]
async fn mismatched_server_identity_or_job_never_reaches_the_printer() {
    for reply in [
        Attempt {
            client_id: PRINTER.to_owned(),
            ..registered()
        },
        Attempt {
            job_id: Some(43),
            ..registered()
        },
    ] {
        let server = https(vec![Reply::Json(json!(reply))]).await;
        let directory = must(tempfile::tempdir());
        let config = config(&directory, &server);
        let mut client = engine(
            &config,
            Work::Observing {
                attempt: registered(),
                job: attempt().job(42),
            },
            false,
            FakePrinter::default(),
        );
        assert!(client.step(false).await.is_err());
        assert_eq!(must(server.seen.lock()).len(), 1);
    }
}

#[tokio::test]
async fn tls_response_limits_redirects_and_pdf_hash_are_enforced() {
    let server = https(vec![
        Reply::Redirect("https://example.invalid/".to_owned()),
        Reply::Oversized,
        Reply::Pdf(b"%PDF-wrong".to_vec()),
        Reply::Status(401),
    ])
    .await;
    let directory = must(tempfile::tempdir());
    let config = config(&directory, &server);
    let api = api(&config);
    assert!(matches!(
        api.current().await,
        Err(ClientError::Rejected { status: 307 })
    ));
    assert!(matches!(api.current().await, Err(ClientError::Response)));
    assert!(matches!(
        api.document(&attempt()).await,
        Err(ClientError::Response)
    ));
    assert!(matches!(
        api.current().await,
        Err(ClientError::Rejected { status: 401 })
    ));
    let untrusted_server = https(vec![Reply::Json(Value::Null)]).await;
    let mut untrusted = config;
    untrusted.server_url = untrusted_server.origin.clone();
    untrusted.ca_certificate = None;
    assert!(matches!(
        must(Api::new(&untrusted)).current().await,
        Err(ClientError::Transport { .. })
    ));
}

#[tokio::test]
async fn pairing_uses_separate_endpoints_and_has_no_json_version_field() {
    let server = https(vec![Reply::Json(json!({ "printer_id":PRINTER, "client_id":CLIENT, "token":format!("npc1.{CLIENT}.{}", "a".repeat(64)) })),
        Reply::Json(json!({ "printer_id":PRINTER, "name":"Printer", "client_id":CLIENT, "client_confirmed":true, "pairing_expires_at_unix_ms":null }))]).await;
    let directory = must(tempfile::tempdir());
    let config = config(&directory, &server);
    let mut api = must(Api::new(&config));
    let pairing = must(api.pair("ABCD-2345").await);
    must(pairing.validate());
    api.authenticate(pairing.token);
    must(must(api.confirm().await).validate(&binding()));
    let seen = must(server.seen.lock());
    assert!(!seen[0].authenticated);
    assert!(seen[1].authenticated);
    assert_eq!(seen[0].body, json!({"code":"ABCD-2345"}));
    assert!(seen[0].path.ends_with("/pair"));
    let malformed = Pairing {
        printer_id: PRINTER.to_owned(),
        client_id: CLIENT.to_owned(),
        token: "wrong".to_owned(),
    };
    assert!(malformed.validate().is_err());
}

#[tokio::test]
async fn configuration_and_saved_binding_cannot_silently_change_server_or_queue() {
    let server = https(Vec::new()).await;
    let directory = must(tempfile::tempdir());
    let mut config = config(&directory, &server);
    drop(must(Store::open(&config)));
    config.queue_name = "Other queue".to_owned();
    assert!(Store::open(&config).is_err());
    let path = directory.path().join("config.toml");
    for origin in [
        "http://example.test",
        "https://user:password@example.test",
        "https://example.test/path",
        "https://example.test/?query",
        "https://example.test/#fragment",
    ] {
        must(std::fs::write(
            &path,
            format!(
                "server_url = {origin:?}\nqueue_name = 'Q'\nstate_directory = {}\n",
                json!(directory.path())
            ),
        ));
        assert!(Config::load(&path).is_err());
    }
    must(std::fs::write(
        &path,
        format!(
            "server_url = 'https://example.test/'\nqueue_name = 'Q'\nstate_directory = {}\n",
            json!(directory.path())
        ),
    ));
    assert_eq!(must(Config::load(&path)).server_url, "https://example.test");
}
