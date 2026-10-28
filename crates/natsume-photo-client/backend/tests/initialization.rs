#![cfg(windows)]
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Arc, Mutex},
};

use natsume_photo_client::{
    initialization::{Config, Error as InitError, Initializer, Secrets},
    workspace::Workspace,
};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use snafu::Snafu;
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::TcpListener,
};
use tokio_rustls::TlsAcceptor;
use tokio_util::task::AbortOnDropHandle;
use zeroize::Zeroizing;

const SOURCE: &str = "01900000-0000-7000-8000-000000000001";
const ENROLLMENT: &str = "01900000-0000-7000-8000-000000000002";
const DATASET: &str = "01900000-0000-7000-8000-000000000003";
const SNAPSHOT: &str = "01900000-0000-7000-8000-000000000004";

#[derive(Debug, Snafu)]
enum TestError {
    #[snafu(context(false))]
    Io { source: std::io::Error },
    #[snafu(context(false))]
    Init { source: InitError },
    #[snafu(context(false))]
    Json { source: serde_json::Error },
    #[snafu(context(false))]
    Tls { source: rustls::Error },
    #[snafu(context(false))]
    Certificate { source: rcgen::Error },
    #[snafu(context(false))]
    Image { source: image::ImageError },
    #[snafu(context(false))]
    Workspace {
        source: natsume_photo_client::workspace::Error,
    },
    #[snafu(display("initialization test fixture is invalid"))]
    Fixture,
}
type Result<T = ()> = std::result::Result<T, TestError>;

#[derive(Clone, Default)]
struct MemorySecrets(Arc<Mutex<BTreeMap<String, Zeroizing<String>>>>);
impl Secrets for MemorySecrets {
    fn load(
        &self,
        origin: &str,
        slot: &str,
    ) -> natsume_photo_client::initialization::Result<Option<Zeroizing<String>>> {
        Ok(self
            .0
            .lock()
            .map_err(|_| InitError::Invalid)?
            .get(&format!("{origin}/{slot}"))
            .cloned())
    }
    fn save(
        &self,
        origin: &str,
        slot: &str,
        value: &str,
    ) -> natsume_photo_client::initialization::Result<()> {
        self.0
            .lock()
            .map_err(|_| InitError::Invalid)?
            .insert(format!("{origin}/{slot}"), Zeroizing::new(value.into()));
        Ok(())
    }
}

enum Reply {
    Pair,
    Confirm,
    Manifest(Vec<u8>),
    Logo(Vec<u8>),
    Complete,
    Lost,
    Redirect,
    Oversized,
}
struct Request {
    path: String,
    body: Value,
    bearer: bool,
}
struct Https {
    origin: String,
    ca: String,
    seen: Arc<Mutex<Vec<Request>>>,
    _task: AbortOnDropHandle<Result>,
}
impl Https {
    async fn start(replies: Vec<Reply>) -> Result<Self> {
        let cert = rcgen::generate_simple_self_signed(vec!["127.0.0.1".into()])?;
        let tls = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_no_client_auth()
        .with_single_cert(
            vec![cert.cert.der().clone()],
            rustls::pki_types::PrivatePkcs8KeyDer::from(cert.signing_key.serialize_der()).into(),
        )?;
        let acceptor = TlsAcceptor::from(Arc::new(tls));
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let origin = format!("https://{}", listener.local_addr()?);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let requests = Arc::clone(&seen);
        let task = tokio::spawn(async move {
            for reply in replies {
                let (socket, _) = listener.accept().await?;
                let Ok(mut socket) = acceptor.accept(socket).await else {
                    continue;
                };
                let request = read_request(&mut socket).await?;
                requests
                    .lock()
                    .map_err(|_| TestError::Fixture)?
                    .push(request);
                if matches!(reply, Reply::Lost) {
                    continue;
                }
                let (status, headers, bytes) = response(reply)?;
                let head = format!(
                    "HTTP/1.1 {status} response\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n",
                    bytes.len()
                );
                socket.write_all(head.as_bytes()).await?;
                socket.write_all(&bytes).await?;
            }
            Ok(())
        });
        Ok(Self {
            origin,
            ca: cert.cert.pem(),
            seen,
            _task: AbortOnDropHandle::new(task),
        })
    }
    fn requests(&self) -> Result<std::sync::MutexGuard<'_, Vec<Request>>> {
        self.seen.lock().map_err(|_| TestError::Fixture)
    }
}

