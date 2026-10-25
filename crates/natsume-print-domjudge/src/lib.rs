//! Independent `DOMjudge` admission client. No Server or workstation dependency.
#![cfg(target_os = "linux")]
mod compiler;
pub mod config;
pub mod wire;

use std::{fmt, path::PathBuf, time::Duration};

use reqwest::{Method, StatusCode};
use sha2::{Digest, Sha256};
use tokio::time::{Instant, timeout_at};
use tokio_util::sync::CancellationToken;

pub use compiler::Runtime;
use config::{Config, Permission, read_regular};
use wire::{Fact, FailureCode, GenerationFailure, Registration, State};

pub type Result<T> = std::result::Result<T, Error>;
pub const TOTAL_BUDGET: Duration = Duration::from_secs(5);
const RESPONSE_LIMIT: usize = 64 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Configuration,
    UnsafeFile,
    InvalidInput,
    InvalidResponse,
    Rejected(u16),
    Transport,
    Ambiguous,
    Generation(FailureCode),
    AlreadyFailed,
    Deadline,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Configuration => "打印配置不可用",
            Self::UnsafeFile => "源文件或部署文件不安全、过大或不可读",
            Self::InvalidInput => "打印参数或 UTF-8 源文件无效",
            Self::Rejected(_) => "服务器拒绝本次操作；未确认入队",
            Self::Generation(_) => "文档生成失败；未确认入队",
            Self::AlreadyFailed => "该请求已记录生成失败；未入队",
            Self::Deadline => "打印请求超时；未确认入队",
            Self::InvalidResponse | Self::Transport | Self::Ambiguous => {
                "结果无法确认；请保留请求 ID 核实，不要自动重新提交"
            }
        })
    }
}
impl std::error::Error for Error {}

pub struct Input {
    pub file: PathBuf,
    pub account: String,
    pub original: String,
    pub request_id: String,
}

impl Input {
    fn validate(&self) -> Result<()> {
        if !wire::uuid_v7(&self.request_id)
            || self.account.is_empty()
            || self.original.len() > 1024
            || self
                .original
                .chars()
                .any(|c| c.is_control() || matches!(c, '/' | '\\'))
        {
            return Err(Error::InvalidInput);
        }
        Ok(())
    }
}

struct Session<'a> {
    client: reqwest::Client,
    token: String,
    origin: url::Url,
    input: &'a Input,
    deadline: Instant,
    cancellation: &'a CancellationToken,
}

impl Session<'_> {
    async fn exchange(
        &self,
        method: Method,
        path: &str,
        body: Option<(&str, Vec<u8>)>,
    ) -> Result<Fact> {
        if Instant::now() >= self.deadline || self.cancellation.is_cancelled() {
            return Err(Error::Deadline);
        }
        let url = self.origin.join(path).map_err(|_| Error::Configuration)?;
        let mut request = self.client.request(method, url).bearer_auth(&self.token);
        if let Some((content_type, bytes)) = body {
            request = request
                .header(reqwest::header::CONTENT_TYPE, content_type)
                .body(bytes);
        }
        let exchange = async {
            let mut response = request.send().await.map_err(|_| Error::Transport)?;
            if response.status() != StatusCode::OK {
                let status = response.status();
                return Err(if status.is_client_error() {
                    Error::Rejected(status.as_u16())
                } else {
                    Error::Transport
                });
            }
            if response
                .content_length()
                .is_some_and(|n| n > RESPONSE_LIMIT as u64)
            {
                return Err(Error::InvalidResponse);
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|_| Error::Transport)? {
                if chunk.len() > RESPONSE_LIMIT - bytes.len() {
                    return Err(Error::InvalidResponse);
                }
                bytes.extend_from_slice(&chunk);
            }
            let fact: Fact = serde_json::from_slice(&bytes).map_err(|_| Error::InvalidResponse)?;
            fact.validate(&self.input.request_id, &self.input.account)?;
            Ok(fact)
        };
        tokio::select! {
            biased;
            () = self.cancellation.cancelled() => Err(Error::Deadline),
            result = timeout_at(self.deadline, exchange) => result.map_err(|_| Error::Deadline)?,
        }
    }

    fn path(&self) -> String {
        format!("/api/v2/print/requests/{}", self.input.request_id)
    }

    async fn register(&self, registration: Registration<'_>) -> Result<Fact> {
        let body = serde_json::to_vec(&registration).map_err(|_| Error::InvalidInput)?;
        match self
            .exchange(
                Method::POST,
                "/api/v2/print/requests",
                Some(("application/json", body.clone())),
            )
            .await
        {
            Ok(fact) => return Ok(fact),
            Err(error @ Error::Rejected(_)) => return Err(error),
            Err(_) => {}
        }
        // GET omits the original filename and source hash. Replaying this exact
        // registration lets the Server verify every field before we trust an old ID.
        match self
            .exchange(
                Method::POST,
                "/api/v2/print/requests",
                Some(("application/json", body)),
            )
            .await
        {
            Ok(fact) => Ok(fact),
            Err(error @ Error::Rejected(409)) => Err(error),
            Err(_) => Err(Error::Ambiguous),
        }
    }

    async fn verify(&self) -> Result<Fact> {
        self.exchange(Method::GET, &self.path(), None)
            .await
            .map_err(|_| Error::Ambiguous)
    }
}

