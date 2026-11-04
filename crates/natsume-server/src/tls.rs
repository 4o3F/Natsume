use std::{
    fs,
    net::{IpAddr, SocketAddr},
    os::unix::fs::PermissionsExt,
    path::Path,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};

use axum::{
    extract::connect_info::Connected,
    serve::{IncomingStream, Listener},
};
use rustls::sign::{CertifiedKey, SigningKey};
use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use snafu::Snafu;
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::{TcpListener, TcpStream},
    sync::{OwnedSemaphorePermit, Semaphore},
    task::JoinSet,
    time::{sleep, timeout},
};
use tokio_rustls::{TlsAcceptor, server::TlsStream};
use zeroize::Zeroize;

const ALPN_HTTP_1_1: &[u8] = b"http/1.1";
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_CONNECTIONS: usize = 1024;
const MAX_HANDSHAKES: usize = 64;
const ACCEPT_RETRY_DELAY: Duration = Duration::from_millis(50);
const PRIVATE_FILE_FORBIDDEN_BITS: u32 = 0o177;
const PRIVATE_DIRECTORY_FORBIDDEN_BITS: u32 = 0o077;

/// Socket-observed peer IP, available to HTTP and WebSocket handlers.
#[derive(Clone, Copy)]
pub(crate) struct PeerAddress(pub(crate) IpAddr);

impl Connected<IncomingStream<'_, TlsListener>> for PeerAddress {
    fn connect_info(stream: IncomingStream<'_, TlsListener>) -> Self {
        Self(stream.remote_addr().ip().to_canonical())
    }
}

impl Connected<SocketAddr> for PeerAddress {
    fn connect_info(address: SocketAddr) -> Self {
        Self(address.ip().to_canonical())
    }
}

/// A concrete TLS adapter for Axum's HTTP/1.1 server.
pub(crate) struct TlsListener {
    tcp_listener: TcpListener,
    tls_acceptor: TlsAcceptor,
    handshakes: JoinSet<Option<(TlsStream<AcceptedTcpStream>, SocketAddr)>>,
    connections: Arc<Semaphore>,
}

/// Keeps the connection slot through TLS, HTTP keep-alive and WebSocket upgrade.
pub(crate) struct AcceptedTcpStream {
    stream: TcpStream,
    _permit: OwnedSemaphorePermit,
}

impl AsyncRead for AcceptedTcpStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}

impl AsyncWrite for AcceptedTcpStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}

impl TlsListener {
    /// Loads and validates TLS identity material before binding the TCP socket.
    ///
    /// # Errors
    ///
    /// Returns a redacted [`TlsError`] when identity loading, TLS configuration,
    /// or TCP binding fails.
    pub(crate) async fn bind(
        address: SocketAddr,
        certificate_path: &Path,
        private_key_path: &Path,
    ) -> Result<Self, TlsError> {
        let server_config = load_server_config(certificate_path, private_key_path)?;
        let tcp_listener = TcpListener::bind(address)
            .await
            .map_err(|source| TlsError::Bind { source })?;
        Ok(Self {
            tcp_listener,
            tls_acceptor: TlsAcceptor::from(server_config),
            handshakes: JoinSet::new(),
            connections: Arc::new(Semaphore::new(MAX_CONNECTIONS)),
        })
    }
}

impl Listener for TlsListener {
    type Io = TlsStream<AcceptedTcpStream>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            let has_in_flight_handshakes = !self.handshakes.is_empty();
            tokio::select! {
                biased;
                completed = self.handshakes.join_next(), if has_in_flight_handshakes => {
                    if let Some(Ok(Some(connection))) = completed {
                        return connection;
                    }
                }
                accepted = self.tcp_listener.accept() => {
                    match accepted {
                        Ok((tcp_stream, remote_address)) => {
                            if let Err(error) = tcp_stream.set_nodelay(true) {
                                tracing::warn!(error = %snafu::Report::from_error(error), "TCP_NODELAY could not be enabled");
                            }
                            if self.handshakes.len() >= MAX_HANDSHAKES {
                                continue;
                            }
                            let Ok(permit) = Arc::clone(&self.connections).try_acquire_owned() else {
                                continue;
                            };
                            let tcp_stream = AcceptedTcpStream { stream: tcp_stream, _permit: permit };
                            let tls_acceptor = self.tls_acceptor.clone();
                            self.handshakes.spawn(async move {
                                match timeout(HANDSHAKE_TIMEOUT, tls_acceptor.accept(tcp_stream)).await {
                                    Ok(Ok(tls_stream)) => Some((tls_stream, remote_address)),
                                    Ok(Err(source)) => {
                                        tracing::warn!(error = %snafu::Report::from_error(TlsError::Handshake { source }), "TLS handshake rejected");
                                        None
                                    }
                                    Err(error) => {
                                        tracing::warn!(error = %snafu::Report::from_error(error), "TLS handshake timed out");
                                        None
                                    }
                                }
                            });
                        }
                        Err(error) => {
                            tracing::warn!(error = %snafu::Report::from_error(error), "TLS listener accept failed");
                            sleep(ACCEPT_RETRY_DELAY).await;
                        }
                    }
                }
            }
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        self.tcp_listener.local_addr()
    }
}

fn load_server_config(
    certificate_path: &Path,
    private_key_path: &Path,
) -> Result<Arc<rustls::ServerConfig>, TlsError> {
    let certificate = CertificateDer::from(read_private_file(certificate_path)?);
    let private_key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(read_private_file(
        private_key_path,
    )?));
    server_config_from_der(certificate, private_key)
}

pub(crate) fn server_config_from_der(
    certificate: CertificateDer<'static>,
    mut private_key: PrivateKeyDer<'static>,
) -> Result<Arc<rustls::ServerConfig>, TlsError> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = rustls::ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|source| TlsError::Identity { source })?
        .with_no_client_auth();
    let signing_key = load_signing_key(&mut private_key)?;
    let certified_key = CertifiedKey::new(vec![certificate], signing_key);
    certified_key
        .keys_match()
        .map_err(|source| TlsError::Identity { source })?;
    let resolver = rustls::sign::SingleCertAndKey::from(certified_key);
    let mut config = builder.with_cert_resolver(Arc::new(resolver));
    config.alpn_protocols = vec![ALPN_HTTP_1_1.to_vec()];
    config.max_early_data_size = 0;
    Ok(Arc::new(config))
}