async fn read_request(socket: &mut (impl tokio::io::AsyncRead + Unpin)) -> Result<Request> {
    let mut bytes = Vec::new();
    let end = loop {
        let mut block = [0; 4096];
        let count = socket.read(&mut block).await?;
        if count == 0 || bytes.len() > 16384 {
            return FixtureSnafu.fail();
        }
        bytes.extend_from_slice(&block[..count]);
        if let Some(end) = bytes.windows(4).position(|b| b == b"\r\n\r\n") {
            break end + 4;
        }
    };
    let head = String::from_utf8_lossy(&bytes[..end]).into_owned();
    let length = head
        .lines()
        .find_map(|l| {
            l.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .and_then(|s| s.trim().parse::<usize>().ok())
        })
        .unwrap_or(0);
    if length > 8192 {
        return FixtureSnafu.fail();
    }
    while bytes.len() < end + length {
        let mut block = [0; 4096];
        let count = socket.read(&mut block).await?;
        if count == 0 {
            return FixtureSnafu.fail();
        }
        bytes.extend_from_slice(&block[..count]);
    }
    Ok(Request {
        path: head
            .split_ascii_whitespace()
            .nth(1)
            .ok_or(TestError::Fixture)?
            .into(),
        body: if length == 0 {
            Value::Null
        } else {
            serde_json::from_slice(&bytes[end..end + length])?
        },
        bearer: head
            .to_ascii_lowercase()
            .contains("authorization: bearer nph1."),
    })
}

fn response(reply: Reply) -> Result<(u16, String, Vec<u8>)> {
    let (status, headers, bytes) = match reply {
        Reply::Pair => (
            201,
            String::new(),
            serde_json::to_vec(
                &json!({"source_id":SOURCE,"enrollment_id":ENROLLMENT,"token":format!("nph1.{SOURCE}.{ENROLLMENT}.{}", "1".repeat(64)),"expires_at_unix_ms":1}),
            )?,
        ),
        Reply::Confirm => (
            200,
            String::new(),
            serde_json::to_vec(
                &json!({"source_id":SOURCE,"name":"desk","state":"enabled","enrollment_confirmed":true,"initialization_completed":false}),
            )?,
        ),
        Reply::Manifest(bytes) => (
            200,
            format!(
                "Content-Type: application/json\r\nETag: \"{}\"\r\n",
                hex::encode(Sha256::digest(&bytes))
            ),
            bytes,
        ),
        Reply::Logo(bytes) => (200, "Content-Type: image/png\r\n".into(), bytes),
        Reply::Complete => (204, String::new(), Vec::new()),
        Reply::Redirect => (
            307,
            "Location: https://127.0.0.1:1/steal\r\n".into(),
            Vec::new(),
        ),
        Reply::Oversized => (201, String::new(), vec![b' '; 8193]),
        Reply::Lost => return FixtureSnafu.fail(),
    };
    Ok((status, headers, bytes))
}

fn png() -> Result<Vec<u8>> {
    let mut output = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_pixel(4, 2, image::Rgba([255, 0, 0, 255]))
        .write_to(&mut output, image::ImageFormat::Png)?;
    Ok(output.into_inner())
}
fn manifest(image: &[u8]) -> Result<Vec<u8>> {
    let team = |id: &str| json!({"account_id":id,"account":id,"organization_id":7,"school_name_zh":"学校","school_name_en":"School","team_name_zh":"队伍","team_name_en":"Team"});
    Ok(serde_json::to_vec(
        &json!({"format_version":1,"source_id":SOURCE,"dataset_id":DATASET,"initialization_id":SNAPSHOT,"teams":[team("a"),team("b")],"school_logos":[{"organization_id":7,"sha256":hex::encode(Sha256::digest(image)),"content_type":"image/png","size_bytes":image.len()}]}),
    )?)
}
fn config(server: &Https, directory: &Path) -> Result<Config> {
    let ca = directory.join("ca.pem");
    std::fs::write(&ca, &server.ca)?;
    Ok(Config {
        server_url: server.origin.clone(),
        workspace: directory.join("workspace"),
        ca_certificate: Some(ca),
    })
}
fn reopen(
    config: &Config,
    secrets: &MemorySecrets,
) -> natsume_photo_client::initialization::Result<Initializer<MemorySecrets>> {
    Initializer::open(
        Config {
            server_url: config.server_url.clone(),
            workspace: config.workspace.clone(),
            ca_certificate: config.ca_certificate.clone(),
        },
        secrets.clone(),
    )
}

