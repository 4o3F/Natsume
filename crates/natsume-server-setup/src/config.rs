//! Server configuration rendering for first-install setup.

use std::{
    fs,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
};

pub use natsume_server::config::CONFIG_PATH;
use natsume_server::config::{ConfigError, ServerConfig};
use snafu::{OptionExt, ResultExt, Snafu, ensure};
use time::{Duration, OffsetDateTime, format_description::well_known::Rfc3339};
use toml::{Table, Value};

/// Administrator-owned PKI directory used when no override is supplied.
pub const DEFAULT_PKI_DIRECTORY: &str = "/root/natsume-pki";
/// Server executable installed by the Deb package.
pub const SERVER_BINARY: &str = "/usr/bin/natsume-server";
/// Systemd unit installed by the Server package.
pub const SERVICE_UNIT: &str = "natsume-server.service";

/// Existing system configuration directory required before setup.
pub const SYSTEM_CONFIG_DIRECTORY: &str = "/etc";
/// Root-owned directory containing Server deployment configuration.
pub const CONFIG_DIRECTORY: &str = "/etc/natsume-server";
/// Root-owned parent of the shared trust directory.
pub const TRUST_PARENT_DIRECTORY: &str = "/etc/natsume";
/// Root-owned directory containing public trust anchors.
pub const TRUST_DIRECTORY: &str = "/etc/natsume/trust";
/// Existing service state directory supplied by the Server Deb.
pub const STATE_DIRECTORY: &str = "/var/lib/natsume-server";
/// Public Control CA trust anchor used by Server and Clients.
pub const CONTROL_CA_TRUST: &str = "/etc/natsume/trust/control-ca.crt";
/// Public Local Origin CA trust anchor used by Server and Clients.
pub const LOCAL_ORIGIN_TRUST: &str = "/etc/natsume/trust/local-origin-ca.crt";
/// Service-owned directory containing online private key material.
pub const KEYS_DIRECTORY: &str = "/var/lib/natsume-server/keys";
/// Online Local Origin CA certificate in DER format.
pub const ORIGIN_CA_DER: &str = "/var/lib/natsume-server/keys/origin-ca.der";
/// Online Local Origin CA private key in unencrypted PKCS#8 DER format.
pub const ORIGIN_CA_KEY: &str = "/var/lib/natsume-server/keys/origin-ca-key.pk8";
/// Server TLS leaf certificate in DER format.
pub const SERVER_TLS_LEAF: &str = "/var/lib/natsume-server/keys/server-tls-leaf.der";
/// Server TLS private key in unencrypted PKCS#8 DER format.
pub const SERVER_TLS_KEY: &str = "/var/lib/natsume-server/keys/server-tls-key.pk8";
/// Persistent Server database, excluded from deployment rollback.
pub const DATABASE: &str = "/var/lib/natsume-server/natsume.db";
/// Bootstrap-owned vault root key, excluded from deployment rollback.
pub const ROOT_KEY: &str = "/var/lib/natsume-server/keys/server-root.key";
/// Directory containing deployment-supplied organization logos.
pub const ORGANIZATION_LOGOS: &str = "/var/lib/natsume-server/organization-logos";
/// Directory containing persisted print documents.
pub const PRINT_DOCUMENTS: &str = "/var/lib/natsume-server/print-documents";

/// Validated first-install parameters retaining the deployer's timestamp spelling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeploymentParameters {
    /// One to sixteen usable Server addresses, deduplicated in input order.
    server_ips: Vec<IpAddr>,
    /// Unprivileged HTTPS listener port.
    https_port: u16,
    /// Gateway DNS hostname, subject to the Server loader's canonical validation.
    gateway_hostname: String,
    /// `DOMjudge` HTTPS origin, subject to the Server loader's canonical validation.
    domjudge_origin: String,
    /// Original strict UTC contest end timestamp.
    contest_end: String,
    /// Original strict UTC Gateway certificate expiry timestamp.
    gateway_not_after: String,
    /// Parsed Gateway expiry used for certificate validity checks.
    gateway_not_after_timestamp: OffsetDateTime,
}

