use std::{
    error::Error as _,
    net::{Ipv4Addr, Ipv6Addr},
};

use rcgen::{CertificateParams, KeyPair};
use snafu::{OptionExt as _, Snafu};

use super::{
    generate::GenerateError,
    import::{ImportError, import_certificate, import_private_key},
    *,
};

#[derive(Debug, Snafu)]
enum TestError {
    #[snafu(display("required PKI fixture value missing: {name}"))]
    MissingValue { name: &'static str },
    #[snafu(context(false), display("PKI fixture filesystem or process failed"))]
    Io { source: std::io::Error },
    #[snafu(context(false), display("PKI fixture configuration failed"))]
    Config {
        source: natsume_server::config::ConfigError,
    },
    #[snafu(context(false), display("PKI fixture deployment validation failed"))]
    Deployment {
        source: natsume_server::DeploymentError,
    },
    #[snafu(context(false), display("PKI fixture encryption parameters failed"))]
    Parameters { source: pkcs5::Error },
    #[snafu(display("OpenSSL failed with {status}"))]
    OpenSsl { status: std::process::ExitStatus },
    #[snafu(context(false), display("PKI generation failed"))]
    Generate { source: GenerateError },
    #[snafu(context(false), display("PKI extension parsing failed"))]
    Extension {
        source: x509_parser::error::X509Error,
    },
    #[snafu(context(false), display("PKI fixture generation failed"))]
    Rcgen { source: rcgen::Error },
    #[snafu(context(false), display("PKI fixture import failed"))]
    Import { source: ImportError },
    #[snafu(context(false), display("PKI fixture DER encoding failed"))]
    Der { source: pkcs8::der::Error },
    #[snafu(context(false), display("PKI fixture PEM encoding failed"))]
    Pem { source: pkcs8::der::pem::Error },
}

#[test]
fn certificate_and_plaintext_pkcs8_format_matrix() -> Result<(), TestError> {
    let key = KeyPair::generate()?;
    let params = CertificateParams::new(vec![Ipv4Addr::LOCALHOST.to_string()])?;
    let cert = params.self_signed(&key)?;
    let imported = import_certificate(cert.der())?;
    assert_eq!(import_certificate(cert.pem().as_bytes())?, imported);
    assert_eq!(imported.ip_sans(), vec![IpAddr::V4(Ipv4Addr::LOCALHOST)]);
    assert!(imported.is_self_signed());
    assert!(!imported.is_ca());
    assert!(!imported.subject().is_empty());
    assert_eq!(import_certificate(imported.to_pem().as_bytes())?, imported);
    assert_eq!(imported.sha256_fingerprint().len(), 95);
    let fingerprint = imported.sha256_fingerprint();
    assert!(fingerprint.split(':').all(|byte| {
        byte.len() == 2
            && byte
                .bytes()
                .all(|digit| digit.is_ascii_digit() || (b'A'..=b'F').contains(&digit))
    }));
    assert_eq!(
        imported.spki_sha256(),
        <[u8; 32]>::from(Sha256::digest(key.subject_public_key_info()))
    );
    for der in [key.serialize_der(), key.serialize_pem().into_bytes()] {
        let private = import_private_key(&der, None)?;
        assert_eq!(private.algorithm(), KeyAlgorithm::P256);
        assert!(private.matches(&imported));
        assert_eq!(format!("{private:?}"), "PrivateKey { <redacted> }");
    }
    assert!(matches!(
        import_certificate(format!("{}{}", cert.pem(), cert.pem()).as_bytes()),
        Err(ImportError::MultipleCertificates)
    ));
    let mut trailing = cert.der().to_vec();
    trailing.push(0);
    assert!(matches!(
        import_certificate(&trailing),
        Err(ImportError::TrailingData)
    ));
    let other_key = KeyPair::generate_for(&rcgen::PKCS_ED25519)?;
    assert_eq!(
        import_private_key(&other_key.serialize_der(), None)?.algorithm(),
        KeyAlgorithm::Other
    );
    let error = import_certificate(b"invalid certificate")
        .err()
        .context(MissingValueSnafu {
            name: "malformed DER must fail",
        })?;
    assert!(error.source().is_some());
    Ok(())
}

fn now() -> OffsetDateTime {
    OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(1_861_920_000)
}

fn ips() -> Vec<IpAddr> {
    vec![Ipv4Addr::LOCALHOST.into(), Ipv6Addr::LOCALHOST.into()]
}

#[test]
fn generated_ca_and_leaf_profiles() -> Result<(), TestError> {
    use time::Duration;
    use x509_parser::extensions::ParsedExtension;

    let now = now();
    let (ca, ca_key) = generate::generate_ca("Control CA", "Example", now)?;
    assert!(ca.is_ca());
    assert!(ca.is_self_signed());
    assert!(ca_key.matches(&ca));
    assert_eq!(ca_key.algorithm(), KeyAlgorithm::P256);
    assert_eq!(ca.not_before(), now - Duration::hours(1));
    assert_eq!(ca.not_after(), now + Duration::days(3650));
    let parsed = ca.parsed()?;
    let constraints = parsed.basic_constraints()?.context(MissingValueSnafu {
        name: "CA constraints exist",
    })?;
    assert!(constraints.critical);
    assert!(constraints.value.ca);
    assert_eq!(constraints.value.path_len_constraint, Some(0));
    let usage = parsed.key_usage()?.context(MissingValueSnafu {
        name: "CA key usage exists",
    })?;
    assert!(usage.critical);
    assert!(usage.value.key_cert_sign());
    assert!(usage.value.crl_sign());
    assert_eq!(usage.value.flags, (1 << 5) | (1 << 6));
    assert!(parsed.extensions().iter().any(|ext| matches!(
        ext.parsed_extension(),
        ParsedExtension::SubjectKeyIdentifier(_)
    )));
    let (leaf, key) =
        generate::issue_server_leaf(&ca, &ca_key, &ips(), now + Duration::days(30), now)?;
    assert_eq!(key.algorithm(), KeyAlgorithm::P256);
    assert!(key.matches(&leaf));
    assert!(!key.matches(&ca));
    assert!(!leaf.is_ca());
    assert!(!leaf.is_self_signed());
    assert_eq!(leaf.not_before(), now - Duration::hours(1));
    assert_eq!(leaf.not_after(), now + Duration::days(365));
    assert_eq!(leaf.ip_sans(), ips());
    let parsed = leaf.parsed()?;
    parsed.verify_signature(Some(ca.parsed()?.public_key()))?;
    let constraints = parsed.basic_constraints()?.context(MissingValueSnafu {
        name: "leaf constraints exist",
    })?;
    assert!(constraints.critical);
    assert!(!constraints.value.ca);
    let usage = parsed.key_usage()?.context(MissingValueSnafu {
        name: "leaf key usage exists",
    })?;
    assert!(usage.critical);
    assert_eq!(usage.value.flags, 1);
    assert!(
        parsed
            .extended_key_usage()?
            .context(MissingValueSnafu { name: "EKU exists" })?
            .value
            .server_auth
    );
    let sans = parsed
        .subject_alternative_name()?
        .context(MissingValueSnafu { name: "SANs exist" })?;
    assert_eq!(sans.value.general_names.len(), ips().len());
    let mut ski = None;
    let mut aki = None;
    for ext in parsed.extensions() {
        match ext.parsed_extension() {
            ParsedExtension::SubjectKeyIdentifier(identifier) => ski = Some(identifier.0),
            ParsedExtension::AuthorityKeyIdentifier(identifier) => {
                aki = identifier.key_identifier.as_ref().map(|value| value.0);
            }
            _ => {}
        }
    }
    assert!(ski.is_some());
    let ca_ski = ca
        .parsed()?
        .extensions()
        .iter()
        .find_map(|ext| match ext.parsed_extension() {
            ParsedExtension::SubjectKeyIdentifier(identifier) => Some(identifier.0.to_vec()),
            _ => None,
        })
        .context(MissingValueSnafu { name: "CA has SKI" })?;
    assert_eq!(aki, Some(ca_ski.as_slice()));
    Ok(())
}

#[test]
fn leaf_expiry_is_capped_by_ca_and_must_cover_gateway() -> Result<(), TestError> {
    use rcgen::{BasicConstraints, IsCa, KeyUsagePurpose};
    use time::Duration;

    let now = now();
    let key = KeyPair::generate()?;
    let mut params = CertificateParams::default();
    params.not_before = now - Duration::hours(1);
    params.not_after = now + Duration::days(90);
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign];
    let ca = import_certificate(params.self_signed(&key)?.der())?;
    let key = import_private_key(&key.serialize_der(), None)?;
    let (leaf, _) = generate::issue_server_leaf(&ca, &key, &ips(), ca.not_after(), now)?;
    assert_eq!(leaf.not_after(), ca.not_after());
    assert!(matches!(
        generate::issue_server_leaf(
            &ca,
            &key,
            &ips(),
            ca.not_after() + Duration::seconds(1),
            now
        ),
        Err(GenerateError::InsufficientValidity)
    ));
    let (long_ca, long_key) = generate::generate_ca("Long-lived CA", "Natsume", now)?;
    assert!(matches!(
        generate::issue_server_leaf(
            &long_ca,
            &long_key,
            &ips(),
            now + Duration::days(365) + Duration::nanoseconds(1),
            now + Duration::nanoseconds(1),
        ),
        Err(GenerateError::InsufficientValidity)
    ));
    let (_, wrong) = generate::generate_ca("Wrong", "Natsume", now)?;
    assert!(matches!(
        generate::issue_server_leaf(&ca, &wrong, &ips(), now, now),
        Err(GenerateError::IssuerKeyMismatch)
    ));
    Ok(())
}

