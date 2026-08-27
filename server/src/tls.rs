use std::{
    fs, net::SocketAddr, os::unix::fs::PermissionsExt, path::Path, sync::Arc, time::Duration,
};

use axum::{
    extract::{ConnectInfo, FromRequestParts, connect_info::Connected},
    http::{StatusCode, request::Parts},
    serve::{IncomingStream, Listener},
};
use rustls::sign::{CertifiedKey, SigningKey};
use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use snafu::Snafu;
use tokio::{
    net::{TcpListener, TcpStream},
    task::JoinSet,
    time::{sleep, timeout},
};
use tokio_rustls::{TlsAcceptor, server::TlsStream};
use zeroize::Zeroize;

const ALPN_HTTP_1_1: &[u8] = b"http/1.1";
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const ACCEPT_RETRY_DELAY: Duration = Duration::from_millis(50);
const PRIVATE_FILE_FORBIDDEN_BITS: u32 = 0o177;
const PRIVATE_DIRECTORY_FORBIDDEN_BITS: u32 = 0o077;

/// A concrete TLS adapter for Axum's HTTP/1.1 server.
pub(crate) struct TlsListener {
    tcp_listener: TcpListener,
    tls_acceptor: TlsAcceptor,
    handshakes: JoinSet<Option<(TlsStream<TcpStream>, SocketAddr)>>,
}

#[allow(dead_code)]
#[derive(Clone, Copy)]
pub(crate) struct ClientAddress(SocketAddr);

impl ClientAddress {
    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) const fn new(address: SocketAddr) -> Self {
        Self(address)
    }

    #[allow(dead_code)]
    pub(crate) const fn ip(self) -> std::net::IpAddr {
        self.0.ip()
    }
}

impl Connected<IncomingStream<'_, TlsListener>> for ClientAddress {
    fn connect_info(stream: IncomingStream<'_, TlsListener>) -> Self {
        Self(*stream.remote_addr())
    }
}

impl<S> FromRequestParts<S> for ClientAddress
where
    S: Send + Sync,
{
    type Rejection = StatusCode;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        if let Some(ConnectInfo(address)) = parts.extensions.get::<ConnectInfo<Self>>() {
            return Ok(*address);
        }
        parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map_or(Err(StatusCode::INTERNAL_SERVER_ERROR), |address| {
                Ok(Self(address.0))
            })
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
            .map_err(|_| TlsError::Bind)?;
        Ok(Self {
            tcp_listener,
            tls_acceptor: TlsAcceptor::from(server_config),
            handshakes: JoinSet::new(),
        })
    }
}

impl Listener for TlsListener {
    type Io = TlsStream<TcpStream>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            let has_in_flight_handshakes = !self.handshakes.is_empty();
            tokio::select! {
                accepted = self.tcp_listener.accept() => {
                    match accepted {
                        Ok((tcp_stream, remote_address)) => {
                            let tls_acceptor = self.tls_acceptor.clone();
                            self.handshakes.spawn(async move {
                                match timeout(HANDSHAKE_TIMEOUT, tls_acceptor.accept(tcp_stream)).await {
                                    Ok(Ok(tls_stream)) => Some((tls_stream, remote_address)),
                                    Ok(Err(_)) | Err(_) => None,
                                }
                            });
                        }
                        Err(_) => sleep(ACCEPT_RETRY_DELAY).await,
                    }
                }
                completed = self.handshakes.join_next(), if has_in_flight_handshakes => {
                    if let Some(Ok(Some(connection))) = completed {
                        return connection;
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
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = rustls::ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|_| TlsError::Identity)?
        .with_no_client_auth();
    let certificate = CertificateDer::from(read_private_file(certificate_path)?);
    let mut private_key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(read_private_file(
        private_key_path,
    )?));
    let signing_key = load_signing_key(&mut private_key)?;
    let certified_key = CertifiedKey::new(vec![certificate], signing_key);
    certified_key.keys_match().map_err(|_| TlsError::Identity)?;
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
    signing_key.map_err(|_| TlsError::Identity)
}

pub(crate) fn read_private_file(path: &Path) -> Result<Vec<u8>, TlsError> {
    let parent = path.parent().ok_or(TlsError::Identity)?;
    let parent_metadata = fs::metadata(parent).map_err(|_| TlsError::Identity)?;
    if !parent_metadata.is_dir()
        || parent_metadata.permissions().mode() & PRIVATE_DIRECTORY_FORBIDDEN_BITS != 0
    {
        return Err(TlsError::Identity);
    }
    let metadata = fs::metadata(path).map_err(|_| TlsError::Identity)?;
    if !metadata.is_file() {
        return Err(TlsError::Identity);
    }
    if metadata.permissions().mode() & PRIVATE_FILE_FORBIDDEN_BITS != 0 {
        return Err(TlsError::Identity);
    }
    fs::read(path).map_err(|_| TlsError::Identity)
}

/// Redacted TLS identity, configuration, or binding failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Snafu)]
pub(crate) enum TlsError {
    #[snafu(display("the TLS identity is invalid"))]
    Identity,
    #[snafu(display("the TLS listener could not bind"))]
    Bind,
}

#[cfg(test)]
pub(crate) mod tests {
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