impl DeploymentParameters {
    /// Checks deployment structure and validity coverage against the supplied time.
    ///
    /// The sixteen-address limit applies after deduplication. Hostname and origin
    /// semantics are checked by [`validate_server_config`].
    ///
    /// # Errors
    /// Returns a typed parameter rejection or a timestamp parser error with its source.
    pub fn new(
        server_ips: Vec<IpAddr>,
        https_port: u16,
        gateway_hostname: String,
        domjudge_origin: String,
        contest_end: String,
        gateway_not_after: String,
        now: OffsetDateTime,
    ) -> Result<Self, ConfigRenderError> {
        let mut unique_ips = Vec::new();
        for ip in server_ips {
            ensure!(
                !ip.is_unspecified() && !ip.is_multicast() && ip != IpAddr::V4(Ipv4Addr::BROADCAST),
                InvalidParameterSnafu {
                    reason: ParameterRejection::UnusableServerIp,
                }
            );
            if !unique_ips.contains(&ip) {
                unique_ips.push(ip);
            }
            ensure!(
                unique_ips.len() <= 16,
                InvalidParameterSnafu {
                    reason: ParameterRejection::TooManyServerIps,
                }
            );
        }
        ensure!(
            !unique_ips.is_empty(),
            InvalidParameterSnafu {
                reason: ParameterRejection::EmptyServerIps,
            }
        );
        ensure!(
            https_port >= 1024,
            InvalidParameterSnafu {
                reason: ParameterRejection::HttpsPortOutOfRange,
            }
        );
        ensure!(
            !gateway_hostname.trim().is_empty(),
            InvalidParameterSnafu {
                reason: ParameterRejection::EmptyGatewayHostname,
            }
        );
        ensure!(
            !domjudge_origin.trim().is_empty(),
            InvalidParameterSnafu {
                reason: ParameterRejection::EmptyDomjudgeOrigin,
            }
        );
        let contest_end_timestamp =
            parse_timestamp(&contest_end, ParameterRejection::ContestEndFormat)?;
        let gateway_not_after_timestamp = parse_timestamp(
            &gateway_not_after,
            ParameterRejection::GatewayNotAfterFormat,
        )?;
        let required_not_after = contest_end_timestamp
            .checked_add(Duration::days(1))
            .context(InvalidParameterSnafu {
                reason: ParameterRejection::GatewayValidityTooShort,
            })?;
        ensure!(
            gateway_not_after_timestamp >= required_not_after,
            InvalidParameterSnafu {
                reason: ParameterRejection::GatewayValidityTooShort,
            }
        );
        ensure!(
            gateway_not_after_timestamp > now,
            InvalidParameterSnafu {
                reason: ParameterRejection::GatewayExpired,
            }
        );
        Ok(Self {
            server_ips: unique_ips,
            https_port,
            gateway_hostname,
            domjudge_origin,
            contest_end,
            gateway_not_after,
            gateway_not_after_timestamp,
        })
    }

    /// Returns Server addresses in their original order, without duplicates.
    #[must_use]
    pub fn server_ips(&self) -> &[IpAddr] {
        &self.server_ips
    }

    /// Returns the validated HTTPS port.
    #[must_use]
    pub const fn https_port(&self) -> u16 {
        self.https_port
    }

    /// Returns the Gateway hostname awaiting authoritative Server validation.
    #[must_use]
    pub fn gateway_hostname(&self) -> &str {
        &self.gateway_hostname
    }

    /// Returns the `DOMjudge` origin awaiting authoritative Server validation.
    #[must_use]
    pub fn domjudge_origin(&self) -> &str {
        &self.domjudge_origin
    }

