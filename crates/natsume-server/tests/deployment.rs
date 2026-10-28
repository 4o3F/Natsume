#![cfg(target_os = "linux")]

use std::{
    error::Error as _,
    fs,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, TcpListener},
    os::unix::fs::PermissionsExt as _,
    time::SystemTime,
};

use diesel::{
    Connection as _, ExpressionMethods as _, RunQueryDsl as _, connection::SimpleConnection as _,
    sqlite::SqliteConnection,
};
use diesel_migrations::{EmbeddedMigrations, MigrationHarness as _, embed_migrations};
use uuid::Uuid;

#[path = "../diesel/schema.rs"]
mod schema;

use natsume_server::{
    BootstrapStatus, CertificateRejection, DeploymentError, DeploymentMaterials, MaterialRole,
    config::{ConfigError, ServerConfig},
    inspect_bootstrap, validate_deployment, validate_setup_materials,
};
use rcgen::{
    BasicConstraints, CertificateParams, CustomExtension, DistinguishedName, DnType,
    ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair, KeyUsagePurpose, PKCS_ECDSA_P256_SHA256,
    PKCS_ECDSA_P384_SHA384, SanType, SignatureAlgorithm,
};
use snafu::Snafu;
use tempfile::TempDir;
use time::{OffsetDateTime, macros::datetime};

const MIGRATIONS: EmbeddedMigrations = embed_migrations!("migrations");
const NOW: OffsetDateTime = datetime!(2029-01-01 00:00 UTC);
const GATEWAY_EXPIRY: OffsetDateTime = datetime!(2030-01-02 00:00 UTC);
const IPS: [IpAddr; 2] = [
    IpAddr::V4(Ipv4Addr::LOCALHOST),
    IpAddr::V6(Ipv6Addr::LOCALHOST),
];

struct Ca {
    der: Vec<u8>,
    key_der: Vec<u8>,
    issuer: Issuer<'static, KeyPair>,
}

impl Ca {
    fn new(
        params: CertificateParams,
        algorithm: &'static SignatureAlgorithm,
        parent: Option<&Self>,
    ) -> Result<Self, TestError> {
        let key = KeyPair::generate_for(algorithm)?;
        let certificate = match parent {
            Some(parent) => params.signed_by(&key, &parent.issuer)?,
            None => params.self_signed(&key)?,
        };
        Ok(Self {
            der: certificate.der().to_vec(),
            key_der: key.serialize_der(),
            issuer: Issuer::new(params, key),
        })
    }
}

fn ca_params(name: &str) -> CertificateParams {
    let mut distinguished_name = DistinguishedName::new();
    distinguished_name.push(DnType::CommonName, name);
    let mut params = CertificateParams::default();
    params.distinguished_name = distinguished_name;
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign];
    params.not_before = datetime!(2028-01-01 00:00 UTC);
    params.not_after = datetime!(2035-01-01 00:00 UTC);
    params
}

fn leaf_params() -> CertificateParams {
    let mut params = CertificateParams::default();
    params.is_ca = IsCa::ExplicitNoCa;
    params.subject_alt_names = IPS.into_iter().map(SanType::IpAddress).collect();
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    params.not_before = datetime!(2028-01-01 00:00 UTC);
    params.not_after = datetime!(2032-01-01 00:00 UTC);
    params
}

struct Fixture {
    directory: TempDir,
    config: ServerConfig,
    control: Ca,
    origin: Ca,
    leaf_key: KeyPair,
    leaf_key_der: Vec<u8>,
    leaf_der: Vec<u8>,
}

impl Fixture {
    fn new() -> Result<Self, TestError> {
        let directory = tempfile::tempdir()?;
        let root = directory.path().display();
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let address = listener.local_addr()?;
        let encoded = format!(
            r#"
[listen]
https = "{address}"
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
gateway_not_after = "2030-01-02T00:00:00Z"
contest_end = "2030-01-01T00:00:00Z"
[trust]
control_root = "{root}/absent-control-ca.crt"
local_origin_root = "{root}/local-origin-ca.crt"
[runtime]
domjudge_origin = "https://judge.example.test"
"#
        );
        let path = directory.path().join("config.toml");
        fs::write(&path, encoded)?;
        let config = ServerConfig::load_from(&path)?;
        let control = Ca::new(ca_params("control"), &PKCS_ECDSA_P256_SHA256, None)?;
        let origin = Ca::new(ca_params("origin"), &PKCS_ECDSA_P256_SHA256, None)?;
        let leaf_key = KeyPair::generate()?;
        let leaf_der = leaf_params()
            .signed_by(&leaf_key, &control.issuer)?
            .der()
            .to_vec();
        Ok(Self {
            directory,
            config,
            control,
            origin,
            leaf_key_der: leaf_key.serialize_der(),
            leaf_key,
            leaf_der,
        })
    }