fn load_signing_key(
    private_key: &mut PrivateKeyDer<'static>,
) -> Result<Arc<dyn SigningKey>, TlsError> {
    let signing_key = rustls::crypto::ring::sign::any_supported_type(private_key);
    private_key.zeroize();
    signing_key.map_err(|source| TlsError::Identity { source })
}

pub(crate) fn read_private_file(path: &Path) -> Result<Vec<u8>, TlsError> {
    let parent = path.parent().ok_or(TlsError::InvalidIdentity)?;
    let parent_metadata =
        fs::metadata(parent).map_err(|source| TlsError::ReadIdentity { source })?;
    if !parent_metadata.is_dir()
        || parent_metadata.permissions().mode() & PRIVATE_DIRECTORY_FORBIDDEN_BITS != 0
    {
        return Err(TlsError::InvalidIdentity);
    }
    let metadata = fs::metadata(path).map_err(|source| TlsError::ReadIdentity { source })?;
    if !metadata.is_file() {
        return Err(TlsError::InvalidIdentity);
    }
    if metadata.permissions().mode() & PRIVATE_FILE_FORBIDDEN_BITS != 0 {
        return Err(TlsError::InvalidIdentity);
    }
    fs::read(path).map_err(|source| TlsError::ReadIdentity { source })
}

/// Redacted TLS identity, configuration, or binding failure.
#[derive(Debug, Snafu)]
pub(crate) enum TlsError {
    #[snafu(display("the TLS identity is invalid"))]
    Identity { source: rustls::Error },
    #[snafu(display("the TLS identity is invalid"))]
    InvalidIdentity,
    #[snafu(display("the TLS identity is invalid"))]
    ReadIdentity { source: std::io::Error },
    #[snafu(display("the TLS listener could not bind"))]
    Bind { source: std::io::Error },
    #[snafu(display("the TLS handshake failed"))]
    Handshake { source: std::io::Error },
}