    /// Returns the original contest end timestamp spelling.
    #[must_use]
    pub fn contest_end(&self) -> &str {
        &self.contest_end
    }

    /// Returns the original Gateway expiry timestamp spelling.
    #[must_use]
    pub fn gateway_not_after(&self) -> &str {
        &self.gateway_not_after
    }

    /// Returns the parsed Gateway expiry instant for certificate validation.
    #[must_use]
    pub const fn gateway_not_after_timestamp(&self) -> OffsetDateTime {
        self.gateway_not_after_timestamp
    }

    /// Selects the D7 wildcard listener, using IPv6 when any Server address needs it.
    #[must_use]
    pub fn listen_address(&self) -> SocketAddr {
        let ip = if self.server_ips.iter().any(IpAddr::is_ipv6) {
            IpAddr::V6(Ipv6Addr::UNSPECIFIED)
        } else {
            IpAddr::V4(Ipv4Addr::UNSPECIFIED)
        };
        SocketAddr::new(ip, self.https_port)
    }
}

fn parse_timestamp(
    text: &str,
    reason: ParameterRejection,
) -> Result<OffsetDateTime, ConfigRenderError> {
    let bytes = text.as_bytes();
    let valid_length = bytes.len() == 20
        || ((22..=30).contains(&bytes.len())
            && bytes[19] == b'.'
            && bytes[20..bytes.len() - 1].iter().all(u8::is_ascii_digit));
    ensure!(
        valid_length
            && bytes[19..].last() == Some(&b'Z')
            && bytes[..19].iter().enumerate().all(|(index, byte)| {
                match index {
                    4 | 7 => *byte == b'-',
                    10 => *byte == b'T',
                    13 | 16 => *byte == b':',
                    _ => byte.is_ascii_digit(),
                }
            })
            && &bytes[17..19] != b"60",
        InvalidParameterSnafu { reason }
    );
    OffsetDateTime::parse(text, &Rfc3339).context(TimestampParseSnafu { reason })
}

/// Renders the non-secret Server configuration using the deployment's fixed paths.
///
/// # Errors
/// Returns a TOML serialization error, preserving its source.
pub fn render_server_config(params: &DeploymentParameters) -> Result<String, ConfigRenderError> {
    let config = Table::from_iter([
        (
            "listen".to_owned(),
            section([("https", params.listen_address().to_string().into())]),
        ),
        ("log".to_owned(), section([("level", "info".into())])),
        (
            "storage".to_owned(),
            section([
                ("database", DATABASE.into()),
                ("root_key", ROOT_KEY.into()),
                ("organization_logos", ORGANIZATION_LOGOS.into()),
                ("print_documents", PRINT_DOCUMENTS.into()),
            ]),
        ),
        (
            "tls".to_owned(),
            section([
                ("certificate", SERVER_TLS_LEAF.into()),
                ("private_key", SERVER_TLS_KEY.into()),
            ]),
        ),
        (
            "site".to_owned(),
            section([
                ("gateway_hostname", params.gateway_hostname().into()),
                ("gateway_not_after", params.gateway_not_after().into()),
                ("contest_end", params.contest_end().into()),
            ]),
        ),
        (
            "trust".to_owned(),
            section([
                ("control_root", CONTROL_CA_TRUST.into()),
                ("local_origin_root", LOCAL_ORIGIN_TRUST.into()),
            ]),
        ),
        (
            "runtime".to_owned(),
            section([("domjudge_origin", params.domjudge_origin().into())]),
        ),
    ]);
    toml::to_string_pretty(&config).context(SerializeSnafu)
}

/// Validates rendered text through the authoritative Server configuration loader.
///
/// # Errors
/// Returns temporary-file I/O errors or the original [`ConfigError`] as a source.
pub fn validate_server_config(text: &str) -> Result<ServerConfig, ConfigRenderError> {
    let directory = tempfile::tempdir().context(TemporaryDirectorySnafu)?;
    let path = directory.path().join("config.toml");
    fs::write(&path, text).context(TemporaryConfigWriteSnafu)?;
    ServerConfig::load_from(&path).context(ValidationSnafu)
}