#[tokio::test]
async fn https_enrollment_downloads_shared_logo_and_then_opens_offline() -> Result {
    let image = png()?;
    let bytes = manifest(&image)?;
    let server = Https::start(vec![
        Reply::Pair,
        Reply::Confirm,
        Reply::Manifest(bytes.clone()),
        Reply::Logo(image.clone()),
        Reply::Complete,
    ])
    .await?;
    let directory = tempfile::tempdir()?;
    let config = config(&server, directory.path())?;
    let secrets = MemorySecrets::default();
    let initializer = reopen(&config, &secrets)?;
    assert!(initializer.needs_pairing_code()?);
    let workspace = initializer.run(Some("ABCD-EFGH")).await?;
    assert_eq!(workspace.roster().await?.teams().len(), 2);
    assert_eq!(
        workspace.logo_path("a").await?,
        workspace.logo_path("b").await?
    );
    assert_eq!(
        std::fs::read(config.workspace.join("initialization.json"))?,
        bytes
    );
    let key = {
        let seen = server.requests()?;
        assert_eq!(seen.len(), 5);
        assert!(!seen[0].bearer);
        assert!(seen[1..].iter().all(|r| r.bearer));
        assert_eq!(seen[4].body.as_object().map(serde_json::Map::len), Some(3));
        let key = secrets
            .0
            .lock()
            .map_err(|_| TestError::Fixture)?
            .iter()
            .find(|(k, _)| k.ends_with("/signing"))
            .map(|(_, v)| v.clone())
            .ok_or(TestError::Fixture)?;
        let mut signing_seed = [0; 32];
        hex::decode_to_slice(key.as_str(), &mut signing_seed).map_err(|_| TestError::Fixture)?;
        assert_eq!(
            seen[0].body["signing_public_key"],
            hex::encode(
                ed25519_dalek::SigningKey::from_bytes(&signing_seed)
                    .verifying_key()
                    .to_bytes()
            )
        );
        key
    };
    std::fs::remove_file(config.ca_certificate.as_ref().ok_or(TestError::Fixture)?)?;
    drop(server);
    drop(workspace);
    let offline = reopen(&config, &secrets)?.run(None).await?;
    assert_eq!(offline.roster().await?.teams().len(), 2);
    let database_before = std::fs::read(config.workspace.join("workspace.sqlite3"))?;
    natsume_photo_client::initialization::diagnose(&offline, &secrets).await?;
    assert_eq!(
        std::fs::read(config.workspace.join("workspace.sqlite3"))?,
        database_before
    );
    assert_eq!(
        Workspace::open(&config.workspace)
            .await?
            .roster()
            .await?
            .teams()
            .len(),
        2
    );
    let journal = std::fs::read_to_string(
        directory
            .path()
            .join(".workspace.photo-init/initialization.jsonl"),
    )?;
    assert!(!journal.contains(key.as_str()));
    assert!(!journal.contains("nph1."));
    Ok(())
}

#[tokio::test]
async fn lost_completion_replays_the_same_receipt_without_redownloading() -> Result {
    let image = png()?;
    let bytes = manifest(&image)?;
    let server = Https::start(vec![
        Reply::Pair,
        Reply::Confirm,
        Reply::Manifest(bytes.clone()),
        Reply::Logo(image),
        Reply::Lost,
        Reply::Complete,
    ])
    .await?;
    let directory = tempfile::tempdir()?;
    let config = config(&server, directory.path())?;
    let secrets = MemorySecrets::default();
    assert!(
        reopen(&config, &secrets)?
            .run(Some("ABCD-EFGH"))
            .await
            .is_err()
    );
    assert!(!config.workspace.exists());
    let journal = directory
        .path()
        .join(".workspace.photo-init/initialization.jsonl");
    std::fs::OpenOptions::new()
        .append(true)
        .open(journal)?
        .write_all(b"{torn")?;
    drop(reopen(&config, &secrets)?.run(None).await?);
    let seen = server.requests()?;
    assert_eq!(seen.len(), 6);
    assert_eq!(seen[4].body, seen[5].body);
    assert_eq!(
        seen[5].body["manifest_sha256"],
        hex::encode(Sha256::digest(bytes))
    );
    assert!(seen[5].path.ends_with("initialization/complete"));
    Ok(())
}