    use super::{ALPN_HTTP_1_1, TlsError, TlsListener, load_server_config, load_signing_key};

    pub(crate) struct TestSupportError;

    pub(crate) struct TestIdentity {
        directory: TestDirectory,
        certificate_path: PathBuf,
        private_key_path: PathBuf,
        ca_certificate: CertificateDer<'static>,
    }

    impl TestIdentity {
        pub(crate) fn new(ip_san: Ipv4Addr) -> Result<Self, TestSupportError> {
            let directory = TestDirectory::new()?;

            let mut ca_params =
                CertificateParams::new(Vec::<String>::new()).map_err(|_| TestSupportError)?;
            ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
            ca_params.key_usages = vec![
                KeyUsagePurpose::DigitalSignature,
                KeyUsagePurpose::KeyCertSign,
                KeyUsagePurpose::CrlSign,
            ];
            let ca_key = KeyPair::generate().map_err(|_| TestSupportError)?;
            let ca_certificate = ca_params
                .self_signed(&ca_key)
                .map_err(|_| TestSupportError)?;
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

            let leaf_key = KeyPair::generate().map_err(|_| TestSupportError)?;
            let mut leaf_params =
                CertificateParams::new(vec![ip_san.to_string()]).map_err(|_| TestSupportError)?;
            leaf_params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
            leaf_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
            let leaf_certificate = leaf_params
                .signed_by(&leaf_key, &issuer)
                .map_err(|_| TestSupportError)?;

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
                .map_err(|_| TestSupportError)
        }
    }

    fn install_der(path: &Path, contents: &[u8]) -> Result<(), TestSupportError> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .map_err(|_| TestSupportError)?;
        file.write_all(contents).map_err(|_| TestSupportError)
    }

