use std::{fs, net::IpAddr, sync::Arc, time::Duration};

use natsume_device_protocol::{
    CONTROL_HELLO_TIMEOUT_SECONDS, CONTROL_MAX_FRAME_BYTES, CONTROL_SUBPROTOCOL,
    is_valid_device_token,
};
use rustls::{ClientConfig, RootCertStore};
use serde::Deserialize;
use tokio::{net::TcpStream, time::timeout};
use tokio_tungstenite::{
    Connector, MaybeTlsStream, WebSocketStream, client_async_tls_with_config,
    tungstenite::{
        Error as WebSocketError,
        client::IntoClientRequest as _,
        handshake::client::Response as WebSocketResponse,
        http::{HeaderValue, StatusCode, header as ws_header},
        protocol::WebSocketConfig,
    },
};
use zeroize::Zeroize as _;

use crate::CanonicalEndpoint;

use super::{ControlClient, ControlError, session::close_socket};

const CONTROL_PATH: &str = "/api/v2/device/control";
const HTTP_1_1_ALPN: &[u8] = b"http/1.1";

pub(super) type ControlSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

#[derive(Debug)]
struct NoCertificateResolver;

impl rustls::client::ResolvesClientCert for NoCertificateResolver {
    fn resolve(
        &self,
        _root_hint_subjects: &[&[u8]],
        _signature_schemes: &[rustls::SignatureScheme],
    ) -> Option<Arc<rustls::sign::CertifiedKey>> {
        None
    }

    fn has_certs(&self) -> bool {
        false
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AttemptError {
    LocalCredential,
    Unauthorized,
    RateLimited,
    ProtocolUnsupported,
    Reconnect,
    Transport,
}

impl ControlClient {
    pub(super) async fn connect(&self) -> Result<ControlSocket, AttemptError> {
        let request = self.build_request()?;
        let websocket_config = WebSocketConfig::default()
            .max_message_size(Some(CONTROL_MAX_FRAME_BYTES))
            .max_frame_size(Some(CONTROL_MAX_FRAME_BYTES));
        let tcp_stream = timeout(
            Duration::from_secs(CONTROL_HELLO_TIMEOUT_SECONDS),
            TcpStream::connect(self.socket_address),
        )
        .await
        .map_err(|_| AttemptError::Transport)?
        .map_err(|_| AttemptError::Transport)?;
        let connection = timeout(
            Duration::from_secs(CONTROL_HELLO_TIMEOUT_SECONDS),
            client_async_tls_with_config(
                request,
                tcp_stream,
                Some(websocket_config),
                Some(self.connector.clone()),
            ),
        )
        .await
        .map_err(|_| AttemptError::Transport)?;
        let (mut socket, response) = connection.map_err(classify_websocket_error)?;
        if !selected_subprotocol_is_exact(&response) {
            close_socket(&mut socket).await;
            return Err(AttemptError::Reconnect);
        }
        Ok(socket)
    }

    fn build_request(
        &self,
    ) -> Result<tokio_tungstenite::tungstenite::http::Request<()>, AttemptError> {
        let mut token = fs::read(&self.device_token).map_err(|_| AttemptError::LocalCredential)?;
        if !is_valid_device_token(&token) {
            token.zeroize();
            return Err(AttemptError::LocalCredential);
        }
        let mut authorization = b"Bearer ".to_vec();
        authorization.extend_from_slice(&token);
        let header = HeaderValue::from_bytes(&authorization);
        authorization.zeroize();
        token.zeroize();
        let header = header.map_err(|_| AttemptError::LocalCredential)?;

        let mut request = self
            .endpoint
            .as_str()
            .into_client_request()
            .map_err(|_| AttemptError::Transport)?;
        request
            .headers_mut()
            .insert(ws_header::AUTHORIZATION, header);
        request.headers_mut().insert(
            ws_header::SEC_WEBSOCKET_PROTOCOL,
            HeaderValue::from_static(CONTROL_SUBPROTOCOL),
        );
        Ok(request)
    }
}

#[derive(Deserialize)]
struct UpgradeErrorBody {
    code: String,
}

fn classify_websocket_error(error: WebSocketError) -> AttemptError {
    match error {
        WebSocketError::Http(response) => {
            classify_upgrade_rejection(response.status(), response.body().as_deref())
        }
        _ => AttemptError::Transport,
    }
}

pub(super) fn classify_upgrade_rejection(status: StatusCode, body: Option<&[u8]>) -> AttemptError {
    match status {
        StatusCode::UNAUTHORIZED => AttemptError::Unauthorized,
        StatusCode::TOO_MANY_REQUESTS => AttemptError::RateLimited,
        StatusCode::BAD_REQUEST
            if body
                .and_then(|body| serde_json::from_slice::<UpgradeErrorBody>(body).ok())
                .is_some_and(|body| body.code == "PROTOCOL_VERSION_UNSUPPORTED") =>
        {
            AttemptError::ProtocolUnsupported
        }
        _ => AttemptError::Transport,
    }
}

fn selected_subprotocol_is_exact(response: &WebSocketResponse) -> bool {
    let mut values = response
        .headers()
        .get_all(ws_header::SEC_WEBSOCKET_PROTOCOL)
        .iter();
    values
        .next()
        .is_some_and(|value| value.as_bytes() == CONTROL_SUBPROTOCOL.as_bytes())
        && values.next().is_none()
}

pub(super) fn build_tls_connector(
    control_certificate: rustls_pki_types::CertificateDer<'static>,
) -> Result<Connector, ControlError> {
    let mut roots = RootCertStore::empty();
    roots
        .add(control_certificate)
        .map_err(|_| ControlError::ControlTrustRoot)?;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|_| ControlError::TlsConfiguration)?;
    let mut config = builder
        .with_root_certificates(roots)
        .with_client_cert_resolver(Arc::new(NoCertificateResolver));
    config.alpn_protocols = vec![HTTP_1_1_ALPN.to_vec()];
    config.enable_early_data = false;
    Ok(Connector::Rustls(Arc::new(config)))
}

pub(super) fn control_url(endpoint: CanonicalEndpoint) -> String {
    let authority = match endpoint.ip() {
        IpAddr::V4(ip) => ip.to_string(),
        IpAddr::V6(ip) => format!("[{ip}]"),
    };
    format!("wss://{authority}:{}{CONTROL_PATH}", endpoint.port().get())
}