#[test]
fn csr_requests_ip_sans_and_server_auth() -> Result<(), TestError> {
    let (key, request) = generate::create_csr(&ips())?;
    let parsed = rcgen::CertificateSigningRequestParams::from_der(&request.der().into())?;
    assert_eq!(key.algorithm(), KeyAlgorithm::P256);
    assert_eq!(
        parsed.params.subject_alt_names,
        ips()
            .into_iter()
            .map(rcgen::SanType::IpAddress)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        parsed.params.extended_key_usages,
        vec![rcgen::ExtendedKeyUsagePurpose::ServerAuth]
    );
    assert!(
        request
            .pem()
            .starts_with("-----BEGIN CERTIFICATE REQUEST-----")
    );
    Ok(())
}

#[test]
fn all_source_choices_have_expected_availability_and_defaults() {
    use super::rules::{
        ControlCaChoice::*, ServerTlsOption::*, Unavailability::*, default_server_tls_option,
        server_tls_options,
    };

    for (control, expected, default) in [
        (
            Generated,
            vec![
                (Issue, Ok(())),
                (Import, Err(NewCaHasNoExistingCertificates)),
                (SigningRequest, Err(NewCaKeyAvailable)),
            ],
            Issue,
        ),
        (
            ImportedWithKey,
            vec![(Issue, Ok(())), (Import, Ok(())), (SigningRequest, Ok(()))],
            Issue,
        ),
        (
            CertificateOnly,
            vec![
                (Issue, Err(ControlKeyOffline)),
                (Import, Ok(())),
                (SigningRequest, Ok(())),
            ],
            SigningRequest,
        ),
    ] {
        assert_eq!(server_tls_options(control), expected, "{control:?}");
        assert_eq!(default_server_tls_option(control), default, "{control:?}");
    }
}