    fn materials(&self) -> DeploymentMaterials<'_> {
        DeploymentMaterials {
            control_root_der: &self.control.der,
            tls_leaf_der: &self.leaf_der,
            tls_key_pkcs8_der: &self.leaf_key_der,
            origin_ca_der: &self.origin.der,
            origin_key_pkcs8_der: &self.origin.key_der,
            local_origin_root_der: &self.origin.der,
        }
    }

    fn validate(&self) -> Result<(), DeploymentError> {
        validate_setup_materials(&self.config, self.materials(), &IPS, SystemTime::from(NOW))
    }

    fn reissue_leaf(&mut self, params: &CertificateParams) -> Result<(), TestError> {
        self.leaf_der = params
            .signed_by(&self.leaf_key, &self.control.issuer)?
            .der()
            .to_vec();
        Ok(())
    }

    fn install(&self) -> Result<(), TestError> {
        let keys = self.directory.path().join("keys");
        fs::create_dir_all(&keys)?;
        fs::set_permissions(&keys, fs::Permissions::from_mode(0o700))?;
        for (name, contents) in [
            ("server-tls-leaf.der", &self.leaf_der),
            ("server-tls-key.pk8", &self.leaf_key_der),
            ("origin-ca.der", &self.origin.der),
            ("origin-ca-key.pk8", &self.origin.key_der),
        ] {
            let path = keys.join(name);
            fs::write(&path, contents)?;
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        }
        fs::write(
            self.directory.path().join("local-origin-ca.crt"),
            &self.origin.der,
        )?;
        Ok(())
    }
}

fn rejected(result: Result<(), DeploymentError>) -> DeploymentError {
    match result {
        Err(error) => error,
        Ok(()) => panic!("invalid deployment material must be rejected"),
    }
}

fn assert_rejected(fixture: &Fixture, role: MaterialRole, reason: CertificateRejection) {
    let error = rejected(fixture.validate());
    assert_eq!(error.material_role(), Some(role), "{error:?}");
    assert_eq!(error.certificate_rejection(), Some(reason), "{error:?}");
    if matches!(
        reason,
        CertificateRejection::IpNotCovered
            | CertificateRejection::UntrustedIssuer
            | CertificateRejection::LeafNotCurrentlyValid
            | CertificateRejection::NotServerAuth
    ) {
        assert!(
            error
                .source()
                .and_then(|source| source.source())
                .and_then(|source| source.downcast_ref::<rustls::Error>())
                .is_some(),
            "leaf verification source was lost: {error:?}"
        );
    }
}

#[test]
fn valid_materials_cover_one_or_multiple_ips_without_io() -> Result<(), TestError> {
    let fixture = Fixture::new()?;
    for ips in [&IPS[..1], &IPS[..]] {
        validate_setup_materials(&fixture.config, fixture.materials(), ips, NOW.into())?;
    }
    assert!(!fixture.directory.path().join("keys").exists());
    assert!(!fixture.directory.path().join("natsume.db").exists());
    Ok(())
}

#[test]
fn runtime_file_validation_needs_no_control_root_database_or_vault() -> Result<(), TestError> {
    let fixture = Fixture::new()?;
    fixture.install()?;
    let path = fixture.directory.path().join("config.toml");
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let encoded = fs::read_to_string(&path)?;
    let encoded = encoded
        .lines()
        .map(|line| {
            if line.starts_with("https =") {
                listener
                    .local_addr()
                    .map(|address| format!("https = \"{address}\""))
            } else {
                Ok(line.to_owned())
            }
        })
        .collect::<Result<Vec<_>, _>>()?
        .join("\n");
    fs::write(&path, encoded)?;
    validate_deployment(&ServerConfig::load_from(&path)?)?;
    assert!(!fixture.directory.path().join("natsume.db").exists());
    assert!(
        !fixture
            .directory
            .path()
            .join("keys/server-root.key")
            .exists()
    );
    assert!(
        !fixture
            .directory
            .path()
            .join("absent-control-ca.crt")
            .exists()
    );
    Ok(())
}