#[tokio::test]
async fn corrupt_logo_stops_completion_and_retry_keeps_the_same_binding() -> Result {
    let image = png()?;
    let bytes = manifest(&image)?;
    let server = Https::start(vec![
        Reply::Pair,
        Reply::Confirm,
        Reply::Manifest(bytes.clone()),
        Reply::Logo(b"wrong".to_vec()),
        Reply::Confirm,
        Reply::Manifest(bytes),
        Reply::Logo(image),
        Reply::Complete,
    ])
    .await?;
    let directory = tempfile::tempdir()?;
    let config = config(&server, directory.path())?;
    let secrets = MemorySecrets::default();
    assert!(matches!(
        reopen(&config, &secrets)?.run(Some("ABCD-EFGH")).await,
        Err(InitError::Invalid)
    ));
    assert!(!config.workspace.exists());
    drop(reopen(&config, &secrets)?.run(None).await?);
    let seen = server.requests()?;
    assert_eq!(
        seen.iter()
            .filter(|r| r.path.ends_with("enrollment/pair"))
            .count(),
        1
    );
    assert_eq!(
        seen.iter()
            .filter(|r| r.path.ends_with("initialization/complete"))
            .count(),
        1
    );
    Ok(())
}

#[tokio::test]
async fn wrong_source_or_secret_fields_in_manifest_are_rejected() -> Result {
    for field in ["source_id", "password"] {
        let image = png()?;
        let mut value: Value = serde_json::from_slice(&manifest(&image)?)?;
        if field == "source_id" {
            value[field] = json!(DATASET);
        } else {
            value["teams"][0][field] = json!("must-not-import");
        }
        let server = Https::start(vec![
            Reply::Pair,
            Reply::Confirm,
            Reply::Manifest(serde_json::to_vec(&value)?),
        ])
        .await?;
        let directory = tempfile::tempdir()?;
        let config = config(&server, directory.path())?;
        assert!(
            reopen(&config, &MemorySecrets::default())?
                .run(Some("code"))
                .await
                .is_err()
        );
        assert!(!config.workspace.exists());
    }
    Ok(())
}