#[test]
fn encrypted_pkcs8_roundtrip_and_passphrase_errors() -> Result<(), TestError> {
    use pkcs8::{EncryptedPrivateKeyInfoRef, der::Decode as _};
    use secrecy::SecretString;

    let (_, key) = generate::generate_ca("Control CA", "Natsume", now())?;
    let passphrase = SecretString::from("pki-test-passphrase-canary");
    let pem = generate::encrypt_private_key_pem(&key, &passphrase)?;
    let (_, document) = pkcs8::SecretDocument::from_pem(&pem)?;
    let encrypted = EncryptedPrivateKeyInfoRef::from_der(document.as_bytes())?;
    let pkcs5::EncryptionScheme::Pbes2(params) = &encrypted.encryption_algorithm else {
        panic!("export must use PBES2");
    };
    let pbkdf = params.kdf.pbkdf2().context(MissingValueSnafu {
        name: "export must use PBKDF2",
    })?;
    assert_eq!(pbkdf.iteration_count, generate::ENCRYPTION_ITERATIONS);
    assert_eq!(pbkdf.prf, pkcs5::pbes2::Pbkdf2Prf::HmacWithSha256);
    assert_eq!(pbkdf.salt.as_ref().len(), 16);
    assert!(matches!(
        params.encryption,
        pkcs5::pbes2::EncryptionScheme::Aes256Cbc { .. }
    ));
    for bytes in [pem.as_bytes(), document.as_bytes()] {
        assert_eq!(
            import_private_key(bytes, Some(&passphrase))?.pkcs8_der(),
            key.pkcs8_der()
        );
        assert!(matches!(
            import_private_key(bytes, None),
            Err(ImportError::PassphraseRequired)
        ));
    }
    let error = import_private_key(
        pem.as_bytes(),
        Some(&SecretString::from("wrong-passphrase-canary")),
    )
    .err()
    .context(MissingValueSnafu {
        name: "wrong passphrase must fail",
    })?;
    assert!(matches!(error, ImportError::WrongPassphrase { .. }));
    assert!(error.source().is_some());
    for message in [error.to_string(), format!("{error:?}")] {
        assert!(!message.contains("passphrase-canary"));
        assert!(!message.contains("PRIVATE KEY"));
    }
    let second = generate::encrypt_private_key_pem(&key, &passphrase)?;
    let (_, second_der) = pkcs8::SecretDocument::from_pem(&second)?;
    let second = EncryptedPrivateKeyInfoRef::from_der(second_der.as_bytes())?;
    let pkcs5::EncryptionScheme::Pbes2(second_params) = &second.encryption_algorithm else {
        panic!("second export must use PBES2");
    };
    assert_ne!(
        params
            .kdf
            .pbkdf2()
            .context(MissingValueSnafu { name: "PBKDF2" })?
            .salt,
        second_params
            .kdf
            .pbkdf2()
            .context(MissingValueSnafu { name: "PBKDF2" })?
            .salt
    );
    assert_ne!(params.encryption, second_params.encryption);
    Ok(())
}

