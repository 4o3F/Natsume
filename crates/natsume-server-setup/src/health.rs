//! Bounded HTTPS health checks pinned to the deployment's sole Control trust anchor.

use crate::pki::Certificate;
use reqwest::{Client, StatusCode, redirect::Policy};
use rustls::{ClientConfig, RootCertStore, crypto::ring, pki_types::CertificateDer};
use serde::Deserialize;
use snafu::{OptionExt as _, ResultExt as _, Snafu, ensure};
use std::{net::SocketAddr, time::Duration};

/// Bounds both TCP connection establishment and the complete health request.
pub const HEALTH_TIMEOUT: Duration = Duration::from_secs(5);
/// Largest accepted response body, including JSON whitespace.
pub const MAX_HEALTH_BODY: usize = 4096;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HealthResponse {
    status: String,
}

fn client(control_ca: &Certificate) -> Result<Client, HealthError> {
    let mut roots = RootCertStore::empty();
    roots
        .add(CertificateDer::from(control_ca.der()))
        .context(TlsSnafu)?;
    let mut tls = ClientConfig::builder_with_provider(ring::default_provider().into())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .context(TlsSnafu)?
        .with_root_certificates(roots)
        .with_no_client_auth();
    tls.alpn_protocols = vec![b"http/1.1".to_vec()];
    Client::builder()
        .tls_backend_preconfigured(tls)
        .http1_only()
        .no_proxy()
        .redirect(Policy::none())
        .connect_timeout(HEALTH_TIMEOUT)
        .timeout(HEALTH_TIMEOUT)
        .build()
        .context(RequestSnafu)
}

/// Accepts only a pinned TLS 1.3 response with HTTP 200 and exactly `{"status":"ok"}`.
///
/// # Errors
/// Returns TLS, transport, status, body-size, JSON, or health-status rejection errors.
pub async fn check_health(
    control_ca: &Certificate,
    address: SocketAddr,
) -> Result<(), HealthError> {
    let mut response = client(control_ca)?
        .get(format!("https://{address}/api/v2/health"))
        .send()
        .await
        .context(RequestSnafu)?;
    ensure!(
        response.status() == StatusCode::OK,
        StatusSnafu {
            status: response.status()
        }
    );
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.context(RequestSnafu)? {
        ensure!(
            chunk.len() <= MAX_HEALTH_BODY - body.len(),
            BodyTooLargeSnafu
        );
        body.extend_from_slice(&chunk);
    }
    let health: HealthResponse = serde_json::from_slice(&body).context(JsonSnafu)?;
    ensure!(health.status == "ok", UnhealthySnafu);
    Ok(())
}

/// Retries startup health checks, preserving the final concrete failure.
///
/// # Errors
/// Rejects zero attempts; otherwise returns the last health error after exhausting attempts.
pub async fn wait_for_health(
    control_ca: &Certificate,
    address: SocketAddr,
    attempts: usize,
    interval: Duration,
    interrupted: impl Fn() -> bool,
) -> Result<(), HealthError> {
    let mut last = None;
    for attempt in 0..attempts {
        ensure!(!interrupted(), InterruptedSnafu);
        let result = check_health(control_ca, address).await;
        ensure!(!interrupted(), InterruptedSnafu);
        match result {
            Ok(()) => return Ok(()),
            Err(error) => last = Some(error),
        }
        if attempt + 1 < attempts {
            tokio::time::sleep(interval).await;
            ensure!(!interrupted(), InterruptedSnafu);
        }
    }
    Err(last.context(NoAttemptsSnafu)?)
}