#[test]
fn missing_ip_wrong_issuer_and_intermediate_chain_are_rejected() -> Result<(), TestError> {
    let mut fixture = Fixture::new()?;
    let mut params = leaf_params();
    params.subject_alt_names = vec![SanType::IpAddress(IPS[0])];
    fixture.reissue_leaf(&params)?;
    assert_rejected(
        &fixture,
        MaterialRole::TlsLeaf,
        CertificateRejection::IpNotCovered,
    );

    for ca in [
        Ca::new(ca_params("other"), &PKCS_ECDSA_P256_SHA256, None)?,
        Ca::new(ca_params("control"), &PKCS_ECDSA_P256_SHA256, None)?,
        Ca::new(
            ca_params("intermediate"),
            &PKCS_ECDSA_P256_SHA256,
            Some(&fixture.control),
        )?,
    ] {
        fixture.leaf_der = leaf_params()
            .signed_by(&fixture.leaf_key, &ca.issuer)?
            .der()
            .to_vec();
        assert_rejected(
            &fixture,
            MaterialRole::TlsLeaf,
            CertificateRejection::UntrustedIssuer,
        );
    }
    Ok(())
}

#[test]
fn non_self_signed_control_ca_is_a_direct_trust_anchor() -> Result<(), TestError> {
    let mut fixture = Fixture::new()?;
    let parent = Ca::new(ca_params("external parent"), &PKCS_ECDSA_P256_SHA256, None)?;
    fixture.control = Ca::new(
        ca_params("imported anchor"),
        &PKCS_ECDSA_P256_SHA256,
        Some(&parent),
    )?;
    fixture.reissue_leaf(&leaf_params())?;
    fixture.validate()?;
    Ok(())
}

#[test]
fn strict_origin_policy_preserves_runtime_acceptance() -> Result<(), TestError> {
    let mut fixture = Fixture::new()?;
    let mut missing_usage = ca_params("origin without usage");
    missing_usage.key_usages.clear();
    let mut short_validity = ca_params("short origin");
    short_validity.not_after = GATEWAY_EXPIRY - time::Duration::seconds(1);
    for (origin, reason) in [
        (
            Ca::new(ca_params("P-384 origin"), &PKCS_ECDSA_P384_SHA384, None)?,
            CertificateRejection::NotP256,
        ),
        (
            Ca::new(missing_usage, &PKCS_ECDSA_P256_SHA256, None)?,
            CertificateRejection::MissingKeyCertSign,
        ),
        (
            Ca::new(short_validity, &PKCS_ECDSA_P256_SHA256, None)?,
            CertificateRejection::InsufficientValidity,
        ),
    ] {
        fixture.origin = origin;
        fixture.install()?;
        validate_deployment(&fixture.config)?;
        assert_rejected(&fixture, MaterialRole::OriginCa, reason);
    }
    Ok(())
}

#[test]
fn ca_constraints_and_explicit_key_usage_are_required() -> Result<(), TestError> {
    let mut fixture = Fixture::new()?;
    for (is_ca, usages, reason) in [
        (
            IsCa::ExplicitNoCa,
            vec![KeyUsagePurpose::KeyCertSign],
            CertificateRejection::NotCa,
        ),
        (
            IsCa::Ca(BasicConstraints::Unconstrained),
            vec![],
            CertificateRejection::MissingKeyCertSign,
        ),
        (
            IsCa::Ca(BasicConstraints::Unconstrained),
            vec![KeyUsagePurpose::DigitalSignature],
            CertificateRejection::MissingKeyCertSign,
        ),
    ] {
        let mut params = ca_params("invalid control");
        params.is_ca = is_ca;
        params.key_usages = usages;
        fixture.control = Ca::new(params, &PKCS_ECDSA_P256_SHA256, None)?;
        fixture.reissue_leaf(&leaf_params())?;
        assert_rejected(&fixture, MaterialRole::ControlCa, reason);
    }
    let mut params = ca_params("non-CA origin");
    params.is_ca = IsCa::ExplicitNoCa;
    fixture.origin = Ca::new(params, &PKCS_ECDSA_P256_SHA256, None)?;
    assert_rejected(
        &fixture,
        MaterialRole::OriginCa,
        CertificateRejection::InvalidIdentity,
    );
    Ok(())
}

