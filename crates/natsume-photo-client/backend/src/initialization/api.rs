use std::time::Duration;

use reqwest::{Client, Method, StatusCode, header};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use snafu::{ResultExt as _, ensure};

use super::{
    Config, InvalidSnafu, IoSnafu, RejectedSnafu, Result, TransportSnafu,
    store::Receipt,
    wire::{self, Logo, Manifest, PairingCredentials},
};

pub(super) fn origin(value: &str) -> Result<String> {
    let url = url::Url::parse(value).context(super::UrlSnafu)?;
    ensure!(
        url.scheme() == "https"
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.path() == "/"
            && url.query().is_none()
            && url.fragment().is_none(),
        super::OriginSnafu
    );
    Ok(url.origin().ascii_serialization())
}

pub(super) struct Api {
    http: Client,
    origin: String,
}
impl Api {
    pub fn new(config: &Config) -> Result<Self> {
        let mut builder = Client::builder()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(30));
        if let Some(path) = &config.ca_certificate {
            let bytes = std::fs::read(path).context(IoSnafu)?;
            let certificates =
                reqwest::Certificate::from_pem_bundle(&bytes).context(TransportSnafu)?;
            ensure!(!certificates.is_empty(), InvalidSnafu);
            for certificate in certificates {
                builder = builder.add_root_certificate(certificate);
            }
        }
        Ok(Self {
            http: builder.build().context(TransportSnafu)?,
            origin: config.server_url.clone(),
        })
    }
    fn request(&self, method: Method, path: &str) -> reqwest::RequestBuilder {
        self.http
            .request(method, format!("{}/api/v2/photo/{path}", self.origin))
    }
    async fn bytes(
        request: reqwest::RequestBuilder,
        status: StatusCode,
        limit: usize,
    ) -> Result<(reqwest::header::HeaderMap, Vec<u8>)> {
        let retryable = request.try_clone();
        let mut response = request.send().await.context(TransportSnafu)?;
        if let Some(request) = retryable {
            for attempt in 0..4_u32 {
                // Only snapshot/download capacity rejections carry Retry-After.
                // Pairing and acknowledgements are deliberately not retried.
                if response.status() != StatusCode::SERVICE_UNAVAILABLE {
                    break;
                }
                let Some(delay) = response
                    .headers()
                    .get(header::RETRY_AFTER)
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| value.parse::<u64>().ok())
                else {
                    break;
                };
                let url = response.url().path();
                if !url.ends_with("/client/initialization") && !url.contains("/client/resources/") {
                    break;
                }
                let Some(next) = request.try_clone() else {
                    break;
                };
                drop(response);
                tokio::time::sleep(Duration::from_secs(
                    delay.clamp(1, 8) * (1_u64 << attempt).min(4),
                ))
                .await;
                response = next.send().await.context(TransportSnafu)?;
            }
        }
        ensure!(
            response.status() == status,
            RejectedSnafu {
                status: response.status().as_u16()
            }
        );
        ensure!(
            response
                .content_length()
                .is_none_or(|length| length <= limit as u64),
            super::LimitSnafu { limit }
        );
        let headers = response.headers().clone();
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.context(TransportSnafu)? {
            ensure!(
                chunk.len() <= limit.saturating_sub(bytes.len()),
                super::LimitSnafu { limit }
            );
            bytes.extend_from_slice(&chunk);
        }
        Ok((headers, bytes))
    }
    async fn json<T: DeserializeOwned>(
        request: reqwest::RequestBuilder,
        status: StatusCode,
    ) -> Result<T> {
        let (_, bytes) = Self::bytes(request, status, 8192).await?;
        serde_json::from_slice(&bytes).map_err(super::json_error)
    }
    pub async fn pair(&self, code: &str, public_key: &str) -> Result<PairingCredentials> {
        let issued: PairingCredentials = Self::json(self.request(Method::POST, "enrollment/pair").json(&json!({"code":code,"signing_public_key":public_key,"client_version":env!("CARGO_PKG_VERSION")})), StatusCode::CREATED).await?;
        issued.validate().context(super::ContractSnafu)?;
        Ok(issued)
    }
    pub async fn confirm(&self, enrollment: &PairingCredentials, public_key: &str) -> Result<()> {
        let value: Value = Self::json(self.request(Method::POST, "enrollment/confirm").bearer_auth(&enrollment.token).json(&json!({"enrollment_id":enrollment.enrollment_id,"signing_public_key":public_key})), StatusCode::OK).await?;
        ensure!(
            value["source_id"] == enrollment.source_id
                && value["state"] == "enabled"
                && value["enrollment_confirmed"] == true
                && value["initialization_completed"] == false,
            InvalidSnafu
        );
        Ok(())
    }
    pub async fn manifest(&self, enrollment: &PairingCredentials) -> Result<(Vec<u8>, Manifest)> {
        let (headers, bytes) = Self::bytes(
            self.request(Method::POST, "client/initialization")
                .bearer_auth(&enrollment.token),
            StatusCode::OK,
            wire::MANIFEST_LIMIT,
        )
        .await?;
        ensure!(
            headers.get(header::ETAG).and_then(|h| h.to_str().ok())
                == Some(format!("\"{}\"", wire::digest(&bytes)).as_str()),
            InvalidSnafu
        );
        let manifest: Manifest = serde_json::from_slice(&bytes).map_err(super::json_error)?;
        manifest
            .validate(&enrollment.source_id)
            .context(super::ContractSnafu)?;
        Ok((bytes, manifest))
    }
    pub async fn resource(&self, enrollment: &PairingCredentials, logo: &Logo) -> Result<Vec<u8>> {
        let (headers, bytes) = Self::bytes(
            self.request(Method::GET, &format!("client/resources/{}", logo.sha256))
                .bearer_auth(&enrollment.token),
            StatusCode::OK,
            wire::IMAGE_LIMIT,
        )
        .await?;
        ensure!(
            headers
                .get(header::CONTENT_TYPE)
                .and_then(|h| h.to_str().ok())
                == Some("image/png")
                && bytes.starts_with(b"\x89PNG\r\n\x1a\n")
                && bytes.len() as u64 == logo.size_bytes
                && wire::digest(&bytes) == logo.sha256,
            InvalidSnafu
        );
        Ok(bytes)
    }
    pub async fn complete(&self, enrollment: &PairingCredentials, receipt: &Receipt) -> Result<()> {
        Self::bytes(self.request(Method::POST, "client/initialization/complete").bearer_auth(&enrollment.token).json(&json!({"initialization_id":receipt.initialization_id,"manifest_sha256":receipt.manifest_sha256,"request_id":receipt.request_id})), StatusCode::NO_CONTENT, 0).await?;
        Ok(())
    }
}