#[test]
fn excessive_kdf_costs_are_rejected_before_decryption() -> Result<(), TestError> {
    use super::import::{MAX_PBKDF2_ITERATIONS, MAX_SCRYPT_N, MAX_SCRYPT_P, MAX_SCRYPT_R};
    use pkcs5::pbes2::{Kdf, Parameters, ScryptParams};
    use pkcs8::{
        EncryptedPrivateKeyInfoRef,
        der::{Encode as _, asn1::OctetStringRef},
    };

    let excessive_pbkdf =
        Parameters::generate_pbkdf2_sha256_aes256cbc(MAX_PBKDF2_ITERATIONS + 1, &[0; 16], [0; 16])?;
    let scrypt = ScryptParams {
        salt: (&[0u8; 16][..]).try_into()?,
        cost_parameter: 16_384,
        block_size: 8,
        parallelization: 1,
        key_length: None,
    };
    let mut parameters = vec![excessive_pbkdf.clone()];
    for kdf in [
        ScryptParams {
            cost_parameter: MAX_SCRYPT_N * 2,
            ..scrypt
        },
        ScryptParams {
            block_size: MAX_SCRYPT_R + 1,
            ..scrypt
        },
        ScryptParams {
            parallelization: MAX_SCRYPT_P + 1,
            ..scrypt
        },
    ] {
        parameters.push(Parameters {
            kdf: Kdf::Scrypt(kdf),
            encryption: excessive_pbkdf.encryption,
        });
    }
    for params in parameters {
        let encrypted = EncryptedPrivateKeyInfoRef {
            encryption_algorithm: params.into(),
            encrypted_data: OctetStringRef::new(&[0; 16])?,
        };
        let der = encrypted.to_der()?;
        let pem =
            pkcs8::der::pem::encode_string("ENCRYPTED PRIVATE KEY", pkcs8::LineEnding::LF, &der)?;
        for bytes in [der.as_slice(), pem.as_bytes()] {
            assert!(matches!(
                import_private_key(bytes, Some(&secrecy::SecretString::from("canary"))),
                Err(ImportError::KdfLimitExceeded)
            ));
            assert!(matches!(
                import_private_key(bytes, None),
                Err(ImportError::KdfLimitExceeded)
            ));
        }
    }
    Ok(())
}