#[test]
fn duplicate_ca_extensions_preserve_parser_errors() -> Result<(), TestError> {
    for (oid, contents) in [
        (&[2, 5, 29, 19][..], vec![0x30, 0x03, 0x01, 0x01, 0xff]),
        (&[2, 5, 29, 15][..], vec![0x03, 0x02, 0x01, 0x06]),
    ] {
        let mut fixture = Fixture::new()?;
        let mut params = ca_params("control with duplicate extension");
        params
            .custom_extensions
            .push(CustomExtension::from_oid_content(oid, contents));
        fixture.control = Ca::new(params, &PKCS_ECDSA_P256_SHA256, None)?;
        fixture.reissue_leaf(&leaf_params())?;
        let error = rejected(fixture.validate());
        assert_eq!(error.material_role(), Some(MaterialRole::ControlCa));
        assert_eq!(
            error.certificate_rejection(),
            Some(CertificateRejection::InvalidDer)
        );
        assert!(
            error
                .source()
                .and_then(|source| source.source())
                .and_then(|source| source.downcast_ref::<x509_parser::error::X509Error>())
                .is_some(),
            "extension parser source was lost: {error:?}"
        );
    }
    Ok(())
}

#[test]
fn unsupported_validation_times_preserve_conversion_errors() -> Result<(), TestError> {
    let fixture = Fixture::new()?;
    for now in [
        SystemTime::UNIX_EPOCH - std::time::Duration::from_secs(1),
        SystemTime::UNIX_EPOCH + std::time::Duration::from_hours(70_389_528),
    ] {
        let error = rejected(validate_setup_materials(
            &fixture.config,
            fixture.materials(),
            &IPS,
            now,
        ));
        let source = error.source().and_then(|source| source.source());
        if now < SystemTime::UNIX_EPOCH {
            assert!(matches!(source, Some(source) if source.is::<std::time::SystemTimeError>()));
        } else {
            assert!(matches!(source, Some(source) if source.is::<time::error::ComponentRange>()));
        }
        assert_eq!(error.certificate_rejection(), None);
        assert_eq!(error.material_role(), None);
    }
    Ok(())
}

#[test]
fn mismatched_origin_root_and_tls_key_are_rejected() -> Result<(), TestError> {
    let fixture = Fixture::new()?;
    let mut materials = fixture.materials();
    materials.local_origin_root_der = &fixture.control.der;
    let error = rejected(validate_setup_materials(
        &fixture.config,
        materials,
        &IPS,
        NOW.into(),
    ));
    assert_eq!(
        error.certificate_rejection(),
        Some(CertificateRejection::TrustRootMismatch)
    );
    let mut materials = fixture.materials();
    materials.tls_key_pkcs8_der = &fixture.origin.key_der;
    let error = rejected(validate_setup_materials(
        &fixture.config,
        materials,
        &IPS,
        NOW.into(),
    ));
    assert_eq!(
        error.certificate_rejection(),
        Some(CertificateRejection::InvalidIdentity)
    );
    Ok(())
}

#[test]
fn leaf_expiry_must_cover_gateway_and_not_outlive_control() -> Result<(), TestError> {
    let mut fixture = Fixture::new()?;
    let mut params = leaf_params();
    params.not_after = GATEWAY_EXPIRY - time::Duration::seconds(1);
    fixture.reissue_leaf(&params)?;
    assert_rejected(
        &fixture,
        MaterialRole::TlsLeaf,
        CertificateRejection::InsufficientValidity,
    );
    params.not_after = GATEWAY_EXPIRY;
    fixture.reissue_leaf(&params)?;
    fixture.validate()?;
    params.not_after = datetime!(2036-01-01 00:00 UTC);
    fixture.reissue_leaf(&params)?;
    assert_rejected(
        &fixture,
        MaterialRole::TlsLeaf,
        CertificateRejection::ExceedsControlCaValidity,
    );
    Ok(())
}

#[test]
fn leaf_current_validity_and_server_auth_are_verified() -> Result<(), TestError> {
    let mut fixture = Fixture::new()?;
    let mut expired = leaf_params();
    expired.not_after = NOW - time::Duration::seconds(1);
    let mut future = leaf_params();
    future.not_before = NOW + time::Duration::seconds(1);
    let mut client_auth = leaf_params();
    client_auth.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
    for (params, reason) in [
        (expired, CertificateRejection::LeafNotCurrentlyValid),
        (future, CertificateRejection::LeafNotCurrentlyValid),
        (client_auth, CertificateRejection::NotServerAuth),
    ] {
        fixture.reissue_leaf(&params)?;
        assert_rejected(&fixture, MaterialRole::TlsLeaf, reason);
    }
    Ok(())
}