/// Renders credential-free Client configuration for `/etc/natsume/config.toml`.
///
/// # Errors
/// Returns a TOML serialization error, preserving its source.
pub fn render_client_config(params: &DeploymentParameters) -> Result<String, ConfigRenderError> {
    let config = Table::from_iter([
        (
            "server".to_owned(),
            section([
                ("ip", params.server_ips()[0].to_string().into()),
                ("port", i64::from(params.https_port()).into()),
            ]),
        ),
        (
            "site".to_owned(),
            section([("gateway_hostname", params.gateway_hostname().into())]),
        ),
    ]);
    let encoded = toml::to_string_pretty(&config).context(SerializeSnafu)?;
    Ok(format!(
        "# Client /etc/natsume/config.toml; contains no credentials or private keys.\n{encoded}"
    ))
}

fn section<const N: usize>(entries: [(&str, Value); N]) -> Value {
    Value::Table(
        entries
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect(),
    )
}

/// Finite reasons why deployment parameters cannot be accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ParameterRejection {
    /// At least one Server IP is required.
    EmptyServerIps,
    /// More than sixteen distinct Server IPs were supplied.
    TooManyServerIps,
    /// Unspecified, multicast, and IPv4 broadcast addresses cannot identify Server.
    UnusableServerIp,
    /// HTTPS ports must be in 1024..=65535.
    HttpsPortOutOfRange,
    /// A Gateway hostname must be supplied.
    EmptyGatewayHostname,
    /// A `DOMjudge` origin must be supplied.
    EmptyDomjudgeOrigin,
    /// Contest end must use strict UTC timestamp syntax and a valid date.
    ContestEndFormat,
    /// Gateway expiry must use strict UTC timestamp syntax and a valid date.
    GatewayNotAfterFormat,
    /// Gateway validity must cover contest end plus 86400 seconds.
    GatewayValidityTooShort,
    /// Gateway expiry must be strictly later than the supplied current time.
    GatewayExpired,
}

/// Parameter, serialization, or authoritative Server validation failure.
#[derive(Debug, Snafu)]
#[non_exhaustive]
pub enum ConfigRenderError {
    /// Deployment input failed a structural or cross-field constraint.
    #[snafu(display("invalid deployment parameter: {reason:?}"))]
    InvalidParameter {
        /// Constraint rejected by parameter validation.
        reason: ParameterRejection,
    },
    /// A structurally valid UTC timestamp contained an invalid date or time.
    #[snafu(display("invalid deployment timestamp: {reason:?}"))]
    TimestampParse {
        /// Timestamp field rejected by the parser.
        reason: ParameterRejection,
        /// Original timestamp parsing cause.
        source: time::error::Parse,
    },
    /// Serializing the TOML table failed.
    #[snafu(display("cannot serialize deployment configuration"))]
    Serialize {
        /// Original TOML serialization cause.
        source: toml::ser::Error,
    },
    /// Creating an isolated validation directory failed.
    #[snafu(display("cannot create configuration validation directory"))]
    TemporaryDirectory {
        /// Original temporary-directory creation cause.
        source: std::io::Error,
    },
    /// Writing the temporary Server configuration failed.
    #[snafu(display("cannot write configuration for validation"))]
    TemporaryConfigWrite {
        /// Original temporary configuration write cause.
        source: std::io::Error,
    },
    /// The Server loader rejected the rendered configuration.
    #[snafu(display("Server configuration validation failed"))]
    Validation {
        /// Authoritative Server configuration rejection.
        source: ConfigError,
    },
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::*;

