use std::sync::Arc;

use rcgen::{
    BasicConstraints, CertificateParams, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose,
};
use rustls::{
    ClientConfig, RootCertStore, ServerConfig,
    pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer},
};

use super::ALPN_HTTP_1_1;

pub(super) fn configs() -> (Arc<ServerConfig>, Arc<ClientConfig>) {
    let mut ca_params = require_ok(
        CertificateParams::new(Vec::<String>::new()),
        "private test CA parameters must build",
    );
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params.key_usages = vec![
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
    ];
    let ca_key = require_ok(KeyPair::generate(), "private test CA key must generate");
    let ca_certificate = require_ok(
        ca_params.self_signed(&ca_key),
        "private test CA certificate must sign",
    );
    let root = ca_certificate.der().clone();
    let issuer = Issuer::new(ca_params, ca_key);

    let leaf_key = require_ok(KeyPair::generate(), "private TLS key must generate");
    let mut leaf_params = require_ok(
        CertificateParams::new(vec![std::net::Ipv4Addr::LOCALHOST.to_string()]),
        "private TLS leaf parameters must build",
    );
    leaf_params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    leaf_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let leaf = require_ok(
        leaf_params.signed_by(&leaf_key, &issuer),
        "private TLS leaf must sign",
    );

    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let server_builder = require_ok(
        ServerConfig::builder_with_provider(provider.clone())
            .with_protocol_versions(&[&rustls::version::TLS13]),
        "TLS 1.3 server policy must build",
    );
    let mut server = require_ok(
        server_builder.with_no_client_auth().with_single_cert(
            vec![leaf.der().clone()],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(leaf_key.serialize_der())),
        ),
        "private TLS identity must configure",
    );
    server.alpn_protocols = vec![ALPN_HTTP_1_1.to_vec()];
    server.max_early_data_size = 0;

    let mut roots = RootCertStore::empty();
    require_ok(roots.add(root), "private test root must parse");
    let client_builder = require_ok(
        ClientConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13]),
        "TLS 1.3 client policy must build",
    );
    let mut client = client_builder
        .with_root_certificates(roots)
        .with_no_client_auth();
    client.alpn_protocols = vec![ALPN_HTTP_1_1.to_vec()];
    client.enable_early_data = false;

    (Arc::new(server), Arc::new(client))
}

fn require_ok<T, E>(result: Result<T, E>, message: &str) -> T {
    match result {
        Ok(value) => value,
        Err(error) => {
            drop(error);
            panic!("{message}");
        }
    }
}