#[test]
fn both_ca_certificates_must_be_currently_valid() -> Result<(), TestError> {
    for role in [MaterialRole::ControlCa, MaterialRole::OriginCa] {
        for (not_before, not_after) in [
            (
                datetime!(2028-01-01 00:00 UTC),
                NOW - time::Duration::seconds(1),
            ),
            (
                NOW + time::Duration::seconds(1),
                datetime!(2035-01-01 00:00 UTC),
            ),
        ] {
            let mut fixture = Fixture::new()?;
            let mut params = ca_params("noncurrent CA");
            params.not_before = not_before;
            params.not_after = not_after;
            let ca = Ca::new(params, &PKCS_ECDSA_P256_SHA256, None)?;
            if role == MaterialRole::ControlCa {
                fixture.control = ca;
                let mut leaf = leaf_params();
                leaf.not_after = not_after;
                fixture.reissue_leaf(&leaf)?;
                assert!(
                    fixture.validate().is_err(),
                    "noncurrent Control CA must fail"
                );
            } else {
                fixture.origin = ca;
                assert_rejected(&fixture, role, CertificateRejection::NotCurrentlyValid);
            }
        }
    }
    Ok(())
}

#[test]
fn single_complete_der_certificates_are_required() -> Result<(), TestError> {
    let fixture = Fixture::new()?;
    for role in [
        MaterialRole::ControlCa,
        MaterialRole::TlsLeaf,
        MaterialRole::OriginCa,
        MaterialRole::LocalOriginRoot,
    ] {
        for suffix in [&b"trailing-canary"[..], &fixture.control.der[..]] {
            let original = match role {
                MaterialRole::ControlCa => &fixture.control.der,
                MaterialRole::TlsLeaf => &fixture.leaf_der,
                _ => &fixture.origin.der,
            };
            let mut invalid = original.clone();
            invalid.extend_from_slice(suffix);
            let mut materials = fixture.materials();
            match role {
                MaterialRole::ControlCa => materials.control_root_der = &invalid,
                MaterialRole::TlsLeaf => materials.tls_leaf_der = &invalid,
                MaterialRole::OriginCa => {
                    materials.origin_ca_der = &invalid;
                    materials.local_origin_root_der = &invalid;
                }
                _ => materials.local_origin_root_der = &invalid,
            }
            assert!(
                validate_setup_materials(&fixture.config, materials, &IPS, NOW.into()).is_err(),
                "{role:?} suffix accepted"
            );
        }
    }
    Ok(())
}