#[test]
fn legacy_openssl_pem_has_conversion_instructions() -> Result<(), TestError> {
    let legacy = b"-----BEGIN EC PRIVATE KEY-----\nProc-Type: 4,ENCRYPTED\nDEK-Info: AES-256-CBC,00112233445566778899AABBCCDDEEFF\n\nAAAA\n-----END EC PRIVATE KEY-----\n";
    let error = import_private_key(legacy, None)
        .err()
        .context(MissingValueSnafu {
            name: "legacy PEM must fail before decryption",
        })?;
    assert!(matches!(error, ImportError::LegacyEncryptedPem));
    assert!(error.to_string().contains(
        "openssl pkcs8 -topk8 -v2 aes-256-cbc -v2prf hmacWithSHA256 -in <old> -out <new>"
    ));
    Ok(())
}

struct Fixture {
    directory: tempfile::TempDir,
    config: natsume_server::config::ServerConfig,
}

impl Fixture {
    fn new() -> Result<Self, TestError> {
        let directory = tempfile::tempdir()?;
        let root = directory.path().display();
        let path = directory.path().join("config.toml");
        std::fs::write(
            &path,
            format!(
                r#"
[listen]
https = "127.0.0.1:8443"
[storage]
database = "{root}/natsume.db"
root_key = "{root}/keys/server-root.key"
organization_logos = "{root}/logos"
print_documents = "{root}/print-documents"
[tls]
certificate = "{root}/keys/server-tls-leaf.der"
private_key = "{root}/keys/server-tls-key.pk8"
[site]
gateway_hostname = "gateway.example.test"
gateway_not_after = "2029-06-02T00:00:00Z"
contest_end = "2029-06-01T00:00:00Z"
[trust]
control_root = "{root}/control-ca.crt"
local_origin_root = "{root}/local-origin-ca.crt"
[runtime]
domjudge_origin = "https://judge.example.test"
"#
            ),
        )?;
        let config = natsume_server::config::ServerConfig::load_from(&path)?;
        Ok(Self { directory, config })
    }

    fn path(&self, name: &str) -> String {
        self.directory
            .path()
            .join(name)
            .to_string_lossy()
            .into_owned()
    }
}

#[test]
fn generated_materials_pass_server_validation_facade() -> Result<(), TestError> {
    use time::Duration;

    let fixture = Fixture::new()?;
    let now = now();
    let (control, control_key) = generate::generate_ca("Control CA", "Natsume", now)?;
    let (origin, origin_key) = generate::generate_ca("Origin CA", "Natsume", now)?;
    let (leaf, leaf_key) = generate::issue_server_leaf(
        &control,
        &control_key,
        &ips(),
        now + Duration::days(180),
        now,
    )?;
    super::validate::validate_materials(
        &fixture.config,
        &control,
        &leaf,
        &leaf_key,
        &origin,
        &origin_key,
        &ips(),
        now.into(),
    )?;
    let error = super::validate::validate_materials(
        &fixture.config,
        &control,
        &leaf,
        &leaf_key,
        &origin,
        &origin_key,
        &[Ipv4Addr::new(192, 0, 2, 1).into()],
        now.into(),
    )
    .err()
    .context(MissingValueSnafu {
        name: "missing SAN must be rejected by the facade",
    })?;
    assert_eq!(
        error.certificate_rejection(),
        Some(natsume_server::CertificateRejection::IpNotCovered)
    );
    assert!(!fixture.directory.path().join("natsume.db").exists());
    assert!(!fixture.directory.path().join("keys").exists());
    Ok(())
}

