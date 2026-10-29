mod error;
mod materials;

use std::{net::IpAddr, time::SystemTime};

use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use snafu::ResultExt as _;

use crate::{
    component::{gateway::GatewayComponent, operator::OperatorComponent},
    config::ServerConfig,
    db::{Database, SchemaState, TransactionError},
    tls,
};

pub use error::{CertificateRejection, DeploymentError, MaterialRole};
pub use materials::DeploymentMaterials;

use error::DeploymentFailure;

/// The bootstrap state observed without migrating or creating a database.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootstrapStatus {
    /// The configured database path does not exist.
    DatabaseMissing,
    /// The database is empty or has no operator account.
    NeedsBootstrap,
    /// The migrated database contains an operator account.
    Initialized,
}

/// Loads deployed TLS and Origin CA files using the runtime's identity checks.
///
/// Reads identity files and the packaged Origin trust root only. Does not read the
/// Control CA, open the database or vault, bind sockets, or start background tasks.
/// This entry point does not apply setup's stricter certificate policy.
///
/// # Errors
///
/// Returns a redacted error when file permissions, identity loading, key matching,
/// or the packaged Origin trust root fail the runtime checks.
pub fn validate_deployment(config: &ServerConfig) -> Result<(), DeploymentError> {
    let validate = || -> Result<(), DeploymentFailure> {
        let certificate =
            tls::read_private_file(config.tls_certificate_path()).context(error::TlsSnafu)?;
        let key = tls::read_private_file(config.tls_private_key_path()).context(error::TlsSnafu)?;
        tls::server_config_from_der(
            CertificateDer::from(certificate),
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key)),
        )
        .context(error::TlsSnafu)?;
        GatewayComponent::validate_files(
            &config
                .origin_ca_certificate_path()
                .context(error::ConfigurationSnafu)?,
            &config
                .origin_ca_private_key_path()
                .context(error::ConfigurationSnafu)?,
            config.local_origin_root_path(),
            config.site(),
        )
        .context(error::OriginCaSnafu)
    };
    validate().map_err(DeploymentError::from)
}

/// Applies setup's strict certificate policy to borrowed materials without I/O.
///
/// Runs runtime identity checks first, then validates complete single-certificate
/// DER, CA constraints, P-256 Origin keys, identical Origin trust material, and a
/// direct Server TLS chain from the sole Control trust anchor for every supplied
/// IP at `now`. Non-self-signed Control anchors are accepted. Certificates must
/// cover the configured Gateway expiry, which must be in the future; the leaf
/// must not outlive its Control CA. No files, database, vault, or sockets are opened.
///
/// # Errors
///
/// Returns a redacted error for invalid identities, certificate policy failures,
/// an empty IP list, or an unsupported validation time.
pub fn validate_setup_materials(
    config: &ServerConfig,
    materials: DeploymentMaterials<'_>,
    server_ips: &[IpAddr],
    now: SystemTime,
) -> Result<(), DeploymentError> {
    materials::validate(config, materials, server_ips, now).map_err(DeploymentError::from)
}

/// Inspects first-install bootstrap state in one read-only database transaction.
///
/// Does not create a database or apply migrations. A first-install retry after a
/// partial failure can use `Initialized` to skip an already completed bootstrap.
/// Does not open the vault, load certificates, bind sockets, or start background tasks.
///
/// # Errors
///
/// Returns a redacted error for inaccessible, corrupt, or unrecognized databases,
/// mismatched migration versions, or persistence failures.
pub async fn inspect_bootstrap(config: &ServerConfig) -> Result<BootstrapStatus, DeploymentError> {
    let database =
        Database::connect_read_only(config.database_path()).context(error::DatabaseSnafu)?;
    let Some(database) = database else {
        return Ok(BootstrapStatus::DatabaseMissing);
    };
    database
        .read(
            |transaction| -> Result<BootstrapStatus, DeploymentFailure> {
                if transaction.schema_state()? == SchemaState::Empty {
                    return Ok(BootstrapStatus::NeedsBootstrap);
                }
                let initialized =
                    OperatorComponent::is_initialized(transaction).context(error::OperatorSnafu)?;
                Ok(if initialized {
                    BootstrapStatus::Initialized
                } else {
                    BootstrapStatus::NeedsBootstrap
                })
            },
        )
        .await
        .map_err(TransactionError::into_error)
        .map_err(DeploymentError::from)
}

#[cfg(test)]
mod tests {
    use std::{fs, net::Ipv4Addr};

    use rcgen::{
        BasicConstraints, CertificateParams, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
        KeyUsagePurpose, PKCS_ECDSA_P256_SHA256, PKCS_ECDSA_P384_SHA384, SanType,
    };
    use snafu::Snafu;
    use tempfile::TempDir;
    use time::macros::datetime;

    use crate::db::DatabaseConfig;

    use super::*;

    struct Fixture {
        directory: TempDir,
        config: ServerConfig,
    }