    fn install_certificate_pem(path: &Path, der: &[u8]) -> Result<(), TestSupportError> {
        let base64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, der);
        let mut pem = String::from("-----BEGIN CERTIFICATE-----\n");
        for line in base64.as_bytes().chunks(64) {
            pem.push_str(std::str::from_utf8(line).map_err(|_| TestSupportError)?);
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
            fs::create_dir(&path).map_err(|_| TestSupportError)?;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
                .map_err(|_| TestSupportError)?;
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
    async fn health_uses_http1_tls13_and_shuts_down_gracefully() -> Result<(), TestFailure> {
        let identity = TestIdentity::new(LOCALHOST).map_err(|_| TestFailure::FixtureFailed)?;
        let listener = bind_identity(&identity).await?;
        let address = Listener::local_addr(&listener).map_err(|_| TestFailure::ListenerFailed)?;
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
        let identity = TestIdentity::new(LOCALHOST).map_err(|_| TestFailure::FixtureFailed)?;
        let listener = bind_identity(&identity).await?;
        let address = Listener::local_addr(&listener).map_err(|_| TestFailure::ListenerFailed)?;
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
        let identity = TestIdentity::new(LOCALHOST).map_err(|_| TestFailure::FixtureFailed)?;
        let wrong_identity =
            TestIdentity::new(LOCALHOST).map_err(|_| TestFailure::FixtureFailed)?;
        let listener = bind_identity(&identity).await?;
        let address = Listener::local_addr(&listener).map_err(|_| TestFailure::ListenerFailed)?;
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
        let identity = TestIdentity::new(LOCALHOST).map_err(|_| TestFailure::FixtureFailed)?;
        let listener = bind_identity(&identity).await?;
        let address = Listener::local_addr(&listener).map_err(|_| TestFailure::ListenerFailed)?;
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
        let identity = TestIdentity::new(LOCALHOST).map_err(|_| TestFailure::FixtureFailed)?;
        let other_identity =
            TestIdentity::new(LOCALHOST).map_err(|_| TestFailure::FixtureFailed)?;
        identity
            .replace_private_key_from(&other_identity)
            .map_err(|_| TestFailure::FixtureFailed)?;
        let occupied = StdTcpListener::bind(SocketAddr::from((LOCALHOST, 0)))
            .map_err(|_| TestFailure::FixtureFailed)?;
        let address = occupied
            .local_addr()
            .map_err(|_| TestFailure::FixtureFailed)?;

        let error = expect_bind_error(&identity, address).await?;
        if error != TlsError::Identity {
            return Err(TestFailure::UnexpectedTlsError);
        }
        drop(occupied);
        Ok(())
    }

    #[tokio::test]
    async fn malformed_der_error_is_redacted() -> Result<(), TestFailure> {
        let identity = TestIdentity::new(LOCALHOST).map_err(|_| TestFailure::FixtureFailed)?;
        fs::write(identity.certificate_path(), b"malformed-der-content-canary")
            .map_err(|_| TestFailure::FixtureFailed)?;
        let error = expect_bind_error(&identity, SocketAddr::from((LOCALHOST, 0))).await?;
        if error != TlsError::Identity {
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
        let certificate_wide =
            TestIdentity::new(LOCALHOST).map_err(|_| TestFailure::FixtureFailed)?;
        fs::set_permissions(
            certificate_wide.certificate_path(),
            fs::Permissions::from_mode(0o644),
        )
        .map_err(|_| TestFailure::FixtureFailed)?;
        if expect_bind_error(&certificate_wide, SocketAddr::from((LOCALHOST, 0))).await?
            != TlsError::Identity
        {
            return Err(TestFailure::UnexpectedTlsError);
        }

        let key_wide = TestIdentity::new(LOCALHOST).map_err(|_| TestFailure::FixtureFailed)?;
        fs::set_permissions(
            key_wide.private_key_path(),
            fs::Permissions::from_mode(0o644),
        )
        .map_err(|_| TestFailure::FixtureFailed)?;
        if expect_bind_error(&key_wide, SocketAddr::from((LOCALHOST, 0))).await?
            != TlsError::Identity
        {
            return Err(TestFailure::UnexpectedTlsError);
        }

        let directory_wide =
            TestIdentity::new(LOCALHOST).map_err(|_| TestFailure::FixtureFailed)?;
        fs::set_permissions(
            directory_wide.directory_path(),
            fs::Permissions::from_mode(0o755),
        )
        .map_err(|_| TestFailure::FixtureFailed)?;
        if expect_bind_error(&directory_wide, SocketAddr::from((LOCALHOST, 0))).await?
            != TlsError::Identity
        {
            return Err(TestFailure::UnexpectedTlsError);
        }
        Ok(())
    }

    #[test]
    fn tls_policy_is_exact() -> Result<(), TestFailure> {
        let identity = TestIdentity::new(LOCALHOST).map_err(|_| TestFailure::FixtureFailed)?;
        let config = load_server_config(identity.certificate_path(), identity.private_key_path())
            .map_err(|_| TestFailure::ListenerFailed)?;
        if config.alpn_protocols != vec![ALPN_HTTP_1_1.to_vec()] || config.max_early_data_size != 0
        {
            return Err(TestFailure::TlsPolicyWasNotExact);
        }
        Ok(())
    }

    #[test]
    fn signing_key_der_is_zeroed_before_return() -> Result<(), TestFailure> {
        let identity = TestIdentity::new(LOCALHOST).map_err(|_| TestFailure::FixtureFailed)?;
        let mut private_key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
            fs::read(identity.private_key_path()).map_err(|_| TestFailure::FixtureFailed)?,
        ));
        load_signing_key(&mut private_key).map_err(|_| TestFailure::ListenerFailed)?;
        if private_key.secret_der().iter().any(|byte| *byte != 0) {
            return Err(TestFailure::PrivateKeyDerWasNotZeroed);
        }
        Ok(())
    }

