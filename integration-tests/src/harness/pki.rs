use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use rcgen::{
    BasicConstraints, CertificateParams, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose, PKCS_ECDSA_P256_SHA256,
};
use zeroize::Zeroize as _;

use super::{require_ok, server::LOCALHOST};

pub(super) struct ServerPki {
    pub(super) private_directory: PathBuf,
    pub(super) server_certificate_path: PathBuf,
    pub(super) server_key_path: PathBuf,
    pub(super) control_root_path: PathBuf,
    pub(super) origin_root_path: PathBuf,
    pub(super) control_certificate_der: Vec<u8>,
}

pub(super) fn install_server_pki(directory: &Path) -> ServerPki {
    let private_directory = directory.join("server-keys");
    require_ok(
        fs::create_dir(&private_directory),
        "server private directory must be created",
    );
    require_ok(
        fs::set_permissions(&private_directory, fs::Permissions::from_mode(0o700)),
        "server private directory mode must be set",
    );

    let (control_params, control_key, control_certificate_der) = make_ca();
    let control_issuer = Issuer::new(control_params, control_key);
    let server_key = require_ok(
        KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256),
        "server TLS key must be generated",
    );
    let mut server_params = require_ok(
        CertificateParams::new(vec![LOCALHOST.to_string()]),
        "server TLS parameters must be created",
    );
    server_params.is_ca = IsCa::ExplicitNoCa;
    server_params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    server_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    server_params.not_before = rcgen::date_time_ymd(2020, 1, 1);
    server_params.not_after = rcgen::date_time_ymd(4090, 1, 1);
    let server_certificate = require_ok(
        server_params.signed_by(&server_key, &control_issuer),
        "server TLS leaf must be signed",
    );
    let server_certificate_path = private_directory.join("server-tls-leaf.der");
    let server_key_path = private_directory.join("server-tls-key.pk8");
    install_private(&server_certificate_path, server_certificate.der().as_ref());
    let mut server_key_der = server_key.serialize_der();
    install_private(&server_key_path, &server_key_der);
    server_key_der.zeroize();

    let (_origin_params, origin_key, origin_certificate_der) = make_ca();
    install_private(
        &private_directory.join("origin-ca.der"),
        &origin_certificate_der,
    );
    let mut origin_key_der = origin_key.serialize_der();
    install_private(
        &private_directory.join("origin-ca-key.pk8"),
        &origin_key_der,
    );
    origin_key_der.zeroize();
    let control_root_path = directory.join("control-ca.crt");
    let origin_root_path = directory.join("local-origin-ca.crt");
    write_certificate_pem(&control_root_path, &control_certificate_der);
    write_certificate_pem(&origin_root_path, &origin_certificate_der);

    ServerPki {
        private_directory,
        server_certificate_path,
        server_key_path,
        control_root_path,
        origin_root_path,
        control_certificate_der,
    }
}

fn make_ca() -> (CertificateParams, KeyPair, Vec<u8>) {
    let key = require_ok(
        KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256),
        "test CA key must be generated",
    );
    let mut params = require_ok(
        CertificateParams::new(Vec::<String>::new()),
        "test CA parameters must be created",
    );
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
    ];
    params.not_before = rcgen::date_time_ymd(2020, 1, 1);
    params.not_after = rcgen::date_time_ymd(4095, 1, 1);
    let certificate = require_ok(
        params.self_signed(&key),
        "test CA certificate must be signed",
    );
    (params, key, certificate.der().to_vec())
}

fn install_private(path: &Path, contents: &[u8]) {
    let mut file = require_ok(
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path),
        "private fixture must be created",
    );
    require_ok(file.write_all(contents), "private fixture must be written");
}

fn write_certificate_pem(path: &Path, certificate_der: &[u8]) {
    let encoded = STANDARD.encode(certificate_der);
    let mut pem = String::from("-----BEGIN CERTIFICATE-----\n");
    for line in encoded.as_bytes().chunks(64) {
        let line = require_ok(
            std::str::from_utf8(line),
            "certificate base64 must be UTF-8",
        );
        pem.push_str(line);
        pem.push('\n');
    }
    pem.push_str("-----END CERTIFICATE-----\n");
    require_ok(fs::write(path, pem), "certificate PEM must be written");
}