    const IPV4: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10));
    const IPV6: IpAddr = IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 0x10));

    struct Parameters {
        ips: Vec<IpAddr>,
        port: u16,
        hostname: String,
        origin: String,
        contest_end: String,
        gateway_not_after: String,
        now: OffsetDateTime,
    }

    impl Default for Parameters {
        fn default() -> Self {
            Self {
                ips: vec![IPV4],
                port: 8443,
                hostname: "gateway.contest.example".to_owned(),
                origin: "https://judge.contest.example".to_owned(),
                contest_end: "2029-12-31T00:00:00Z".to_owned(),
                gateway_not_after: "2030-01-01T00:00:00Z".to_owned(),
                now: OffsetDateTime::UNIX_EPOCH,
            }
        }
    }

    impl Parameters {
        fn build(self) -> Result<DeploymentParameters, ConfigRenderError> {
            DeploymentParameters::new(
                self.ips,
                self.port,
                self.hostname,
                self.origin,
                self.contest_end,
                self.gateway_not_after,
                self.now,
            )
        }
    }

    #[derive(Debug, Snafu)]
    enum TestError {
        #[snafu(context(false), display("configuration operation failed"))]
        Config { source: ConfigRenderError },
        #[snafu(context(false), display("test TOML could not be decoded"))]
        Decode { source: toml::de::Error },
        #[snafu(context(false), display("test timestamp could not be parsed"))]
        Timestamp { source: time::error::Parse },
    }

    fn assert_rejection(parameters: Parameters, expected: ParameterRejection) {
        let Err(error) = parameters.build() else {
            panic!("parameters requiring rejection {expected:?} were accepted");
        };
        assert!(
            matches!(error, ConfigRenderError::InvalidParameter { reason } if reason == expected),
            "expected {expected:?}, got {error:?}"
        );
    }

    fn assert_schema(actual: &Table, expected: &Table) {
        assert_eq!(
            actual.keys().collect::<Vec<_>>(),
            expected.keys().collect::<Vec<_>>(),
            "top-level sections changed"
        );
        for (name, expected_section) in expected {
            let (Some(actual_section), Some(expected_section)) =
                (actual[name].as_table(), expected_section.as_table())
            else {
                panic!("expected TOML tables for section {name}");
            };
            assert_eq!(
                actual_section.keys().collect::<Vec<_>>(),
                expected_section.keys().collect::<Vec<_>>(),
                "keys changed in section {name}"
            );
        }
    }

    #[test]
    fn rendered_server_matches_example_and_passes_loader() -> Result<(), TestError> {
        let params = Parameters::default().build()?;
        let text = render_server_config(&params)?;
        let actual: Table = toml::from_str(&text)?;
        let expected: Table = toml::from_str(include_str!(
            "../../../packaging/server/config.example.toml"
        ))?;
        assert_eq!(
            actual, expected,
            "rendered values must match deployment defaults"
        );
        validate_server_config(&text)?;
        assert_eq!(
            params.listen_address(),
            SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), 8443)
        );
        assert_eq!(
            params.gateway_hostname(),
            actual["site"]["gateway_hostname"]
                .as_str()
                .unwrap_or_default()
        );
        assert_eq!(
            params.domjudge_origin(),
            actual["runtime"]["domjudge_origin"]
                .as_str()
                .unwrap_or_default()
        );
        assert_eq!(
            params.gateway_not_after_timestamp(),
            OffsetDateTime::parse(params.gateway_not_after(), &Rfc3339)?
        );
        assert!(
            !text.contains("[journal]"),
            "journal is outside the v2 configuration"
        );
        Ok(())
    }

    #[test]
    fn listener_uses_ipv6_for_mixed_or_ipv6_only_addresses() -> Result<(), TestError> {
        for ips in [vec![IPV4, IPV6], vec![IPV6], vec![IPV6, IPV4]] {
            let params = Parameters {
                ips,
                ..Parameters::default()
            }
            .build()?;
            let text = render_server_config(&params)?;
            let table: Table = toml::from_str(&text)?;
            assert_eq!(
                params.listen_address(),
                SocketAddr::new(Ipv6Addr::UNSPECIFIED.into(), 8443)
            );
            assert_eq!(table["listen"]["https"].as_str(), Some("[::]:8443"));
            validate_server_config(&text)?;
        }
        Ok(())
    }

    #[test]
    fn rejects_empty_and_unusable_server_addresses() {
        assert_rejection(
            Parameters {
                ips: vec![],
                ..Parameters::default()
            },
            ParameterRejection::EmptyServerIps,
        );
        for ip in [
            IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            IpAddr::V6(Ipv6Addr::UNSPECIFIED),
            IpAddr::V4(Ipv4Addr::new(224, 0, 0, 1)),
            IpAddr::V6(Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 0, 1)),
            IpAddr::V4(Ipv4Addr::BROADCAST),
        ] {
            assert_rejection(
                Parameters {
                    ips: vec![IPV4, ip],
                    ..Parameters::default()
                },
                ParameterRejection::UnusableServerIp,
            );
        }
    }

    #[test]
    fn deduplicates_addresses_in_order_and_limits_distinct_ips() -> Result<(), TestError> {
        let params = Parameters {
            ips: vec![IPV6, IPV4, IPV6, IPV4],
            ..Parameters::default()
        }
        .build()?;
        assert_eq!(params.server_ips(), &[IPV6, IPV4]);
        let mut ips: Vec<_> = (1..=16)
            .map(|last| IpAddr::V4(Ipv4Addr::new(192, 0, 2, last)))
            .collect();
        ips.push(ips[0]);
        let params = Parameters {
            ips: ips.clone(),
            ..Parameters::default()
        }
        .build()?;
        assert_eq!(params.server_ips().len(), 16);
        ips.push(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 17)));
        assert_rejection(
            Parameters {
                ips,
                ..Parameters::default()
            },
            ParameterRejection::TooManyServerIps,
        );
        Ok(())
    }

    #[test]
    fn validates_port_boundaries() -> Result<(), TestError> {
        for port in [0, 80, 1023] {
            assert_rejection(
                Parameters {
                    port,
                    ..Parameters::default()
                },
                ParameterRejection::HttpsPortOutOfRange,
            );
        }
        for port in [1024, 65_535] {
            let params = Parameters {
                port,
                ..Parameters::default()
            }
            .build()?;
            assert_eq!(params.https_port(), port);
            validate_server_config(&render_server_config(&params)?)?;
        }
        Ok(())
    }

    #[test]
    fn rejects_empty_hostname_and_origin() {
        assert_rejection(
            Parameters {
                hostname: " \n".to_owned(),
                ..Parameters::default()
            },
            ParameterRejection::EmptyGatewayHostname,
        );
        assert_rejection(
            Parameters {
                origin: String::new(),
                ..Parameters::default()
            },
            ParameterRejection::EmptyDomjudgeOrigin,
        );
    }

    #[test]
    fn rejects_noncanonical_timestamp_shapes_for_both_fields() {
        for text in [
            "",
            "2030-01-01T00:00:00",
            "2030-01-01T00:00:00+00:00",
            "2030-01-01T00:00:00-05:00",
            "2030-01-01t00:00:00Z",
            "2030-01-01T00:00:00z",
            "2030-01-01T00:00:60Z",
            "2030-01-01T00:00:60.1Z",
            "2030-01-01T00:00:00.1234567890Z",
            "2030-01-01T00:00:00.Z",
            "2030-01-01T00:00:00.aZ",
            "2030-1-01T00:00:00Z",
            "2030-01-01 00:00:00Z",
            "2030-01-01T00:00:00Z\n",
            "2030-01-01T00:00:00.１２Z",
        ] {
            assert_rejection(
                Parameters {
                    contest_end: text.to_owned(),
                    ..Parameters::default()
                },
                ParameterRejection::ContestEndFormat,
            );
            assert_rejection(
                Parameters {
                    gateway_not_after: text.to_owned(),
                    ..Parameters::default()
                },
                ParameterRejection::GatewayNotAfterFormat,
            );
        }
    }

    #[test]
    fn timestamp_parser_errors_preserve_their_sources() {
        for gateway in [false, true] {
            let mut input = Parameters::default();
            let expected = if gateway {
                input.gateway_not_after = "2030-02-30T00:00:00Z".to_owned();
                ParameterRejection::GatewayNotAfterFormat
            } else {
                input.contest_end = "2029-12-31T24:00:00Z".to_owned();
                ParameterRejection::ContestEndFormat
            };
            let Err(error) = input.build() else {
                panic!("invalid calendar value was accepted");
            };
            assert!(
                matches!(error, ConfigRenderError::TimestampParse { reason, .. } if reason == expected),
                "unexpected error: {error:?}"
            );
            assert!(
                error
                    .source()
                    .is_some_and(<dyn Error>::is::<time::error::Parse>),
                "timestamp parser source must be retained"
            );
        }
    }

    #[test]
    fn preserves_fractional_timestamps_and_checks_exact_coverage() -> Result<(), TestError> {
        for fraction in ["1", "123456789"] {
            let contest_end = format!("2029-12-31T00:00:00.{fraction}Z");
            let gateway_not_after = format!("2030-01-01T00:00:00.{fraction}Z");
            let params = Parameters {
                contest_end: contest_end.clone(),
                gateway_not_after: gateway_not_after.clone(),
                ..Parameters::default()
            }
            .build()?;
            assert_eq!(params.contest_end(), contest_end);
            assert_eq!(params.gateway_not_after(), gateway_not_after);
            let text = render_server_config(&params)?;
            let table: Table = toml::from_str(&text)?;
            assert_eq!(
                table["site"]["gateway_not_after"].as_str(),
                Some(gateway_not_after.as_str())
            );
            assert_eq!(
                table["site"]["contest_end"].as_str(),
                Some(contest_end.as_str())
            );
            validate_server_config(&text)?;
        }
        assert_rejection(
            Parameters {
                contest_end: "2029-12-31T00:00:00.000000001Z".to_owned(),
                ..Parameters::default()
            },
            ParameterRejection::GatewayValidityTooShort,
        );
        assert_rejection(
            Parameters {
                gateway_not_after: "2029-12-31T23:59:59Z".to_owned(),
                ..Parameters::default()
            },
            ParameterRejection::GatewayValidityTooShort,
        );
        assert_rejection(
            Parameters {
                contest_end: "9999-12-31T23:59:59Z".to_owned(),
                gateway_not_after: "9999-12-31T23:59:59Z".to_owned(),
                ..Parameters::default()
            },
            ParameterRejection::GatewayValidityTooShort,
        );
        Ok(())
    }

    #[test]
    fn gateway_expiry_must_be_strictly_after_now() -> Result<(), TestError> {
        let expiry = OffsetDateTime::parse("2030-01-01T00:00:00Z", &Rfc3339)?;
        for now in [expiry, expiry + Duration::nanoseconds(1)] {
            assert_rejection(
                Parameters {
                    now,
                    ..Parameters::default()
                },
                ParameterRejection::GatewayExpired,
            );
        }
        Parameters {
            now: expiry - Duration::nanoseconds(1),
            ..Parameters::default()
        }
        .build()?;
        Ok(())
    }

    #[test]
    fn server_loader_owns_hostname_and_origin_rejections() -> Result<(), TestError> {
        for hostname in ["Gateway.contest.example", "gateway_contest.example"] {
            let params = Parameters {
                hostname: hostname.to_owned(),
                ..Parameters::default()
            }
            .build()?;
            let Err(error) = validate_server_config(&render_server_config(&params)?) else {
                panic!("invalid hostname {hostname:?} was accepted");
            };
            assert!(error.source().is_some_and(<dyn Error>::is::<ConfigError>));
            assert!(matches!(
                error,
                ConfigRenderError::Validation {
                    source: ConfigError::InvalidGatewayHostname
                }
            ));
        }
        for origin in [
            "http://judge.example",
            "https://judge.example/path",
            "https://judge.example/",
        ] {
            let params = Parameters {
                origin: origin.to_owned(),
                ..Parameters::default()
            }
            .build()?;
            let Err(error) = validate_server_config(&render_server_config(&params)?) else {
                panic!("invalid origin {origin:?} was accepted");
            };
            assert!(error.source().is_some_and(<dyn Error>::is::<ConfigError>));
            assert!(matches!(
                error,
                ConfigRenderError::Validation {
                    source: ConfigError::InvalidDomjudgeOrigin
                }
            ));
        }
        Ok(())
    }

    #[test]
    fn toml_escaping_cannot_inject_keys() -> Result<(), TestError> {
        let server_schema: Table = toml::from_str(include_str!(
            "../../../packaging/server/config.example.toml"
        ))?;
        let client_schema: Table = toml::from_str(include_str!(
            "../../../packaging/client/config.example.toml"
        ))?;
        for payload in [
            "quote\"value",
            "line\n[rogue]\nkey = 1\n",
            "slash\\value\r\t\0",
        ] {
            for hostname in [false, true] {
                let mut input = Parameters::default();
                if hostname {
                    input.hostname = payload.to_owned();
                } else {
                    input.origin = payload.to_owned();
                }
                let params = input.build()?;
                let text = render_server_config(&params)?;
                let server: Table = toml::from_str(&text)?;
                assert_schema(&server, &server_schema);
                assert_eq!(
                    server["site"]["gateway_hostname"].as_str(),
                    Some(params.gateway_hostname())
                );
                assert_eq!(
                    server["runtime"]["domjudge_origin"].as_str(),
                    Some(params.domjudge_origin())
                );
                let Err(error) = validate_server_config(&text) else {
                    panic!("special-character parameter was accepted");
                };
                assert!(error.source().is_some_and(<dyn Error>::is::<ConfigError>));
                let client: Table = toml::from_str(&render_client_config(&params)?)?;
                assert_schema(&client, &client_schema);
                assert_eq!(
                    client["site"]["gateway_hostname"].as_str(),
                    Some(params.gateway_hostname())
                );
            }
        }
        Ok(())
    }

    #[test]
    fn client_config_matches_example_and_uses_first_ip_literal() -> Result<(), TestError> {
        let params = Parameters::default().build()?;
        let text = render_client_config(&params)?;
        assert!(text.starts_with("# Client /etc/natsume/config.toml; contains no credentials"));
        let actual: Table = toml::from_str(&text)?;
        let expected: Table = toml::from_str(include_str!(
            "../../../packaging/client/config.example.toml"
        ))?;
        assert_eq!(actual, expected);
        let params = Parameters {
            ips: vec![IPV6, IPV4],
            ..Parameters::default()
        }
        .build()?;
        let client: Table = toml::from_str(&render_client_config(&params)?)?;
        assert_schema(&client, &expected);
        assert_eq!(client["server"]["ip"].as_str(), Some("2001:db8::10"));
        assert_eq!(client["server"]["port"].as_integer(), Some(8443));
        Ok(())
    }

    #[test]
    fn malformed_server_config_retains_loader_error() {
        let Err(error) = validate_server_config("[listen") else {
            panic!("malformed TOML was accepted");
        };
        assert!(error.source().is_some_and(<dyn Error>::is::<ConfigError>));
        assert!(matches!(
            error,
            ConfigRenderError::Validation {
                source: ConfigError::DecodeFailed { .. }
            }
        ));
    }
}
