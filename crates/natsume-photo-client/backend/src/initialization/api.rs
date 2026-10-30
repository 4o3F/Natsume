//! Photo enrollment HTTPS transport; retry and response policies are explicit.
use super::{IoSnafu, RejectedSnafu, Result, ServerOrigin, TransportSnafu};
use ed25519_dalek::VerifyingKey;
use natsume_photo_protocol::{
    PhotoConfirmation, PhotoEnrollmentIssued, PhotoInitialization, PhotoInitializationComplete,
    PhotoLogo, PhotoPairRequest, PhotoSource, Sha256Digest, SourceState,
};
use reqwest::{Client, Method, StatusCode, header};
use serde::de::DeserializeOwned;
use snafu::{ResultExt as _, ensure};
use std::{path::PathBuf, time::Duration};

#[derive(Clone, Copy)]
enum Endpoint<'a> {
    Pair,
    Confirm,
    PrepareInitialization,
    Resource(&'a Sha256Digest),
    Complete,
}
impl Endpoint<'_> {
    fn path(&self) -> String {
        match self {
            Self::Pair => "enrollment/pair".into(),
            Self::Confirm => "enrollment/confirm".into(),
            Self::PrepareInitialization => "client/initialization".into(),
            Self::Resource(hash) => format!("client/resources/{hash}"),
            Self::Complete => "client/initialization/complete".into(),
        }
    }
}
#[derive(Clone, Copy)]
enum RetryPolicy {
    Never,
    Capacity,
}
struct Download {
    bytes: Vec<u8>,
    etag: Option<String>,
    content_type: Option<String>,
}
/// Original bytes are retained because integrity is defined over bytes, not reserialization.
pub(super) struct InitializationDownload {
    pub bytes: Vec<u8>,
    pub manifest: PhotoInitialization,
}
pub(super) struct PhotoEnrollmentClient {
    http: Client,
    origin: ServerOrigin,
}
impl PhotoEnrollmentClient {
    /// Reads an optional CA bundle and builds TLS; called on a blocking thread.
    pub fn load_tls(origin: ServerOrigin, ca_certificate: Option<PathBuf>) -> Result<Self> {
        let mut builder = Client::builder()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(30));
        if let Some(path) = ca_certificate {
            let bytes = std::fs::read(path).context(IoSnafu)?;
            let certificates =
                reqwest::Certificate::from_pem_bundle(&bytes).context(TransportSnafu)?;
            ensure!(!certificates.is_empty(), super::EmptyCaSnafu);
            for certificate in certificates {
                builder = builder.add_root_certificate(certificate);
            }
        }
        Ok(Self {
            http: builder.build().context(TransportSnafu)?,
            origin,
        })
    }
    fn photo_request(&self, endpoint: Endpoint<'_>) -> reqwest::RequestBuilder {
        let method = if matches!(endpoint, Endpoint::Resource(_)) {
            Method::GET
        } else {
            Method::POST
        };
        self.http.request(
            method,
            format!("{}/api/v2/photo/{}", self.origin, endpoint.path()),
        )
    }
    async fn send_bounded(
        request: reqwest::RequestBuilder,
        expected: StatusCode,
        limit: usize,
        retry: RetryPolicy,
    ) -> Result<Download> {
        let retryable = request.try_clone();
        let mut response = request.send().await.context(TransportSnafu)?;
        if let (RetryPolicy::Capacity, Some(request)) = (retry, retryable) {
            for attempt in 0..4_u32 {
                if response.status() != StatusCode::SERVICE_UNAVAILABLE {
                    break;
                }
                let Some(delay) = response
                    .headers()
                    .get(header::RETRY_AFTER)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok())
                else {
                    break;
                };
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
            response.status() == expected,
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
        let etag = response
            .headers()
            .get(header::ETAG)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.context(TransportSnafu)? {
            ensure!(
                chunk.len() <= limit.saturating_sub(bytes.len()),
                super::LimitSnafu { limit }
            );
            bytes.extend_from_slice(&chunk);
        }
        Ok(Download {
            bytes,
            etag,
            content_type,
        })
    }
    async fn send_json<T: DeserializeOwned>(
        request: reqwest::RequestBuilder,
        status: StatusCode,
        limit: usize,
    ) -> Result<T> {
        let download = Self::send_bounded(request, status, limit, RetryPolicy::Never).await?;
        serde_json::from_slice(&download.bytes).map_err(super::json_error)
    }
    pub async fn pair(
        &self,
        code: &str,
        public_key: &VerifyingKey,
    ) -> Result<PhotoEnrollmentIssued> {
        let body = PhotoPairRequest {
            code: code.to_owned(),
            signing_public_key: hex::encode(public_key.as_bytes())
                .parse()
                .context(super::ValueSnafu)?,
            client_version: env!("CARGO_PKG_VERSION").to_owned(),
        };
        let issued: PhotoEnrollmentIssued = Self::send_json(
            self.photo_request(Endpoint::Pair).json(&body),
            StatusCode::CREATED,
            natsume_photo_protocol::ENROLLMENT_RESPONSE_LIMIT,
        )
        .await?;
        issued.validate().context(super::ContractSnafu)?;
        Ok(issued)
    }
    pub async fn confirm(
        &self,
        enrollment: &PhotoEnrollmentIssued,
        public_key: &VerifyingKey,
    ) -> Result<()> {
        let body = PhotoConfirmation {
            enrollment_id: enrollment.enrollment_id.clone(),
            signing_public_key: hex::encode(public_key.as_bytes())
                .parse()
                .context(super::ValueSnafu)?,
        };
        let source: PhotoSource = Self::send_json(
            self.photo_request(Endpoint::Confirm)
                .bearer_auth(&enrollment.token)
                .json(&body),
            StatusCode::OK,
            natsume_photo_protocol::ENROLLMENT_RESPONSE_LIMIT,
        )
        .await?;
        ensure!(
            source.source_id == enrollment.source_id
                && source.state == SourceState::Enabled
                && source.enrollment_confirmed
                && !source.initialization_completed,
            super::ConfirmationSnafu
        );
        Ok(())
    }
    pub async fn prepare_initialization(
        &self,
        enrollment: &PhotoEnrollmentIssued,
    ) -> Result<InitializationDownload> {
        let download = Self::send_bounded(
            self.photo_request(Endpoint::PrepareInitialization)
                .bearer_auth(&enrollment.token),
            StatusCode::OK,
            natsume_photo_protocol::MANIFEST_LIMIT,
            RetryPolicy::Capacity,
        )
        .await?;
        ensure!(
            download.etag.as_deref()
                == Some(format!("\"{}\"", Sha256Digest::of(&download.bytes)).as_str()),
            super::IntegritySnafu
        );
        let manifest: PhotoInitialization =
            serde_json::from_slice(&download.bytes).map_err(super::json_error)?;
        manifest
            .validate(&enrollment.source_id)
            .context(super::ContractSnafu)?;
        Ok(InitializationDownload {
            bytes: download.bytes,
            manifest,
        })
    }
    pub async fn resource(
        &self,
        enrollment: &PhotoEnrollmentIssued,
        logo: &PhotoLogo,
    ) -> Result<Vec<u8>> {
        let download = Self::send_bounded(
            self.photo_request(Endpoint::Resource(&logo.sha256))
                .bearer_auth(&enrollment.token),
            StatusCode::OK,
            natsume_photo_protocol::IMAGE_LIMIT,
            RetryPolicy::Capacity,
        )
        .await?;
        ensure!(
            download.content_type.as_deref() == Some(logo.content_type.as_str())
                && download.bytes.starts_with(b"\x89PNG\r\n\x1a\n")
                && download.bytes.len() as u64 == logo.size_bytes
                && Sha256Digest::of(&download.bytes) == logo.sha256,
            super::IntegritySnafu
        );
        Ok(download.bytes)
    }
    pub async fn complete(
        &self,
        enrollment: &PhotoEnrollmentIssued,
        receipt: &PhotoInitializationComplete,
    ) -> Result<()> {
        let mut response = self
            .photo_request(Endpoint::Complete)
            .bearer_auth(&enrollment.token)
            .json(receipt)
            .send()
            .await
            .context(TransportSnafu)?;
        ensure!(
            response.status() == StatusCode::NO_CONTENT,
            RejectedSnafu {
                status: response.status().as_u16()
            }
        );
        ensure!(
            response.chunk().await.context(TransportSnafu)?.is_none(),
            super::UnexpectedBodySnafu
        );
        Ok(())
    }
}