    #[tokio::test]
    async fn stalled_handshake_does_not_serialize_acceptance() -> Result<(), TestFailure> {
        let identity = TestIdentity::new(LOCALHOST).map_err(|_| TestFailure::FixtureFailed)?;
        let listener = bind_identity(&identity).await?;
        let address = Listener::local_addr(&listener).map_err(|_| TestFailure::ListenerFailed)?;
        let stalled_socket = TcpStream::connect(address)
            .await
            .map_err(|_| TestFailure::ClientFailed)?;
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
            .map_err(|_| TestFailure::HandshakeWasSerialized)??;
        shutdown
            .send(())
            .map_err(|()| TestFailure::ShutdownSignalFailed)?;
        drop(stalled_socket);
        await_server(server).await
    }

    #[tokio::test]
    async fn shutdown_aborts_stalled_handshake() -> Result<(), TestFailure> {
        let identity = TestIdentity::new(LOCALHOST).map_err(|_| TestFailure::FixtureFailed)?;
        let listener = bind_identity(&identity).await?;
        let address = Listener::local_addr(&listener).map_err(|_| TestFailure::ListenerFailed)?;
        let stalled_socket = TcpStream::connect(address)
            .await
            .map_err(|_| TestFailure::ClientFailed)?;
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
        let identity = TestIdentity::new(LOCALHOST).map_err(|_| TestFailure::FixtureFailed)?;
        let listener = bind_identity(&identity).await?;
        let address = Listener::local_addr(&listener).map_err(|_| TestFailure::ListenerFailed)?;
        let mut stalled_socket = TcpStream::connect(address)
            .await
            .map_err(|_| TestFailure::ClientFailed)?;
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

    async fn bind_identity(identity: &TestIdentity) -> Result<TlsListener, TestFailure> {
        TlsListener::bind(
            SocketAddr::from((LOCALHOST, 0)),
            identity.certificate_path(),
            identity.private_key_path(),
        )
        .await
        .map_err(|_| TestFailure::ListenerFailed)
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
            axum::serve(listener, http::tests::health_router())
                .with_graceful_shutdown(async move {
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
            .map_err(|_| TestFailure::ClientFailed)?;
        let mut config =
            ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_protocol_versions(versions)
                .map_err(|_| TestFailure::ClientFailed)?
                .with_root_certificates(roots)
                .with_no_client_auth();
        config.alpn_protocols = alpn_protocols;
        let connector = TlsConnector::from(Arc::new(config));
        let tcp_stream = TcpStream::connect(address)
            .await
            .map_err(|_| TestFailure::ClientFailed)?;
        connector
            .connect(ServerName::from(server_ip), tcp_stream)
            .await
            .map_err(|_| TestFailure::ClientFailed)
    }

    async fn request_health(tls_stream: &mut TlsStream<TcpStream>) -> Result<Vec<u8>, TestFailure> {
        tls_stream
            .write_all(HEALTH_REQUEST)
            .await
            .map_err(|_| TestFailure::ClientFailed)?;
        let mut response = Vec::new();
        tls_stream
            .read_to_end(&mut response)
            .await
            .map_err(|_| TestFailure::ClientFailed)?;
        Ok(response)
    }

    fn assert_health_response(response: &[u8]) -> Result<(), TestFailure> {
        let response = std::str::from_utf8(response).map_err(|_| TestFailure::InvalidResponse)?;
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
    enum TestFailure {
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