#[test]
fn empty_ips_and_past_gateway_expiry_are_rejected() -> Result<(), TestError> {
    let fixture = Fixture::new()?;
    assert!(
        validate_setup_materials(&fixture.config, fixture.materials(), &[], NOW.into()).is_err()
    );
    assert!(
        validate_setup_materials(
            &fixture.config,
            fixture.materials(),
            &IPS,
            GATEWAY_EXPIRY.into()
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn materials_and_errors_redact_all_input_bytes() -> Result<(), TestError> {
    let fixture = Fixture::new()?;
    let canary = b"SECRET-CERTIFICATE-KEY-CANARY";
    let materials = DeploymentMaterials {
        control_root_der: canary,
        tls_leaf_der: canary,
        tls_key_pkcs8_der: canary,
        origin_ca_der: canary,
        origin_key_pkcs8_der: canary,
        local_origin_root_der: canary,
    };
    assert_eq!(
        format!("{materials:?}"),
        "DeploymentMaterials { <redacted> }"
    );
    let error = rejected(validate_setup_materials(
        &fixture.config,
        materials,
        &IPS,
        NOW.into(),
    ));
    for message in [
        error.to_string(),
        format!("{error:?}"),
        snafu::Report::from_error(&error).to_string(),
    ] {
        assert!(!message.contains("CANARY"), "secret exposed in diagnostics");
    }
    Ok(())
}

#[tokio::test]
async fn missing_database_is_not_created_and_empty_database_is_not_migrated()
-> Result<(), TestError> {
    let fixture = Fixture::new()?;
    let result = inspect_bootstrap(&fixture.config).await?;
    assert_eq!(result, BootstrapStatus::DatabaseMissing);
    let path = fixture.directory.path().join("natsume.db");
    assert!(!path.exists());
    fs::write(&path, [])?;
    let before = fs::read(&path)?;
    let result = inspect_bootstrap(&fixture.config).await?;
    assert_eq!(result, BootstrapStatus::NeedsBootstrap);
    assert_eq!(
        fs::read(path)?,
        before,
        "inspection modified empty database"
    );
    Ok(())
}

#[tokio::test]
async fn migrated_bootstrap_state_is_read_without_writes() -> Result<(), TestError> {
    use schema::operator_accounts;

    for initialized in [false, true] {
        let fixture = Fixture::new()?;
        let mut connection = SqliteConnection::establish(
            &fixture
                .directory
                .path()
                .join("natsume.db")
                .to_string_lossy(),
        )?;
        connection
            .run_pending_migrations(MIGRATIONS)
            .map_err(std::io::Error::other)?;
        if initialized {
            diesel::insert_into(operator_accounts::table)
                .values((
                    operator_accounts::operator_id.eq(Uuid::now_v7().to_string()),
                    operator_accounts::username.eq("admin"),
                    operator_accounts::role.eq("admin"),
                    operator_accounts::password_hash.eq("fixture-password-hash"),
                    operator_accounts::credential_revision.eq(1_i64),
                ))
                .execute(&mut connection)?;
        }
        drop(connection);
        let before = fs::read(fixture.directory.path().join("natsume.db"))?;
        let inspection = inspect_bootstrap(&fixture.config).await?;
        assert_eq!(
            inspection,
            if initialized {
                BootstrapStatus::Initialized
            } else {
                BootstrapStatus::NeedsBootstrap
            }
        );
        assert_eq!(
            fs::read(fixture.directory.path().join("natsume.db"))?,
            before
        );
        assert!(!fixture.directory.path().join("keys").exists());
    }
    Ok(())
}

#[tokio::test]
async fn unknown_incomplete_and_corrupt_databases_are_rejected() -> Result<(), TestError> {
    for sql in [
        "CREATE TABLE unknown_table (value TEXT)",
        "CREATE TABLE __diesel_schema_migrations (version TEXT); CREATE TABLE unknown_table (value TEXT)",
        "CREATE TABLE __diesel_schema_migrations (version TEXT); INSERT INTO __diesel_schema_migrations VALUES ('unknown')",
    ] {
        let fixture = Fixture::new()?;
        let mut connection = SqliteConnection::establish(
            &fixture
                .directory
                .path()
                .join("natsume.db")
                .to_string_lossy(),
        )?;
        connection.batch_execute(sql)?;
        drop(connection);
        let before = fs::read(fixture.directory.path().join("natsume.db"))?;
        assert!(
            inspect_bootstrap(&fixture.config).await.is_err(),
            "unknown schema accepted"
        );
        assert_eq!(
            fs::read(fixture.directory.path().join("natsume.db"))?,
            before
        );
    }
    for damage in [
        "DROP TABLE devices",
        "DELETE FROM __diesel_schema_migrations WHERE version = (SELECT MAX(version) FROM __diesel_schema_migrations)",
    ] {
        let fixture = Fixture::new()?;
        let mut connection = SqliteConnection::establish(
            &fixture
                .directory
                .path()
                .join("natsume.db")
                .to_string_lossy(),
        )?;
        connection
            .run_pending_migrations(MIGRATIONS)
            .map_err(std::io::Error::other)?;
        connection.batch_execute(damage)?;
        drop(connection);
        let before = fs::read(fixture.directory.path().join("natsume.db"))?;
        assert!(
            inspect_bootstrap(&fixture.config).await.is_err(),
            "damaged schema accepted: {damage}"
        );
        assert_eq!(
            fs::read(fixture.directory.path().join("natsume.db"))?,
            before
        );
    }
    let corrupt = Fixture::new()?;
    fs::write(
        corrupt.directory.path().join("natsume.db"),
        b"corrupt-database-canary",
    )?;
    assert!(
        inspect_bootstrap(&corrupt.config).await.is_err(),
        "corrupt database accepted"
    );
    assert_eq!(
        fs::read(corrupt.directory.path().join("natsume.db"))?,
        b"corrupt-database-canary"
    );
    Ok(())
}

#[derive(Debug, Snafu)]
enum TestError {
    #[snafu(context(false), display("deployment fixture connection failed"))]
    Connection { source: diesel::ConnectionError },
    #[snafu(context(false), display("deployment fixture query failed"))]
    Query { source: diesel::result::Error },
    #[snafu(context(false), display("deployment fixture filesystem failed"))]
    Io { source: std::io::Error },
    #[snafu(context(false), display("deployment fixture configuration failed"))]
    Config { source: ConfigError },
    #[snafu(
        context(false),
        display("deployment fixture certificate generation failed")
    )]
    Certificate { source: rcgen::Error },
    #[snafu(context(false), display("deployment validation failed"))]
    Deployment { source: DeploymentError },
}