    impl Fixture {
        fn new() -> Result<Self, TestError> {
            let directory = tempfile::tempdir()?;
            let root = directory.path().display();
            let path = directory.path().join("config.toml");
            fs::write(
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
gateway_not_after = "2030-01-02T00:00:00Z"
contest_end = "2030-01-01T00:00:00Z"
[trust]
control_root = "{root}/control-ca.crt"
local_origin_root = "{root}/local-origin-ca.crt"
[runtime]
domjudge_origin = "https://judge.example.test"
"#
                ),
            )?;
            Ok(Self {
                directory,
                config: ServerConfig::load_from(&path)?,
            })
        }
    }

    #[tokio::test]
    async fn migrated_bootstrap_state_is_read_without_writes() -> Result<(), TestError> {
        let fixture = Fixture::new()?;
        let database = Database::connect_and_migrate(&DatabaseConfig::new(
            fixture.config.database_path(),
            true,
        ))
        .await?;
        let inspection = inspect_bootstrap(&fixture.config).await?;
        assert_eq!(inspection, BootstrapStatus::NeedsBootstrap);
        database
            .write(|transaction| -> Result<(), crate::test_error::TestError> {
                OperatorComponent::create_first_admin(
                    transaction,
                    "admin",
                    "fixture-password-hash",
                )?;
                Ok(())
            })
            .await
            .map_err(TransactionError::into_error)?;
        let inspection = inspect_bootstrap(&fixture.config).await?;
        assert_eq!(inspection, BootstrapStatus::Initialized);
        assert!(!fixture.directory.path().join("keys").exists());
        Ok(())
    }

    #[test]
    fn gateway_der_bridge_keeps_permissive_runtime_policy() -> Result<(), TestError> {
        let fixture = Fixture::new()?;
        let mut ca_params = CertificateParams::default();
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca_params.key_usages = vec![KeyUsagePurpose::KeyCertSign];
        let control_key = KeyPair::generate()?;
        let control = ca_params.self_signed(&control_key)?;
        let issuer = Issuer::from_params(&ca_params, &control_key);
        let leaf_key = KeyPair::generate()?;
        let mut leaf_params = CertificateParams::default();
        leaf_params.subject_alt_names = vec![SanType::IpAddress(Ipv4Addr::LOCALHOST.into())];
        leaf_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let leaf = leaf_params.signed_by(&leaf_key, &issuer)?;
        let leaf_key_der = leaf_key.serialize_der();
        for (algorithm, usages, reason) in [
            (
                &PKCS_ECDSA_P384_SHA384,
                vec![KeyUsagePurpose::KeyCertSign],
                CertificateRejection::NotP256,
            ),
            (
                &PKCS_ECDSA_P256_SHA256,
                vec![],
                CertificateRejection::MissingKeyCertSign,
            ),
        ] {
            let key = KeyPair::generate_for(algorithm)?;
            let mut params = ca_params.clone();
            params.key_usages = usages;
            let ca = params.self_signed(&key)?;
            let key_der = key.serialize_der();
            GatewayComponent::validate_der(
                ca.der(),
                &PrivatePkcs8KeyDer::from(key_der.as_slice()),
                ca.der(),
                fixture.config.site(),
            )
            .context(RuntimeRegressionSnafu)?;
            let result = validate_setup_materials(
                &fixture.config,
                DeploymentMaterials {
                    control_root_der: control.der(),
                    tls_leaf_der: leaf.der(),
                    tls_key_pkcs8_der: &leaf_key_der,
                    origin_ca_der: ca.der(),
                    origin_key_pkcs8_der: &key_der,
                    local_origin_root_der: ca.der(),
                },
                &[Ipv4Addr::LOCALHOST.into()],
                datetime!(2029-01-01 00:00 UTC).into(),
            );
            let Err(error) = result else {
                return Err(TestError::StrictRegression);
            };
            assert_eq!(error.certificate_rejection(), Some(reason), "{error:?}");
        }
        Ok(())
    }

    #[derive(Debug, Snafu)]
    enum TestError {
        #[snafu(context(false), display("deployment fixture filesystem failed"))]
        Io { source: std::io::Error },
        #[snafu(context(false), display("deployment fixture configuration failed"))]
        Config { source: crate::config::ConfigError },
        #[snafu(context(false), display("deployment fixture database failed"))]
        Database { source: crate::db::DatabaseError },
        #[snafu(context(false), display("deployment fixture connection failed"))]
        Connection { source: diesel::ConnectionError },
        #[snafu(context(false), display("deployment fixture query failed"))]
        Query { source: diesel::result::Error },
        #[snafu(context(false), display("deployment fixture operation failed"))]
        Operation {
            source: crate::test_error::TestError,
        },
        #[snafu(context(false), display("deployment fixture certificate failed"))]
        Certificate { source: rcgen::Error },
        #[snafu(context(false), display("deployment inspection failed"))]
        Deployment { source: DeploymentError },
        #[snafu(display("runtime rejected previously accepted material"))]
        RuntimeRegression {
            source: crate::component::gateway::GatewayLoadError,
        },
        #[snafu(display("strict policy accepted invalid material"))]
        StrictRegression,
    }
}
