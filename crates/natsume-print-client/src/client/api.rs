use reqwest::{
    Method,
    header::{AUTHORIZATION, HeaderValue},
};
use secrecy::{ExposeSecret as _, SecretString};
use serde::de::DeserializeOwned;
use sha2::{Digest as _, Sha256};
use snafu::{ResultExt as _, ensure};

use super::{
    config::Config,
    error::{RejectedSnafu, ResponseSnafu, Result, TransportSnafu},
    wire::{Attempt, Job, Pairing, PrinterInfo, Report},
};

#[derive(Clone)]
pub(super) struct Api {
    http: reqwest::Client,
    origin: String,
    token: Option<SecretString>,
}

impl Api {
    pub fn new(config: &Config) -> Result<Self> {
        Ok(Self {
            http: config.http()?,
            origin: config.server_url.clone(),
            token: None,
        })
    }

    pub fn authenticate(&mut self, token: String) {
        self.token = Some(token.into());
    }

    fn request(&self, method: Method, path: &str) -> Result<reqwest::RequestBuilder> {
        let mut request = self
            .http
            .request(method, format!("{}/api/v2/print/{path}", self.origin));
        if let Some(token) = &self.token {
            let mut header = HeaderValue::from_str(&format!("Bearer {}", token.expose_secret()))
                .map_err(|_| ResponseSnafu.build())?;
            header.set_sensitive(true);
            request = request.header(AUTHORIZATION, header);
        }
        Ok(request)
    }

    async fn bytes(request: reqwest::RequestBuilder, limit: usize) -> Result<Vec<u8>> {
        let mut response = request.send().await.context(TransportSnafu)?;
        ensure!(
            response.status().is_success(),
            RejectedSnafu {
                status: response.status().as_u16()
            }
        );
        ensure!(
            response
                .content_length()
                .is_none_or(|length| length <= limit as u64),
            ResponseSnafu
        );
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.context(TransportSnafu)? {
            ensure!(
                chunk.len() <= limit.saturating_sub(bytes.len()),
                ResponseSnafu
            );
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }

    async fn json<T: DeserializeOwned>(request: reqwest::RequestBuilder) -> Result<T> {
        let bytes = Self::bytes(request, 512 * 1024).await?;
        serde_json::from_slice(&bytes).map_err(|_| ResponseSnafu.build())
    }

    pub async fn pair(&self, code: &str) -> Result<Pairing> {
        let pairing: Pairing = Self::json(
            self.request(Method::POST, "client/pair")?
                .json(&serde_json::json!({"code":code})),
        )
        .await?;
        pairing.validate()?;
        Ok(pairing)
    }

    pub async fn confirm(&self) -> Result<PrinterInfo> {
        Self::json(self.request(Method::POST, "client/confirmation")?).await
    }

    pub async fn current(&self) -> Result<Option<Attempt>> {
        Self::json(self.request(Method::GET, "client/attempts/current")?).await
    }

    pub async fn claim(&self, request_id: &str, queue_name: &str) -> Result<Option<Attempt>> {
        Self::json(
            self.request(Method::POST, "client/claims")?
                .json(&serde_json::json!({"request_id":request_id,"queue_name":queue_name})),
        )
        .await
    }

    pub async fn attempt(&self, id: &str) -> Result<Attempt> {
        Self::json(self.request(Method::GET, &format!("client/attempts/{id}"))?).await
    }

    pub async fn register(&self, id: &str, job: &Job) -> Result<Attempt> {
        Self::json(
            self.request(Method::PUT, &format!("client/attempts/{id}/job"))?
                .json(job),
        )
        .await
    }

    pub async fn report(&self, id: &str, report: &Report) -> Result<Attempt> {
        Self::json(
            self.request(Method::POST, &format!("client/attempts/{id}/reports"))?
                .json(report),
        )
        .await
    }

    pub async fn document(&self, attempt: &Attempt) -> Result<Vec<u8>> {
        let bytes = Self::bytes(
            self.request(
                Method::GET,
                &format!("client/attempts/{}/document", attempt.attempt_id),
            )?,
            8 * 1024 * 1024,
        )
        .await?;
        ensure!(
            bytes.len() as u64 == attempt.document_bytes
                && hex::encode(Sha256::digest(&bytes)) == attempt.document_sha256,
            ResponseSnafu
        );
        Ok(bytes)
    }
}