#[tokio::test]
async fn tls_redirect_and_body_limits_are_enforced() -> Result {
    for reply in [Reply::Redirect, Reply::Oversized] {
        let server = Https::start(vec![reply]).await?;
        let directory = tempfile::tempdir()?;
        let config = config(&server, directory.path())?;
        assert!(
            reopen(&config, &MemorySecrets::default())?
                .run(Some("code"))
                .await
                .is_err()
        );
        assert_eq!(server.requests()?.len(), 1);
    }
    let server = Https::start(vec![Reply::Pair]).await?;
    let directory = tempfile::tempdir()?;
    let mut config = config(&server, directory.path())?;
    config.ca_certificate = None;
    assert!(matches!(
        reopen(&config, &MemorySecrets::default())?
            .run(Some("code"))
            .await,
        Err(InitError::Transport { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn origin_paths_exclusive_lock_and_existing_targets_are_checked() -> Result {
    let directory = tempfile::tempdir()?;
    let secrets = MemorySecrets::default();
    for origin in [
        "http://localhost",
        "https://user:password@localhost",
        "https://localhost/path",
        "https://localhost/?key=x",
    ] {
        let config = Config {
            server_url: origin.into(),
            workspace: directory.path().join("invalid"),
            ca_certificate: None,
        };
        assert!(matches!(
            Initializer::open(config, secrets.clone()),
            Err(InitError::Invalid)
        ));
    }
    let config = Config {
        server_url: "https://localhost".into(),
        workspace: directory.path().join("workspace"),
        ca_certificate: None,
    };
    let first = reopen(&config, &secrets)?;
    assert!(matches!(reopen(&config, &secrets), Err(InitError::Busy)));
    drop(first);
    assert!(reopen(&config, &secrets)?.needs_pairing_code()?);
    Ok(())
}

#[tokio::test]
async fn lost_pairing_does_not_allow_a_missing_key_to_be_regenerated() -> Result {
    let server = Https::start(vec![Reply::Lost]).await?;
    let directory = tempfile::tempdir()?;
    let config = config(&server, directory.path())?;
    let secrets = MemorySecrets::default();
    assert!(reopen(&config, &secrets)?.run(Some("code")).await.is_err());
    secrets.0.lock().map_err(|_| TestError::Fixture)?.clear();
    assert!(matches!(
        reopen(&config, &secrets)?.run(Some("replacement")).await,
        Err(InitError::MissingSecret)
    ));
    Ok(())
}

#[tokio::test]
async fn damaged_saved_workspace_prevents_completion_replay() -> Result {
    let image = png()?;
    let server = Https::start(vec![
        Reply::Pair,
        Reply::Confirm,
        Reply::Manifest(manifest(&image)?),
        Reply::Logo(image),
        Reply::Lost,
    ])
    .await?;
    let directory = tempfile::tempdir()?;
    let config = config(&server, directory.path())?;
    let secrets = MemorySecrets::default();
    assert!(reopen(&config, &secrets)?.run(Some("code")).await.is_err());
    let staging = directory.path().join(".workspace.photo-init");
    let journal = std::fs::read_to_string(staging.join("initialization.jsonl"))?;
    let record: Value = serde_json::from_str(journal.lines().last().ok_or(TestError::Fixture)?)?;
    let prepared = record["state"]["phase"]["receipt"]["prepared"]
        .as_str()
        .ok_or(TestError::Fixture)?;
    std::fs::remove_file(staging.join(prepared).join("workspace.sqlite3"))?;
    assert!(matches!(
        reopen(&config, &secrets)?.run(None).await,
        Err(InitError::Workspace { .. })
    ));
    assert!(!config.workspace.exists());
    Ok(())
}

#[test]
fn changed_origin_corrupt_journal_and_existing_directory_are_not_reused() -> Result {
    let directory = tempfile::tempdir()?;
    let secrets = MemorySecrets::default();
    let mut config = Config {
        server_url: "https://localhost".into(),
        workspace: directory.path().join("workspace"),
        ca_certificate: None,
    };
    drop(reopen(&config, &secrets)?);
    config.server_url = "https://other.example".into();
    assert!(matches!(reopen(&config, &secrets), Err(InitError::Invalid)));
    config.server_url = "https://localhost".into();
    let journal = directory
        .path()
        .join(".workspace.photo-init/initialization.jsonl");
    std::fs::write(&journal, b"{}\n")?;
    assert!(matches!(
        reopen(&config, &secrets),
        Err(InitError::Json { .. })
    ));
    config.workspace = directory.path().join("existing");
    std::fs::create_dir(&config.workspace)?;
    std::fs::write(config.workspace.join("keep"), b"original")?;
    assert!(matches!(reopen(&config, &secrets), Err(InitError::Invalid)));
    assert_eq!(std::fs::read(config.workspace.join("keep"))?, b"original");
    Ok(())
}

use std::io::Write as _;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct LiveInput {
    origin: String,
    code: Zeroizing<String>,
    ca: std::path::PathBuf,
    workspace: std::path::PathBuf,
}

#[tokio::test]
#[ignore = "started by the WSL Server interoperability test"]
async fn windows_client_against_live_server() -> Result {
    let path = std::env::var_os("NATSUME_PHOTO_INTEROP_INPUT").ok_or(TestError::Fixture)?;
    let input: LiveInput = serde_json::from_slice(&std::fs::read(path)?)?;
    let secrets = MemorySecrets::default();
    let config = Config {
        server_url: input.origin,
        workspace: input.workspace,
        ca_certificate: Some(input.ca),
    };
    let workspace = reopen(&config, &secrets)?.run(Some(&input.code)).await?;
    assert_eq!(workspace.roster().await?.teams().len(), 2);
    assert!(workspace.logo_path("account-a").await?.is_some());
    drop(workspace);
    let offline = Workspace::open(&config.workspace).await?;
    natsume_photo_client::initialization::diagnose(&offline, &secrets).await?;
    Ok(())
}