#[cfg(test)]
pub(crate) mod tests {
    impl
        axum::extract::connect_info::Connected<
            axum::serve::IncomingStream<'_, tokio::net::TcpListener>,
        > for super::PeerAddress
    {
        fn connect_info(stream: axum::serve::IncomingStream<'_, tokio::net::TcpListener>) -> Self {
            Self(stream.remote_addr().ip().to_canonical())
        }
    }
    use std::{
        fs::{self, OpenOptions},
        io::Write,
        net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener as StdTcpListener},
        os::unix::fs::{OpenOptionsExt, PermissionsExt},
        path::{Path, PathBuf},
        sync::Arc,
        time::Duration,
    };

    use axum::serve::Listener;
    use rcgen::{
        BasicConstraints, CertificateParams, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
        KeyUsagePurpose,
    };
    use rustls::{ClientConfig, ProtocolVersion, RootCertStore};
    use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName};
    use snafu::Snafu;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpStream,
        sync::oneshot,
        task::JoinHandle,
        time::timeout,
    };
    use tokio_rustls::{TlsConnector, client::TlsStream};
    use uuid::Uuid;
    use zeroize::Zeroize;

    use crate::{
        config::{ORIGIN_CA_CERTIFICATE_FILENAME, ORIGIN_CA_PRIVATE_KEY_FILENAME},
        http,
    };

    use super::{
        ALPN_HTTP_1_1, TlsError, TlsListener, load_server_config, load_signing_key,
        server_config_from_der,
    };

    #[derive(Debug, snafu::Snafu)]
    pub(crate) enum TestSupportError {
        #[snafu(context(false), display("TLS fixture operation failed"))]
        Source {
            #[snafu(source(from(crate::test_error::TestError, Box::new)))]
            source: Box<crate::test_error::TestError>,
        },
    }

    pub(crate) struct TestIdentity {
        directory: TestDirectory,
        certificate_path: PathBuf,
        private_key_path: PathBuf,
        ca_certificate: CertificateDer<'static>,
    }

    impl TestIdentity {
        pub(crate) fn new(ip_san: Ipv4Addr) -> Result<Self, TestSupportError> {
            let directory = TestDirectory::new()?;

            let mut ca_params = CertificateParams::new(Vec::<String>::new()).map_err(|source| {
                TestSupportError::from(crate::test_error::TestError::from(source))
            })?;
            ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
            // Distinct issuer/leaf names let Windows build the test certificate chain.
            ca_params
                .distinguished_name
                .push(rcgen::DnType::CommonName, "Natsume test origin CA");
            ca_params.key_usages = vec![
                KeyUsagePurpose::DigitalSignature,
                KeyUsagePurpose::KeyCertSign,
                KeyUsagePurpose::CrlSign,
            ];
            let ca_key = KeyPair::generate().map_err(|source| {
                TestSupportError::from(crate::test_error::TestError::from(source))
            })?;
            let ca_certificate = ca_params.self_signed(&ca_key).map_err(|source| {
                TestSupportError::from(crate::test_error::TestError::from(source))
            })?;
            install_der(
                &directory.path.join(ORIGIN_CA_CERTIFICATE_FILENAME),
                ca_certificate.der().as_ref(),
            )?;
            install_certificate_pem(
                &directory.path.join("local-origin-ca.crt"),
                ca_certificate.der().as_ref(),
            )?;
            let mut origin_private_key_der = ca_key.serialize_der();
            let origin_key_result = install_der(
                &directory.path.join(ORIGIN_CA_PRIVATE_KEY_FILENAME),
                &origin_private_key_der,
            );
            origin_private_key_der.zeroize();
            origin_key_result?;
            let issuer = Issuer::new(ca_params, ca_key);

            let leaf_key = KeyPair::generate().map_err(|source| {
                TestSupportError::from(crate::test_error::TestError::from(source))
            })?;
            let mut leaf_params =
                CertificateParams::new(vec![ip_san.to_string()]).map_err(|source| {
                    TestSupportError::from(crate::test_error::TestError::from(source))
                })?;
            leaf_params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
            leaf_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
            let leaf_certificate = leaf_params
                .signed_by(&leaf_key, &issuer)
                .map_err(|source| {
                    TestSupportError::from(crate::test_error::TestError::from(source))
                })?;

            let certificate_path = directory.path.join("server-tls-leaf.der");
            let private_key_path = directory.path.join("server-tls-key.pk8");
            install_der(&certificate_path, leaf_certificate.der().as_ref())?;
            let mut private_key_der = leaf_key.serialize_der();
            let key_result = install_der(&private_key_path, &private_key_der);
            private_key_der.zeroize();
            key_result?;

            Ok(Self {
                directory,
                certificate_path,
                private_key_path,
                ca_certificate: ca_certificate.der().clone(),
            })
        }

        pub(crate) fn certificate_path(&self) -> &Path {
            &self.certificate_path
        }

        pub(crate) fn private_key_path(&self) -> &Path {
            &self.private_key_path
        }

        pub(crate) fn ca_certificate(&self) -> &CertificateDer<'static> {
            &self.ca_certificate
        }

        pub(crate) fn directory_path(&self) -> &Path {
            &self.directory.path
        }

        pub(crate) fn replace_private_key_from(
            &self,
            other: &Self,
        ) -> Result<(), TestSupportError> {
            fs::copy(&other.private_key_path, &self.private_key_path)
                .map(|_| ())
                .map_err(|source| {
                    TestSupportError::from(crate::test_error::TestError::from(source))
                })
        }
    }

    fn install_der(path: &Path, contents: &[u8]) -> Result<(), TestSupportError> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .map_err(|source| TestSupportError::from(crate::test_error::TestError::from(source)))?;
        file.write_all(contents)
            .map_err(|source| TestSupportError::from(crate::test_error::TestError::from(source)))
    }

    fn install_certificate_pem(path: &Path, der: &[u8]) -> Result<(), TestSupportError> {
        let base64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, der);
        let mut pem = String::from("-----BEGIN CERTIFICATE-----\n");
        for line in base64.as_bytes().chunks(64) {
            pem.push_str(std::str::from_utf8(line).map_err(|source| {
                TestSupportError::from(crate::test_error::TestError::from(source))
            })?);
            pem.push('\n');
        }
        pem.push_str("-----END CERTIFICATE-----\n");
        install_der(path, pem.as_bytes())
    }

    struct TestDirectory {
        path: PathBuf,
    }

    impl TestDirectory {
        fn new() -> Result<Self, TestSupportError> {
            let path = std::env::temp_dir()
                .join(format!("natsume-server-tls-path-canary-{}", Uuid::now_v7()));
            fs::create_dir(&path).map_err(|source| {
                TestSupportError::from(crate::test_error::TestError::from(source))
            })?;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).map_err(|source| {
                TestSupportError::from(crate::test_error::TestError::from(source))
            })?;
            Ok(Self { path })
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _cleanup_result = fs::remove_dir_all(&self.path);
        }
    }

    const LOCALHOST: Ipv4Addr = Ipv4Addr::LOCALHOST;
    const TEST_TIMEOUT: Duration = Duration::from_secs(2);
    const HEALTH_REQUEST: &[u8] =
        b"GET /api/v2/health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n";

    #[tokio::test]
    async fn accepted_connection_has_nodelay() -> Result<(), TestFailure> {
        let identity = TestIdentity::new(LOCALHOST)
            .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?;
        let mut listener = bind_identity(&identity).await?;
        let address = Listener::local_addr(&listener)
            .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?;
        let versions = [&rustls::version::TLS13];
        let client = connect_client(
            address,
            identity.ca_certificate(),
            IpAddr::V4(LOCALHOST),
            &versions,
            vec![b"http/1.1".to_vec()],
        );
        let (accepted, client) = timeout(TEST_TIMEOUT, async {
            tokio::join!(Listener::accept(&mut listener), client)
        })
        .await
        .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?;
        let _client = client?;
        let enabled = accepted
            .0
            .get_ref()
            .0
            .stream
            .nodelay()
            .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?;
        assert!(enabled, "accepted server socket must disable Nagle");
        Ok(())
    }

    #[tokio::test]
    async fn health_uses_http1_tls13_and_shuts_down_gracefully() -> Result<(), TestFailure> {
        let identity = TestIdentity::new(LOCALHOST)
            .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?;
        let listener = bind_identity(&identity).await?;
        let address = Listener::local_addr(&listener)
            .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?;
        let (shutdown, server) = spawn_server(listener);

        let mut tls_stream = connect_client(
            address,
            identity.ca_certificate(),
            IpAddr::V4(LOCALHOST),
            &[&rustls::version::TLS13],
            vec![b"h2".to_vec(), b"http/1.1".to_vec()],
        )
        .await?;
        if tls_stream.get_ref().1.alpn_protocol() != Some(b"http/1.1".as_slice()) {
            return Err(TestFailure::UnexpectedAlpn);
        }
        if tls_stream.get_ref().1.protocol_version() != Some(ProtocolVersion::TLSv1_3) {
            return Err(TestFailure::UnexpectedProtocolVersion);
        }
        let response = request_health(&mut tls_stream).await?;
        assert_health_response(&response)?;
        shutdown
            .send(())
            .map_err(|()| TestFailure::ShutdownSignalFailed)?;
        await_server(server).await
    }

    #[tokio::test]
    async fn tls12_only_client_is_rejected() -> Result<(), TestFailure> {
        let identity = TestIdentity::new(LOCALHOST)
            .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?;
        let listener = bind_identity(&identity).await?;
        let address = Listener::local_addr(&listener)
            .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?;
        let (shutdown, server) = spawn_server(listener);

        let result = connect_client(
            address,
            identity.ca_certificate(),
            IpAddr::V4(LOCALHOST),
            &[&rustls::version::TLS12],
            vec![b"http/1.1".to_vec()],
        )
        .await;
        if result.is_ok() {
            return Err(TestFailure::HandshakeShouldHaveFailed);
        }
        shutdown
            .send(())
            .map_err(|()| TestFailure::ShutdownSignalFailed)?;
        await_server(server).await
    }

    #[tokio::test]
    async fn wrong_trust_root_is_rejected() -> Result<(), TestFailure> {
        let identity = TestIdentity::new(LOCALHOST)
            .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?;
        let wrong_identity = TestIdentity::new(LOCALHOST)
            .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?;
        let listener = bind_identity(&identity).await?;
        let address = Listener::local_addr(&listener)
            .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?;
        let (shutdown, server) = spawn_server(listener);

        let result = connect_client(
            address,
            wrong_identity.ca_certificate(),
            IpAddr::V4(LOCALHOST),
            &[&rustls::version::TLS13],
            vec![b"http/1.1".to_vec()],
        )
        .await;
        if result.is_ok() {
            return Err(TestFailure::HandshakeShouldHaveFailed);
        }
        shutdown
            .send(())
            .map_err(|()| TestFailure::ShutdownSignalFailed)?;
        await_server(server).await
    }

    #[tokio::test]
    async fn wrong_ip_san_is_rejected() -> Result<(), TestFailure> {
        let identity = TestIdentity::new(LOCALHOST)
            .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?;
        let listener = bind_identity(&identity).await?;
        let address = Listener::local_addr(&listener)
            .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?;
        let (shutdown, server) = spawn_server(listener);

        let result = connect_client(
            address,
            identity.ca_certificate(),
            IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2)),
            &[&rustls::version::TLS13],
            vec![b"http/1.1".to_vec()],
        )
        .await;
        if result.is_ok() {
            return Err(TestFailure::HandshakeShouldHaveFailed);
        }
        shutdown
            .send(())
            .map_err(|()| TestFailure::ShutdownSignalFailed)?;
        await_server(server).await
    }

    #[tokio::test]
    async fn mismatched_identity_fails_before_bind() -> Result<(), TestFailure> {
        let identity = TestIdentity::new(LOCALHOST)
            .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?;
        let other_identity = TestIdentity::new(LOCALHOST)
            .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?;
        identity
            .replace_private_key_from(&other_identity)
            .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?;
        let occupied = StdTcpListener::bind(SocketAddr::from((LOCALHOST, 0)))
            .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?;
        let address = occupied
            .local_addr()
            .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?;

        let error = expect_bind_error(&identity, address).await?;
        if !matches!(error, TlsError::Identity { .. }) {
            return Err(TestFailure::UnexpectedTlsError);
        }
        drop(occupied);
        Ok(())
    }

    #[tokio::test]
    async fn malformed_der_error_is_redacted() -> Result<(), TestFailure> {
        let identity = TestIdentity::new(LOCALHOST)
            .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?;
        fs::write(identity.certificate_path(), b"malformed-der-content-canary")
            .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?;
        let error = expect_bind_error(&identity, SocketAddr::from((LOCALHOST, 0))).await?;
        if !matches!(error, TlsError::Identity { .. }) {
            return Err(TestFailure::UnexpectedTlsError);
        }
        let display = error.to_string();
        let debug = format!("{error:?}");
        if display.contains("malformed-der-content-canary")
            || debug.contains("malformed-der-content-canary")
            || display.contains("tls-path-canary")
            || debug.contains("tls-path-canary")
        {
            return Err(TestFailure::TlsErrorWasNotRedacted);
        }
        Ok(())
    }

    #[tokio::test]
    async fn material_permissions_fail_closed() -> Result<(), TestFailure> {
        let certificate_wide = TestIdentity::new(LOCALHOST)
            .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?;
        fs::set_permissions(
            certificate_wide.certificate_path(),
            fs::Permissions::from_mode(0o644),
        )
        .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?;
        if !matches!(
            expect_bind_error(&certificate_wide, SocketAddr::from((LOCALHOST, 0))).await?,
            TlsError::InvalidIdentity
        ) {
            return Err(TestFailure::UnexpectedTlsError);
        }

        let key_wide = TestIdentity::new(LOCALHOST)
            .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?;
        fs::set_permissions(
            key_wide.private_key_path(),
            fs::Permissions::from_mode(0o644),
        )
        .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?;
        if !matches!(
            expect_bind_error(&key_wide, SocketAddr::from((LOCALHOST, 0))).await?,
            TlsError::InvalidIdentity
        ) {
            return Err(TestFailure::UnexpectedTlsError);
        }

        let directory_wide = TestIdentity::new(LOCALHOST)
            .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?;
        fs::set_permissions(
            directory_wide.directory_path(),
            fs::Permissions::from_mode(0o755),
        )
        .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?;
        if !matches!(
            expect_bind_error(&directory_wide, SocketAddr::from((LOCALHOST, 0))).await?,
            TlsError::InvalidIdentity
        ) {
            return Err(TestFailure::UnexpectedTlsError);
        }
        Ok(())
    }

    #[test]
    fn tls_policy_is_exact() -> Result<(), TestFailure> {
        let identity = TestIdentity::new(LOCALHOST)
            .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?;
        let config =
            load_server_config(identity.certificate_path(), identity.private_key_path())
                .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?;
        if config.alpn_protocols != vec![ALPN_HTTP_1_1.to_vec()] || config.max_early_data_size != 0
        {
            return Err(TestFailure::TlsPolicyWasNotExact);
        }
        Ok(())
    }

    #[test]
    fn server_config_from_der_accepts_matching_identity() -> Result<(), crate::test_error::TestError>
    {
        let identity = TestIdentity::new(LOCALHOST)?;
        let config = server_config_from_der(
            CertificateDer::from(fs::read(identity.certificate_path())?),
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(fs::read(
                identity.private_key_path(),
            )?)),
        )?;
        assert_eq!(
            config.alpn_protocols,
            vec![ALPN_HTTP_1_1.to_vec()],
            "DER configuration must preserve HTTP/1.1 ALPN"
        );
        assert_eq!(
            config.max_early_data_size, 0,
            "DER configuration must disable early data"
        );
        Ok(())
    }

    #[test]
    fn server_config_from_der_rejects_mismatched_key() -> Result<(), crate::test_error::TestError> {
        let identity = TestIdentity::new(LOCALHOST)?;
        let other_identity = TestIdentity::new(LOCALHOST)?;
        let result = server_config_from_der(
            CertificateDer::from(fs::read(identity.certificate_path())?),
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(fs::read(
                other_identity.private_key_path(),
            )?)),
        );
        assert!(
            matches!(result, Err(TlsError::Identity { .. })),
            "DER configuration must reject a key that does not match the certificate"
        );
        Ok(())
    }

    #[test]
    fn signing_key_der_is_zeroed_before_return() -> Result<(), TestFailure> {
        let identity = TestIdentity::new(LOCALHOST)
            .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?;
        let mut private_key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
            fs::read(identity.private_key_path())
                .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?,
        ));
        load_signing_key(&mut private_key)
            .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?;
        if private_key.secret_der().iter().any(|byte| *byte != 0) {
            return Err(TestFailure::PrivateKeyDerWasNotZeroed);
        }
        Ok(())
    }

    #[tokio::test]
    async fn stalled_handshake_does_not_serialize_acceptance() -> Result<(), TestFailure> {
        let identity = TestIdentity::new(LOCALHOST)
            .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?;
        let listener = bind_identity(&identity).await?;
        let address = Listener::local_addr(&listener)
            .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?;
        let stalled_socket = TcpStream::connect(address)
            .await
            .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?;
        let (shutdown, server) = spawn_server(listener);

        let request = async {
            let mut tls_stream = connect_client(
                address,
                identity.ca_certificate(),
                IpAddr::V4(LOCALHOST),
                &[&rustls::version::TLS13],
                vec![b"http/1.1".to_vec()],
            )
            .await?;
            let response = request_health(&mut tls_stream).await?;
            assert_health_response(&response)
        };
        timeout(TEST_TIMEOUT, request)
            .await
            .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))??;
        shutdown
            .send(())
            .map_err(|()| TestFailure::ShutdownSignalFailed)?;
        drop(stalled_socket);
        await_server(server).await
    }

    #[tokio::test]
    async fn shutdown_aborts_stalled_handshake() -> Result<(), TestFailure> {
        let identity = TestIdentity::new(LOCALHOST)
            .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?;
        let listener = bind_identity(&identity).await?;
        let address = Listener::local_addr(&listener)
            .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?;
        let stalled_socket = TcpStream::connect(address)
            .await
            .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?;
        let (shutdown, server) = spawn_server(listener);

        shutdown
            .send(())
            .map_err(|()| TestFailure::ShutdownSignalFailed)?;
        await_server(server).await?;
        drop(stalled_socket);
        Ok(())
    }

    #[tokio::test]
    async fn stalled_handshake_closes_after_fixed_timeout() -> Result<(), TestFailure> {
        let identity = TestIdentity::new(LOCALHOST)
            .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?;
        let listener = bind_identity(&identity).await?;
        let address = Listener::local_addr(&listener)
            .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?;
        let mut stalled_socket = TcpStream::connect(address)
            .await
            .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?;
        let (shutdown, server) = spawn_server(listener);

        let mut byte = [0_u8; 1];
        match timeout(Duration::from_secs(7), stalled_socket.read(&mut byte)).await {
            Ok(Ok(0) | Err(_)) => {}
            Ok(Ok(_)) | Err(_) => return Err(TestFailure::HandshakeTimeoutWasNotEnforced),
        }
        shutdown
            .send(())
            .map_err(|()| TestFailure::ShutdownSignalFailed)?;
        await_server(server).await
    }

    #[tokio::test]
    async fn connection_capacity_covers_idle_tls_and_websocket_upgrade()
    -> Result<(), crate::test_error::TestError> {
        use axum::extract::{ConnectInfo, ws::WebSocketUpgrade};
        use futures_util::{SinkExt as _, StreamExt as _};
        let identity = TestIdentity::new(LOCALHOST).map_err(crate::test_error::TestError::from)?;
        let mut listener = bind_identity(&identity).await?;
        listener.connections = Arc::new(tokio::sync::Semaphore::new(1));
        let capacity = Arc::clone(&listener.connections);
        let address = listener.local_addr()?;
        let application =
            axum::Router::new().route(
                "/echo",
                axum::routing::get(
                    |ConnectInfo(peer): ConnectInfo<super::PeerAddress>,
                     upgrade: WebSocketUpgrade| async move {
                        assert_eq!(peer.0, IpAddr::V4(LOCALHOST));
                        upgrade.on_upgrade(|mut socket| async move {
                            while let Some(Ok(message)) = socket.recv().await {
                                if socket.send(message).await.is_err() {
                                    break;
                                }
                            }
                        })
                    },
                ),
            );
        let server = tokio::spawn(crate::server_runtime::serve_http(
            listener,
            application,
            std::future::pending(),
        ));
        let tls = connect_client(
            address,
            identity.ca_certificate(),
            IpAddr::V4(LOCALHOST),
            &[&rustls::version::TLS13],
            vec![ALPN_HTTP_1_1.to_vec()],
        )
        .await?;
        assert_eq!(capacity.available_permits(), 0);
        let mut excess = TcpStream::connect(address).await?;
        let mut byte = [0];
        assert!(matches!(
            timeout(TEST_TIMEOUT, excess.read(&mut byte)).await?,
            Ok(0) | Err(_)
        ));
        let (mut socket, response) =
            tokio_tungstenite::client_async(format!("wss://{address}/echo"), tls).await?;
        assert_eq!(response.status(), 101);
        assert_eq!(capacity.available_permits(), 0);
        elapse_http_time(crate::server_runtime::HTTP_HEADER_READ_TIMEOUT + Duration::from_secs(5))
            .await;
        assert_eq!(capacity.available_permits(), 0);
        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                "still active".into(),
            ))
            .await?;
        assert!(matches!(
            socket.next().await,
            Some(Ok(tokio_tungstenite::tungstenite::Message::Text(_)))
        ));
        drop(socket);
        timeout(TEST_TIMEOUT, async {
            while capacity.available_permits() != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        server.abort();
        Ok(())
    }

    #[tokio::test]
    async fn repeated_slow_handshake_overload_is_bounded_and_recovers()
    -> Result<(), crate::test_error::TestError> {
        let identity = TestIdentity::new(LOCALHOST).map_err(crate::test_error::TestError::from)?;
        let listener = bind_identity(&identity).await?;
        let capacity = Arc::clone(&listener.connections);
        let address = listener.local_addr()?;
        let (shutdown, server) = spawn_server(listener);
        let mut peak_fds = 0;
        for _ in 0..3 {
            let mut sockets = Vec::new();
            for _ in 0..super::MAX_HANDSHAKES {
                sockets.push(TcpStream::connect(address).await?);
            }
            timeout(TEST_TIMEOUT, async {
                while capacity.available_permits() != super::MAX_CONNECTIONS - super::MAX_HANDSHAKES
                {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .map_err(crate::test_error::TestError::from)?;
            // Bound client concurrency while avoiding serial socket latency
            // consuming the stalled handshakes' five-second lifetime.
            for _ in 0..16 {
                let mut rejections = tokio::task::JoinSet::new();
                for _ in 0..32 {
                    rejections.spawn(async move {
                        let mut excess = TcpStream::connect(address).await?;
                        let mut byte = [0];
                        timeout(TEST_TIMEOUT, excess.read(&mut byte))
                            .await
                            .map_err(std::io::Error::other)
                    });
                }
                while let Some(result) = rejections.join_next().await {
                    assert!(matches!(result??, Ok(0) | Err(_)));
                    assert_eq!(
                        capacity.available_permits(),
                        super::MAX_CONNECTIONS - super::MAX_HANDSHAKES
                    );
                }
            }
            peak_fds = peak_fds.max(fs::read_dir("/proc/self/fd")?.count());
            // Exercise expiry, not just voluntary client disconnects.
            for socket in &mut sockets {
                assert!(matches!(socket.read(&mut [0]).await, Ok(0) | Err(_)));
            }
            drop(sockets);
            timeout(TEST_TIMEOUT, async {
                while capacity.available_permits() != super::MAX_CONNECTIONS {
                    tokio::task::yield_now().await;
                }
            })
            .await?;
            let started = std::time::Instant::now();
            let mut tls = connect_client(
                address,
                identity.ca_certificate(),
                IpAddr::V4(LOCALHOST),
                &[&rustls::version::TLS13],
                vec![ALPN_HTTP_1_1.to_vec()],
            )
            .await?;
            let response = request_health(&mut tls).await?;
            assert!(response.starts_with(b"HTTP/1.1 200"));
            tracing::info!(
                "TLS overload recovery health latency: {:?}",
                started.elapsed()
            );
        }
        tracing::info!(
            "TLS overload: 3 rounds, 64 stalled + 512 rejected connections per round; peak process FDs (including test clients): {peak_fds}"
        );
        for line in fs::read_to_string("/proc/self/status")?
            .lines()
            .filter(|line| line.starts_with("VmRSS:") || line.starts_with("VmHWM:"))
        {
            tracing::info!("{line}");
        }
        shutdown
            .send(())
            .map_err(crate::test_error::TestError::from)?;
        await_server(server).await?;
        Ok(())
    }

    struct HttpFixture {
        identity: TestIdentity,
        address: SocketAddr,
        capacity: Arc<tokio::sync::Semaphore>,
        shutdown: Option<oneshot::Sender<()>>,
        server: tokio_util::task::AbortOnDropHandle<std::io::Result<()>>,
    }

    impl HttpFixture {
        async fn start(
            permits: usize,
            router: axum::Router,
        ) -> Result<Self, crate::test_error::TestError> {
            let identity = TestIdentity::new(LOCALHOST)?;
            let mut listener = bind_identity(&identity).await?;
            listener.connections = Arc::new(tokio::sync::Semaphore::new(permits));
            let capacity = Arc::clone(&listener.connections);
            let address = listener.local_addr()?;
            let (shutdown, stopped) = oneshot::channel();
            let server = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(
                crate::server_runtime::serve_http(listener, router, async move {
                    let _shutdown_result = stopped.await;
                }),
            ));
            Ok(Self {
                identity,
                address,
                capacity,
                shutdown: Some(shutdown),
                server,
            })
        }

        async fn connect(&self) -> Result<TlsStream<TcpStream>, crate::test_error::TestError> {
            Ok(connect_client(
                self.address,
                self.identity.ca_certificate(),
                IpAddr::V4(LOCALHOST),
                &[&rustls::version::TLS13],
                vec![ALPN_HTTP_1_1.to_vec()],
            )
            .await?)
        }

        fn stop(&mut self) {
            if let Some(shutdown) = self.shutdown.take() {
                shutdown
                    .send(())
                    .unwrap_or_else(|()| panic!("HTTP test server stopped early"));
            }
        }

        async fn finish(mut self) -> Result<(), crate::test_error::TestError> {
            self.stop();
            timeout(TEST_TIMEOUT, self.server).await???;
            Ok(())
        }
    }

    // Resume before socket I/O so Tokio cannot auto-advance past another deadline
    // while the operating system is delivering loopback TLS records.
    async fn elapse_http_time(duration: Duration) {
        tokio::time::pause();
        tokio::time::advance(duration).await;
        tokio::time::resume();
        tokio::task::yield_now().await;
    }

    async fn wait_for_permits(
        capacity: &tokio::sync::Semaphore,
        expected: usize,
    ) -> Result<(), crate::test_error::TestError> {
        timeout(TEST_TIMEOUT, async {
            while capacity.available_permits() != expected {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        Ok(())
    }

    async fn read_head(
        stream: &mut TlsStream<TcpStream>,
    ) -> Result<String, crate::test_error::TestError> {
        let mut head = Vec::new();
        timeout(TEST_TIMEOUT, async {
            while !head.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                stream.read_exact(&mut byte).await?;
                head.push(byte[0]);
                assert!(
                    head.len() < 8192,
                    "HTTP test response head exceeded its bound"
                );
            }
            Ok::<_, std::io::Error>(())
        })
        .await??;
        Ok(std::str::from_utf8(&head)?.to_owned())
    }

    async fn read_response(
        stream: &mut TlsStream<TcpStream>,
    ) -> Result<Vec<u8>, crate::test_error::TestError> {
        let head = read_head(stream).await?;
        assert!(
            head.starts_with("HTTP/1.1 200 OK\r\n"),
            "unexpected response: {head}"
        );
        let length = head
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
            .unwrap_or_else(|| panic!("missing content length: {head}"));
        let mut body = vec![0; length];
        timeout(TEST_TIMEOUT, stream.read_exact(&mut body)).await??;
        Ok(body)
    }

    async fn assert_http_closed(
        client: &mut TlsStream<TcpStream>,
    ) -> Result<(), crate::test_error::TestError> {
        let mut remaining = Vec::new();
        let result = timeout(TEST_TIMEOUT, client.read_to_end(&mut remaining)).await?;
        assert!(
            result.is_ok()
                || result.as_ref().is_err_and(|error| matches!(
                    error.kind(),
                    std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::ConnectionReset
                )),
            "unexpected TLS closure result: {result:?}"
        );
        assert!(remaining.is_empty() || remaining.starts_with(b"HTTP/1.1 408"));
        Ok(())
    }

    async fn keep_alive_health(
        stream: &mut TlsStream<TcpStream>,
    ) -> Result<(), crate::test_error::TestError> {
        stream
            .write_all(b"GET /api/v2/health HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await?;
        assert_eq!(read_response(stream).await?, br#"{"status":"ok"}"#);
        Ok(())
    }

    #[tokio::test]
    async fn idle_http_keep_alive_closes_and_returns_all_permits()
    -> Result<(), crate::test_error::TestError> {
        let fixture =
            HttpFixture::start(super::MAX_CONNECTIONS, http::tests::health_router()).await?;
        let mut client = fixture.connect().await?;
        keep_alive_health(&mut client).await?;
        assert_eq!(
            fixture.capacity.available_permits(),
            super::MAX_CONNECTIONS - 1
        );
        elapse_http_time(Duration::from_secs(29)).await;
        assert_eq!(
            fixture.capacity.available_permits(),
            super::MAX_CONNECTIONS - 1
        );
        elapse_http_time(Duration::from_secs(2)).await;
        wait_for_permits(&fixture.capacity, super::MAX_CONNECTIONS).await?;
        assert_http_closed(&mut client).await?;
        fixture.finish().await
    }

    #[tokio::test]
    async fn repeated_http_requests_within_deadline_keep_the_same_connection()
    -> Result<(), crate::test_error::TestError> {
        use axum::extract::ConnectInfo;
        let router = http::tests::health_router().route(
            "/peer",
            axum::routing::get(
                |ConnectInfo(peer): ConnectInfo<super::PeerAddress>| async move {
                    assert_eq!(peer.0, IpAddr::V4(LOCALHOST));
                    "peer verified"
                },
            ),
        );
        let fixture = HttpFixture::start(1, router).await?;
        let mut client = fixture.connect().await?;
        keep_alive_health(&mut client).await?;
        for _ in 0..4 {
            elapse_http_time(Duration::from_secs(20)).await;
            client
                .write_all(b"GET /peer HTTP/1.1\r\nHost: localhost\r\n\r\n")
                .await?;
            assert_eq!(read_response(&mut client).await?, b"peer verified");
            assert_eq!(fixture.capacity.available_permits(), 0);
        }
        fixture.finish().await
    }

    #[tokio::test]
    async fn partial_next_request_head_does_not_extend_the_deadline()
    -> Result<(), crate::test_error::TestError> {
        let fixture = HttpFixture::start(1, http::tests::health_router()).await?;
        let mut client = fixture.connect().await?;
        keep_alive_health(&mut client).await?;
        client.write_all(b"G").await?;
        elapse_http_time(Duration::from_secs(20)).await;
        client.write_all(b"ET /api/v2/health HTTP/1.1\r\n").await?;
        elapse_http_time(Duration::from_secs(11)).await;
        wait_for_permits(&fixture.capacity, 1).await?;
        assert_http_closed(&mut client).await?;
        fixture.finish().await
    }

    #[tokio::test]
    async fn expired_idle_http_connections_restore_capacity_for_new_clients()
    -> Result<(), crate::test_error::TestError> {
        let fixture = HttpFixture::start(2, http::tests::health_router()).await?;
        let mut first = fixture.connect().await?;
        let mut second = fixture.connect().await?;
        keep_alive_health(&mut first).await?;
        keep_alive_health(&mut second).await?;
        assert_eq!(fixture.capacity.available_permits(), 0);
        let mut excess = TcpStream::connect(fixture.address).await?;
        assert!(matches!(
            timeout(TEST_TIMEOUT, excess.read(&mut [0])).await?,
            Ok(0) | Err(_)
        ));
        elapse_http_time(Duration::from_secs(31)).await;
        wait_for_permits(&fixture.capacity, 2).await?;
        let mut fresh = fixture.connect().await?;
        keep_alive_health(&mut fresh).await?;
        assert_eq!(fixture.capacity.available_permits(), 1);
        fixture.finish().await
    }

    #[tokio::test]
    async fn slow_upload_and_request_processing_outlive_the_header_deadline()
    -> Result<(), crate::test_error::TestError> {
        use futures_util::StreamExt as _;
        let (received, mut chunks) = tokio::sync::mpsc::channel(4);
        let release = Arc::new(tokio::sync::Notify::new());
        let handler_release = Arc::clone(&release);
        let router = axum::Router::new().route(
            "/upload",
            axum::routing::post(move |body: axum::body::Body| {
                let received = received.clone();
                let release = Arc::clone(&handler_release);
                async move {
                    let mut stream = body.into_data_stream();
                    let mut size = 0;
                    while let Some(chunk) = stream.next().await {
                        let chunk = chunk.unwrap_or_else(|error| panic!("upload failed: {error}"));
                        size += chunk.len();
                        received
                            .send(size)
                            .await
                            .unwrap_or_else(|error| panic!("upload receiver dropped: {error}"));
                    }
                    release.notified().await;
                    size.to_string()
                }
            }),
        );
        let fixture = HttpFixture::start(1, router).await?;
        let mut client = fixture.connect().await?;
        client
            .write_all(b"POST /upload HTTP/1.1\r\nHost: localhost\r\nContent-Length: 4\r\n\r\n")
            .await?;
        for size in 1..=4 {
            client.write_all(b"x").await?;
            assert_eq!(timeout(TEST_TIMEOUT, chunks.recv()).await?, Some(size));
            elapse_http_time(Duration::from_secs(35)).await;
            assert_eq!(fixture.capacity.available_permits(), 0);
        }
        release.notify_one();
        assert_eq!(read_response(&mut client).await?, b"4");
        fixture.finish().await
    }

    #[tokio::test]
    async fn slow_streaming_download_outlives_the_header_deadline()
    -> Result<(), crate::test_error::TestError> {
        let router = http::tests::health_router().route(
            "/download",
            axum::routing::get(|| async {
                axum::body::Body::from_stream(futures_util::stream::unfold(0, |index| async move {
                    if index == 3 {
                        return None;
                    }
                    if index > 0 {
                        tokio::time::sleep(Duration::from_secs(35)).await;
                    }
                    Some((
                        Ok::<_, std::convert::Infallible>(axum::body::Bytes::from_static(b"chunk")),
                        index + 1,
                    ))
                }))
            }),
        );
        let fixture = HttpFixture::start(1, router).await?;
        let mut client = fixture.connect().await?;
        client
            .write_all(b"GET /download HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await?;
        assert!(read_head(&mut client).await?.starts_with("HTTP/1.1 200 OK"));
        for index in 0..3 {
            if index > 0 {
                elapse_http_time(Duration::from_secs(35)).await;
            }
            let mut chunk = [0; 10];
            timeout(TEST_TIMEOUT, client.read_exact(&mut chunk)).await??;
            assert_eq!(&chunk, b"5\r\nchunk\r\n");
            assert_eq!(fixture.capacity.available_permits(), 0);
        }
        let mut end = [0; 5];
        timeout(TEST_TIMEOUT, client.read_exact(&mut end)).await??;
        assert_eq!(&end, b"0\r\n\r\n");
        keep_alive_health(&mut client).await?;
        fixture.finish().await
    }

    #[tokio::test]
    async fn http_shutdown_closes_idle_connections_and_drains_an_in_flight_request()
    -> Result<(), crate::test_error::TestError> {
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let handler_entered = Arc::clone(&entered);
        let handler_release = Arc::clone(&release);
        let router = http::tests::health_router().route(
            "/slow",
            axum::routing::get(move || {
                let entered = Arc::clone(&handler_entered);
                let release = Arc::clone(&handler_release);
                async move {
                    entered.notify_one();
                    release.notified().await;
                    "drained"
                }
            }),
        );
        let mut fixture = HttpFixture::start(2, router).await?;
        let mut idle = fixture.connect().await?;
        keep_alive_health(&mut idle).await?;
        let mut active = fixture.connect().await?;
        active
            .write_all(b"GET /slow HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await?;
        timeout(TEST_TIMEOUT, entered.notified()).await?;
        fixture.stop();
        wait_for_permits(&fixture.capacity, 1).await?;
        assert!(
            TcpStream::connect(fixture.address).await.is_err(),
            "listener remained open during HTTP drain"
        );
        assert!(
            !fixture.server.is_finished(),
            "HTTP shutdown abandoned the active request"
        );
        elapse_http_time(Duration::from_secs(35)).await;
        assert!(
            !fixture.server.is_finished(),
            "header timeout cut the active request during shutdown"
        );
        release.notify_one();
        assert_eq!(read_response(&mut active).await?, b"drained");
        wait_for_permits(&fixture.capacity, 2).await?;
        fixture.finish().await
    }

    async fn bind_identity(identity: &TestIdentity) -> Result<TlsListener, TestFailure> {
        TlsListener::bind(
            SocketAddr::from((LOCALHOST, 0)),
            identity.certificate_path(),
            identity.private_key_path(),
        )
        .await
        .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))
    }

    async fn expect_bind_error(
        identity: &TestIdentity,
        address: SocketAddr,
    ) -> Result<TlsError, TestFailure> {
        match TlsListener::bind(
            address,
            identity.certificate_path(),
            identity.private_key_path(),
        )
        .await
        {
            Ok(_) => Err(TestFailure::ExpectedTlsFailure),
            Err(error) => Ok(error),
        }
    }

    fn spawn_server(
        listener: TlsListener,
    ) -> (oneshot::Sender<()>, JoinHandle<std::io::Result<()>>) {
        let (shutdown_sender, shutdown_receiver) = oneshot::channel();
        let server = tokio::spawn(async move {
            crate::server_runtime::serve_http(listener, http::tests::health_router(), async move {
                let _shutdown_result = shutdown_receiver.await;
            })
            .await
        });
        (shutdown_sender, server)
    }

    async fn await_server(server: JoinHandle<std::io::Result<()>>) -> Result<(), TestFailure> {
        match timeout(TEST_TIMEOUT, server).await {
            Ok(Ok(Ok(()))) => Ok(()),
            Ok(Ok(Err(_)) | Err(_)) | Err(_) => Err(TestFailure::ServerDidNotExit),
        }
    }

    async fn connect_client(
        address: SocketAddr,
        trust_root: &CertificateDer<'static>,
        server_ip: IpAddr,
        versions: &[&'static rustls::SupportedProtocolVersion],
        alpn_protocols: Vec<Vec<u8>>,
    ) -> Result<TlsStream<TcpStream>, TestFailure> {
        let mut roots = RootCertStore::empty();
        roots
            .add(trust_root.clone())
            .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?;
        let mut config =
            ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_protocol_versions(versions)
                .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?
                .with_root_certificates(roots)
                .with_no_client_auth();
        config.alpn_protocols = alpn_protocols;
        let connector = TlsConnector::from(Arc::new(config));
        let tcp_stream = TcpStream::connect(address)
            .await
            .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?;
        connector
            .connect(ServerName::from(server_ip), tcp_stream)
            .await
            .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))
    }

    async fn request_health(tls_stream: &mut TlsStream<TcpStream>) -> Result<Vec<u8>, TestFailure> {
        tls_stream
            .write_all(HEALTH_REQUEST)
            .await
            .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?;
        let mut response = Vec::new();
        tls_stream
            .read_to_end(&mut response)
            .await
            .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?;
        Ok(response)
    }

    fn assert_health_response(response: &[u8]) -> Result<(), TestFailure> {
        let response = std::str::from_utf8(response)
            .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?;
        let Some((headers, body)) = response.split_once("\r\n\r\n") else {
            return Err(TestFailure::InvalidResponse);
        };
        let mut lines = headers.lines();
        if lines.next() != Some("HTTP/1.1 200 OK") {
            return Err(TestFailure::InvalidResponse);
        }
        let content_type_is_exact = lines.any(|line| {
            line.split_once(':').is_some_and(|(name, value)| {
                name.eq_ignore_ascii_case("content-type") && value.trim() == "application/json"
            })
        });
        if !content_type_is_exact || body != r#"{"status":"ok"}"# {
            return Err(TestFailure::InvalidResponse);
        }
        Ok(())
    }

    #[derive(Debug, Snafu)]
    pub(crate) enum TestFailure {
        #[snafu(context(false), display("test infrastructure operation failed"))]
        Source {
            #[snafu(source(from(crate::test_error::TestError, Box::new)))]
            source: Box<crate::test_error::TestError>,
        },
        #[snafu(display("the TLS test fixture failed"))]
        FixtureFailed,
        #[snafu(display("the TLS listener failed unexpectedly"))]
        ListenerFailed,
        #[snafu(display("the TLS client failed unexpectedly"))]
        ClientFailed,
        #[snafu(display("the TLS server did not exit promptly"))]
        ServerDidNotExit,
        #[snafu(display("the TLS test shutdown signal could not be sent"))]
        ShutdownSignalFailed,
        #[snafu(display("the negotiated ALPN protocol was unexpected"))]
        UnexpectedAlpn,
        #[snafu(display("the negotiated TLS protocol version was unexpected"))]
        UnexpectedProtocolVersion,
        #[snafu(display("the HTTP health response was invalid"))]
        InvalidResponse,
        #[snafu(display("the TLS handshake should have failed"))]
        HandshakeShouldHaveFailed,
        #[snafu(display("a TLS bind failure was expected"))]
        ExpectedTlsFailure,
        #[snafu(display("the TLS error classification was unexpected"))]
        UnexpectedTlsError,
        #[snafu(display("the TLS error exposed rejected context"))]
        TlsErrorWasNotRedacted,
        #[snafu(display("the configured TLS policy was not exact"))]
        TlsPolicyWasNotExact,
        #[snafu(display("the TLS private-key DER staging buffer was not zeroed"))]
        PrivateKeyDerWasNotZeroed,
        #[snafu(display("a stalled TLS handshake serialized acceptance"))]
        HandshakeWasSerialized,
        #[snafu(display("the fixed TLS handshake timeout was not enforced"))]
        HandshakeTimeoutWasNotEnforced,
    }
}