fn openssl(arguments: &[&str], input: Option<&[u8]>) -> Result<Vec<u8>, TestError> {
    use std::{
        io::Write as _,
        process::{Command, Stdio},
    };

    let mut child = Command::new("openssl")
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    if let Some(input) = input {
        child
            .stdin
            .take()
            .context(MissingValueSnafu {
                name: "child stdin is piped",
            })?
            .write_all(input)?;
    }
    let output = child.wait_with_output()?;
    snafu::ensure!(
        output.status.success(),
        OpenSslSnafu {
            status: output.status
        }
    );
    Ok(output.stdout)
}

#[test]
fn openssl_decrypts_export_and_imports_its_encrypted_pkcs8() -> Result<(), TestError> {
    use secrecy::{ExposeSecret as _, SecretString};

    let fixture = Fixture::new()?;
    let (certificate, key) = generate::generate_ca("Control CA", "Natsume", now())?;
    let passphrase = SecretString::from("openssl-stdin-passphrase-canary");
    let pem = generate::encrypt_private_key_pem(&key, &passphrase)?;
    let path = fixture.path("encrypted.pk8.pem");
    std::fs::write(&path, pem.as_bytes())?;
    let public = openssl(
        &[
            "pkey", "-in", &path, "-passin", "stdin", "-pubout", "-outform", "DER",
        ],
        Some(passphrase.expose_secret().as_bytes()),
    )?;
    assert_eq!(public, key.key_pair()?.subject_public_key_info());
    assert!(key.matches(&certificate));
    let external = openssl(
        &[
            "genpkey",
            "-algorithm",
            "EC",
            "-pkeyopt",
            "ec_paramgen_curve:P-256",
            "-aes-256-cbc",
            "-pass",
            "stdin",
        ],
        Some(passphrase.expose_secret().as_bytes()),
    )?;
    let imported = import_private_key(&external, Some(&passphrase))?;
    assert_eq!(imported.algorithm(), KeyAlgorithm::P256);
    let external_path = fixture.path("openssl-encrypted.pem");
    std::fs::write(&external_path, &external)?;
    let public = openssl(
        &[
            "pkey",
            "-in",
            &external_path,
            "-passin",
            "stdin",
            "-pubout",
            "-outform",
            "DER",
        ],
        Some(passphrase.expose_secret().as_bytes()),
    )?;
    assert_eq!(public, imported.key_pair()?.subject_public_key_info());
    Ok(())
}

#[test]
fn openssl_sec1_formats_and_rsa_pkcs1_rejection() -> Result<(), TestError> {
    use pkcs8::der::{Decode as _, Encode as _};

    let fixture = Fixture::new()?;
    for (curve, algorithm) in [
        ("prime256v1", KeyAlgorithm::P256),
        ("secp384r1", KeyAlgorithm::P384),
    ] {
        let path = fixture.path(&format!("{curve}.pem"));
        openssl(
            &[
                "ecparam", "-name", curve, "-genkey", "-noout", "-out", &path,
            ],
            None,
        )?;
        let pem = std::fs::read(&path)?;
        let der = openssl(&["ec", "-in", &path, "-outform", "DER"], None)?;
        let key = import_private_key(&pem, None)?;
        assert_eq!(key.algorithm(), algorithm);
        assert_eq!(import_private_key(&der, None)?.pkcs8_der(), key.pkcs8_der());
        let info = pkcs8::PrivateKeyInfoRef::from_der(key.pkcs8_der())?;
        assert_eq!(info.algorithm.oid, sec1::ALGORITHM_OID);
        assert!(info.algorithm.parameters.is_some());
        let external_public = openssl(&["pkey", "-in", &path, "-pubout", "-outform", "DER"], None)?;
        assert_eq!(external_public, key.key_pair()?.subject_public_key_info());
        let mut sec1 = sec1::EcPrivateKey::from_der(&der)?;
        sec1.public_key = None;
        let without_public = zeroize::Zeroizing::new(sec1.to_der()?);
        let Err(error) = import_private_key(&without_public, None) else {
            panic!("SEC1 key without a public key must be rejected: {curve}");
        };
        assert!(matches!(error, ImportError::KeyAlgorithm { .. }));
        assert!(
            error
                .source()
                .is_some_and(<dyn std::error::Error>::is::<rcgen::Error>)
        );
    }
    let scrypt_pem = openssl(
        &[
            "pkcs8",
            "-topk8",
            "-in",
            &fixture.path("prime256v1.pem"),
            "-scrypt",
            "-passout",
            "stdin",
        ],
        Some(b"scrypt-test-passphrase"),
    )?;
    assert_eq!(
        import_private_key(
            &scrypt_pem,
            Some(&secrecy::SecretString::from("scrypt-test-passphrase"))
        )?
        .algorithm(),
        KeyAlgorithm::P256
    );
    let rsa_path = fixture.path("rsa.pem");
    openssl(&["genrsa", "-traditional", "-out", &rsa_path, "2048"], None)?;
    let pem = std::fs::read(&rsa_path)?;
    let der = openssl(
        &["rsa", "-in", &rsa_path, "-traditional", "-outform", "DER"],
        None,
    )?;
    for bytes in [pem.as_slice(), der.as_slice()] {
        assert!(matches!(
            import_private_key(bytes, None),
            Err(ImportError::UnsupportedKey { .. })
        ));
    }
    Ok(())
}