/// Health check failure without recording response bodies or private material.
#[derive(Debug, Snafu)]
pub enum HealthError {
    /// Installation cancellation stopped the retry loop.
    #[snafu(display("health check interrupted"))]
    Interrupted,
    /// The sole trust anchor or TLS protocol configuration was rejected.
    #[snafu(display("health TLS configuration failed"))]
    Tls {
        /// Original rustls cause.
        source: rustls::Error,
    },
    /// The HTTPS exchange or client construction failed.
    #[snafu(display("health HTTPS request failed"))]
    Request {
        /// Original transport cause, without response contents.
        source: reqwest::Error,
    },
    /// HTTP status did not indicate success, including all redirects.
    #[snafu(display("health endpoint returned HTTP {status}"))]
    Status {
        /// Rejected HTTP status.
        status: StatusCode,
    },
    /// The response exceeded the fixed memory budget.
    #[snafu(display("health response exceeds 4 KiB"))]
    BodyTooLarge,
    /// The body did not match the exact health response schema.
    #[snafu(display("invalid health response JSON"))]
    Json {
        /// Original schema or JSON decoding failure.
        source: serde_json::Error,
    },
    /// Server reported a status other than ok.
    #[snafu(display("Server health status is not ok"))]
    Unhealthy,
    /// The caller supplied no opportunity to check health.
    #[snafu(display("health checks require at least one attempt"))]
    NoAttempts,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pki::{
        generate::{self, GenerateError},
        import::ImportError,
    };
    use rustls::{
        ServerConfig,
        pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer},
    };
    use std::sync::Arc;
    use tokio::{
        io::{AsyncReadExt as _, AsyncWriteExt as _},
        net::TcpListener,
        task::JoinHandle,
    };
    use tokio_rustls::TlsAcceptor;

    #[derive(Debug, Snafu)]
    enum TestError {
        #[snafu(context(false), display("health fixture I/O failed"))]
        Io { source: std::io::Error },
        #[snafu(context(false), display("health fixture TLS failed"))]
        Tls { source: rustls::Error },
        #[snafu(context(false), display("health fixture PKI generation failed"))]
        Generate { source: GenerateError },
        #[snafu(context(false), display("health fixture PKI import failed"))]
        Import { source: ImportError },
        #[snafu(context(false), display("health fixture task failed"))]
        Join { source: tokio::task::JoinError },
        #[snafu(context(false), display("health check failed"))]
        Health { source: HealthError },
    }

    async fn server(
        status: &str,
        body: &str,
        requests: usize,
    ) -> Result<(Certificate, SocketAddr, JoinHandle<Result<(), TestError>>), TestError> {
        let now = time::OffsetDateTime::now_utc();
        let (ca, ca_key) = generate::generate_ca("Health CA", "Natsume", now)?;
        let (leaf, key) = generate::issue_server_leaf(
            &ca,
            &ca_key,
            &[std::net::Ipv4Addr::LOCALHOST.into()],
            now + time::Duration::days(1),
            now,
        )?;
        let mut config = ServerConfig::builder_with_provider(ring::default_provider().into())
            .with_protocol_versions(&[&rustls::version::TLS13])?
            .with_no_client_auth()
            .with_single_cert(
                vec![CertificateDer::from(leaf.der().to_vec())],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.pkcs8_der().to_vec())),
            )?;
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nLocation: https://127.0.0.1:1/should-not-follow\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let task = tokio::spawn(async move {
            let acceptor = TlsAcceptor::from(Arc::new(config));
            for _ in 0..requests {
                let (socket, _) = listener.accept().await?;
                let Ok(mut tls) = acceptor.accept(socket).await else {
                    continue;
                };
                assert_eq!(
                    tls.get_ref().1.protocol_version(),
                    Some(rustls::ProtocolVersion::TLSv1_3)
                );
                assert_eq!(
                    tls.get_ref().1.alpn_protocol(),
                    Some(b"http/1.1".as_slice())
                );
                let mut request = [0; 2048];
                let count = tls.read(&mut request).await?;
                assert!(request[..count].starts_with(b"GET /api/v2/health HTTP/1.1\r\n"));
                tls.write_all(response.as_bytes()).await?;
                tls.shutdown().await?;
            }
            Ok(())
        });
        Ok((ca, address, task))
    }

    #[tokio::test]
    async fn accepts_exact_ok_and_retries() -> Result<(), TestError> {
        let (ca, address, task) = server("200 OK", r#"{"status":"ok"}"#, 1).await?;
        wait_for_health(&ca, address, 3, Duration::ZERO, || false).await?;
        task.await??;
        assert!(matches!(
            wait_for_health(&ca, address, 0, Duration::ZERO, || false).await,
            Err(HealthError::NoAttempts)
        ));
        Ok(())
    }

    #[tokio::test]
    async fn interruption_stops_health_retries() -> Result<(), TestError> {
        let (ca, address, task) = server("503 Unavailable", r#"{"status":"ok"}"#, 1).await?;
        let checks = std::cell::Cell::new(0);
        let result = wait_for_health(&ca, address, 20, Duration::ZERO, || {
            let count = checks.get();
            checks.set(count + 1);
            count >= 2
        })
        .await;
        assert!(matches!(result, Err(HealthError::Interrupted)));
        task.await??;
        Ok(())
    }

    #[tokio::test]
    async fn rejects_invalid_status_schema_and_oversized_body() -> Result<(), TestError> {
        for (status, body, expected) in [
            ("200 OK", r#"{"status":"failed"}"#.to_owned(), "unhealthy"),
            (
                "200 OK",
                r#"{"status":"ok","extra":true}"#.to_owned(),
                "json",
            ),
            ("200 OK", " ".repeat(MAX_HEALTH_BODY + 1), "size"),
            ("503 Unavailable", r#"{"status":"ok"}"#.to_owned(), "status"),
            ("302 Found", String::new(), "status"),
        ] {
            let (ca, address, task) = server(status, &body, 1).await?;
            let result = check_health(&ca, address).await;
            assert!(
                match expected {
                    "unhealthy" => matches!(result, Err(HealthError::Unhealthy)),
                    "json" => matches!(result, Err(HealthError::Json { .. })),
                    "size" => matches!(result, Err(HealthError::BodyTooLarge)),
                    _ => matches!(result, Err(HealthError::Status { .. })),
                },
                "unexpected health result: {result:?}"
            );
            task.await??;
        }
        Ok(())
    }

    #[tokio::test]
    async fn rejects_untrusted_ca_and_retains_last_retry_error() -> Result<(), TestError> {
        let (_, address, task) = server("200 OK", r#"{"status":"ok"}"#, 2).await?;
        let (other, _) =
            generate::generate_ca("Other", "Natsume", time::OffsetDateTime::now_utc())?;
        assert!(matches!(
            wait_for_health(&other, address, 2, Duration::ZERO, || false).await,
            Err(HealthError::Request { .. })
        ));
        task.await??;
        Ok(())
    }
}