/// `started` is captured at process entry, before argument/config parsing.
/// Tests supply private config/runtime fixtures without changing the executable's deployment path.
///
/// # Errors
/// Returns bounded input/configuration failures, server rejection, generation failure or an
/// ambiguous outcome. Transport failures after sending a request do not prove rejection.
pub async fn run(
    config: &Config,
    runtime: &Runtime,
    input: &Input,
    started: Instant,
    cancellation: &CancellationToken,
) -> Result<Fact> {
    let deadline = started + TOTAL_BUDGET;
    if Instant::now() >= deadline || cancellation.is_cancelled() {
        return Err(Error::Deadline);
    }
    input.validate()?;
    let source = read_regular(&input.file, 256 * 1024, Permission::Source)?;
    std::str::from_utf8(&source).map_err(|_| Error::InvalidInput)?;
    let (client, token) = config.client()?;
    let session = Session {
        client,
        token,
        origin: config.origin()?,
        input,
        deadline,
        cancellation,
    };
    let registration = Registration {
        request_id: &input.request_id,
        account: &input.account,
        original_filename: &input.original,
        source_sha256: hex::encode(Sha256::digest(&source)),
    };
    let registered = session.register(registration).await?;
    match registered.state {
        State::Ready => return Ok(registered),
        State::GenerationFailed => return Err(Error::AlreadyFailed),
        State::Preparing => {}
    }
    let document = match compiler::compile(
        runtime,
        &registered,
        &input.original,
        &source,
        deadline,
        cancellation,
    )
    .await
    {
        Ok(bytes) => bytes,
        Err(error @ Error::Generation(code)) => {
            // No outbox, compiler output, or retry after this invocation.
            let body =
                serde_json::to_vec(&GenerationFailure { code }).map_err(|_| Error::InvalidInput)?;
            let _ = session
                .exchange(
                    Method::POST,
                    &format!("{}/generation-failure", session.path()),
                    Some(("application/json", body)),
                )
                .await;
            return Err(error);
        }
        Err(error) => return Err(error),
    };
    let document_hash = hex::encode(Sha256::digest(&document));
    let document_bytes = document.len() as u64;
    let uploaded = match session
        .exchange(
            Method::PUT,
            &format!("{}/document", session.path()),
            Some(("application/pdf", document)),
        )
        .await
    {
        Ok(fact) => fact,
        Err(error @ Error::Rejected(_)) => return Err(error),
        Err(_) => session.verify().await?,
    };
    if uploaded.state != State::Ready {
        return Err(Error::Ambiguous);
    }
    if !registered.same_registration(&uploaded)
        || !uploaded
            .document
            .as_ref()
            .is_some_and(|d| d.sha256 == document_hash && d.bytes == document_bytes)
    {
        return Err(Error::Ambiguous);
    }
    Ok(uploaded)
}

/// Display fields are quoted JSON data, never terminal control sequences.
#[must_use]
pub fn success_message(fact: &Fact) -> String {
    let Some(admission) = &fact.admission else {
        return Error::InvalidResponse.to_string();
    };
    let display = |s: &str| serde_json::to_string(s).unwrap_or_default();
    format!(
        "已入队，尚未打印完成。前方任务数：{}；前方页数：{}；处理中：{}。队伍：{}；账号：{}；座位：{}；请求 ID：{}",
        admission.ahead_tasks,
        admission.ahead_pages,
        admission.in_progress_tasks,
        display(&fact.team_name),
        display(&fact.account),
        fact.seat_label
            .as_deref()
            .map_or_else(|| "未分配".to_owned(), display),
        fact.request_id
    )
}