#[test]
fn openssl_verifies_issued_leaf_for_each_ip() -> Result<(), TestError> {
    use time::Duration;

    let fixture = Fixture::new()?;
    let (ca, key) = generate::generate_ca("Control CA", "Natsume", now())?;
    let (leaf, _) =
        generate::issue_server_leaf(&ca, &key, &ips(), now() + Duration::days(180), now())?;
    let ca_path = fixture.path("ca.pem");
    let leaf_path = fixture.path("leaf.pem");
    std::fs::write(&ca_path, ca.to_pem())?;
    std::fs::write(&leaf_path, leaf.to_pem())?;
    let timestamp = now().unix_timestamp().to_string();
    for ip in ips() {
        openssl(
            &[
                "verify",
                "-CAfile",
                &ca_path,
                "-purpose",
                "sslserver",
                "-verify_ip",
                &ip.to_string(),
                "-attime",
                &timestamp,
                &leaf_path,
            ],
            None,
        )?;
    }
    Ok(())
}

#[test]
fn openssl_signs_csr_and_materials_pass_server_validation() -> Result<(), TestError> {
    let fixture = Fixture::new()?;
    let now = OffsetDateTime::now_utc();
    let (ca, ca_key) = generate::generate_ca("Control CA", "Natsume", now)?;
    let (origin, origin_key) = generate::generate_ca("Origin CA", "Natsume", now)?;
    let (leaf_key, request) = generate::create_csr(&ips())?;
    let ca_path = fixture.path("ca.pem");
    let key_path = fixture.path("ca-key.pk8");
    let request_path = fixture.path("request.pem");
    let extensions_path = fixture.path("extensions.cnf");
    let leaf_path = fixture.path("signed.pem");
    std::fs::write(&ca_path, ca.to_pem())?;
    std::fs::write(&key_path, ca_key.pkcs8_der())?;
    std::fs::write(&request_path, request.pem())?;
    std::fs::write(
        &extensions_path,
        "basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=serverAuth\nsubjectAltName=IP:127.0.0.1,IP:::1\nsubjectKeyIdentifier=hash\nauthorityKeyIdentifier=keyid\n",
    )?;
    openssl(
        &[
            "x509",
            "-req",
            "-in",
            &request_path,
            "-CA",
            &ca_path,
            "-CAkey",
            &key_path,
            "-CAkeyform",
            "DER",
            "-set_serial",
            "42",
            "-days",
            "3649",
            "-extfile",
            &extensions_path,
            "-out",
            &leaf_path,
        ],
        None,
    )?;
    let leaf = import_certificate(&std::fs::read(leaf_path)?)?;
    assert!(leaf_key.matches(&leaf));
    assert_eq!(leaf.ip_sans(), ips());
    super::validate::validate_materials(
        &fixture.config,
        &ca,
        &leaf,
        &leaf_key,
        &origin,
        &origin_key,
        &ips(),
        std::time::SystemTime::now(),
    )?;
    Ok(())
}
