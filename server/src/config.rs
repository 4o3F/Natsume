use std::{fs, net::SocketAddr, path::Path, path::PathBuf};

use serde::Deserialize;
use snafu::Snafu;
use time::{Duration, OffsetDateTime, format_description::well_known::Rfc3339};

use crate::component::runtime::is_canonical_https_origin;

/// Fixed location of the deployment-owned Server configuration.
pub const CONFIG_PATH: &str = "/etc/natsume-server/config.toml";
pub(crate) const ORIGIN_CA_CERTIFICATE_FILENAME: &str = "origin-ca.der";
pub(crate) const ORIGIN_CA_PRIVATE_KEY_FILENAME: &str = "origin-ca-key.pk8";
pub(crate) const GATEWAY_VALIDITY_MARGIN_SECONDS: i64 = 86_400;

/// Validated configuration consumed by the Stage 3 Server startup.
pub struct ServerConfig {
    listen_address: SocketAddr,
    log_level: LogLevel,
    database_path: PathBuf,
    organization_logos_path: PathBuf,
    vault_master_key_path: PathBuf,
    tls_certificate_path: PathBuf,
    tls_private_key_path: PathBuf,
    site: GatewaySiteConfig,
    local_origin_root_path: PathBuf,
    domjudge_origin: String,
}

impl ServerConfig {
    /// Loads the fixed deployment-owned Server configuration.
    ///
    /// # Errors
    ///
    /// Returns a redacted [`ConfigError`] when the file cannot be read,
    /// decoded, or validated.
    pub fn load() -> Result<Self, ConfigError> {
        Self::load_from(Path::new(CONFIG_PATH))
    }

    /// Loads Server configuration from an explicit test/composition path.
    ///
    /// # Errors
    ///
    /// Returns a redacted [`ConfigError`] when the file cannot be read,
    /// decoded, or validated.
    pub fn load_from(path: &Path) -> Result<Self, ConfigError> {
        let encoded = fs::read_to_string(path)
            .map_err(|error| ConfigError::ReadFailed { kind: error.kind() })?;
        let raw: RawServerConfig =
            toml::from_str(&encoded).map_err(|error| decode_error(&error, &encoded))?;
        Self::validate(raw)
    }

    fn validate(raw: RawServerConfig) -> Result<Self, ConfigError> {
        let listen_address = raw
            .listen
            .https
            .parse::<SocketAddr>()
            .map_err(|_| ConfigError::InvalidListenAddress)?;
        if !is_canonical_https_origin(&raw.runtime.domjudge_origin) {
            return Err(ConfigError::InvalidDomjudgeOrigin);
        }
        let config = Self {
            listen_address,
            log_level: raw.log.level,
            database_path: raw.storage.database,
            organization_logos_path: raw.storage.organization_logos,
            vault_master_key_path: raw.storage.root_key,
            tls_certificate_path: raw.tls.certificate,
            tls_private_key_path: raw.tls.private_key,
            site: GatewaySiteConfig::validate(raw.site)?,
            local_origin_root_path: raw.trust.local_origin_root,
            domjudge_origin: raw.runtime.domjudge_origin,
        };
        config.validate_paths(&raw.trust.control_root)?;
        Ok(config)
    }

    fn validate_paths(&self, control_root: &Path) -> Result<(), ConfigError> {
        require_absolute(&self.database_path, ConfigError::RelativeDatabasePath)?;
        require_absolute(
            &self.organization_logos_path,
            ConfigError::RelativeOrganizationLogosPath,
        )?;
        require_absolute(
            &self.vault_master_key_path,
            ConfigError::RelativeVaultMasterKeyPath,
        )?;
        require_absolute(
            &self.tls_certificate_path,
            ConfigError::RelativeTlsCertificatePath,
        )?;
        require_absolute(
            &self.tls_private_key_path,
            ConfigError::RelativeTlsPrivateKeyPath,
        )?;
        require_absolute(control_root, ConfigError::RelativeControlRootPath)?;
        require_absolute(
            &self.local_origin_root_path,
            ConfigError::RelativeLocalOriginRootPath,
        )?;
        Ok(())
    }

    /// Returns the already validated listener socket address.
    #[must_use]
    pub(crate) const fn listen_address(&self) -> SocketAddr {
        self.listen_address
    }

    #[must_use]
    pub(crate) const fn log_level(&self) -> LogLevel {
        self.log_level
    }

    pub(crate) fn organization_logos_path(&self) -> &Path {
        &self.organization_logos_path
    }

    pub(crate) fn database_path(&self) -> &Path {
        &self.database_path
    }

    pub(crate) fn vault_master_key_path(&self) -> &Path {
        &self.vault_master_key_path
    }

    pub(crate) fn tls_certificate_path(&self) -> &Path {
        &self.tls_certificate_path
    }

    pub(crate) fn tls_private_key_path(&self) -> &Path {
        &self.tls_private_key_path
    }

    pub(crate) fn site(&self) -> &GatewaySiteConfig {
        &self.site
    }

    pub(crate) fn local_origin_root_path(&self) -> &Path {
        &self.local_origin_root_path
    }

    pub(crate) fn domjudge_origin(&self) -> &str {
        &self.domjudge_origin
    }

    pub(crate) fn origin_ca_certificate_path(&self) -> Result<PathBuf, ConfigError> {
        self.private_keys_directory()
            .map(|directory| directory.join(ORIGIN_CA_CERTIFICATE_FILENAME))
    }

    pub(crate) fn origin_ca_private_key_path(&self) -> Result<PathBuf, ConfigError> {
        self.private_keys_directory()
            .map(|directory| directory.join(ORIGIN_CA_PRIVATE_KEY_FILENAME))
    }

    fn private_keys_directory(&self) -> Result<&Path, ConfigError> {
        self.tls_private_key_path
            .parent()
            .ok_or(ConfigError::InvalidPrivateKeysDirectory)
    }
}

fn decode_error(error: &toml::de::Error, encoded: &str) -> ConfigError {
    // Serde's missing-field names come from our derived configuration schema.
    // Other messages can contain rejected values, so retain only their location.
    if let Some(field) = error
        .message()
        .strip_prefix("missing field `")
        .and_then(|message| message.strip_suffix('`'))
    {
        return ConfigError::MissingField {
            field: field.to_owned(),
        };
    }
    let offset = error.span().map_or(0, |span| span.start);
    let (line, column) = encoded
        .char_indices()
        .take_while(|(index, _)| *index < offset)
        .fold((1, 1), |(line, column), (_, character)| {
            if character == '\n' {
                (line + 1, 1)
            } else {
                (line, column + 1)
            }
        });
    ConfigError::DecodeFailed { line, column }
}

fn require_absolute(path: &Path, error: ConfigError) -> Result<(), ConfigError> {
    if path.is_absolute() {
        Ok(())
    } else {
        Err(error)
    }
}

#[derive(Deserialize)]
struct RawServerConfig {
    listen: RawListenConfig,
    #[serde(default)]
    log: RawLogConfig,
    storage: RawStorageConfig,
    tls: RawTlsConfig,
    site: RawGatewaySiteConfig,
    trust: RawTrustConfig,
    runtime: RawRuntimeConfig,
}

#[derive(Deserialize)]
struct RawRuntimeConfig {
    domjudge_origin: String,
}

#[derive(Deserialize, Default)]
struct RawLogConfig {
    #[serde(default)]
    level: LogLevel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub(crate) enum LogLevel {
    Error,
    Warn,
    #[default]
    Info,
    Debug,
    Trace,
}

#[derive(Deserialize)]
struct RawListenConfig {
    https: String,
}

#[derive(Deserialize)]
struct RawStorageConfig {
    database: PathBuf,
    root_key: PathBuf,
    organization_logos: PathBuf,
}

#[derive(Deserialize)]
struct RawTlsConfig {
    certificate: PathBuf,
    private_key: PathBuf,
}

#[derive(Deserialize)]
struct RawTrustConfig {
    control_root: PathBuf,
    local_origin_root: PathBuf,
}

/// Validated installation policy used by the Gateway certificate issuer.
#[derive(Clone)]
pub(crate) struct GatewaySiteConfig {
    gateway_hostname: String,
    gateway_not_after: GatewayNotAfter,
}

impl GatewaySiteConfig {
    fn validate(raw: RawGatewaySiteConfig) -> Result<Self, ConfigError> {
        if !is_canonical_dns_hostname(&raw.gateway_hostname) {
            return Err(ConfigError::InvalidGatewayHostname);
        }
        let gateway_not_after = GatewayNotAfter::parse(&raw.gateway_not_after)
            .ok_or(ConfigError::InvalidGatewayNotAfter)?;
        let contest_end =
            GatewayNotAfter::parse(&raw.contest_end).ok_or(ConfigError::InvalidContestEnd)?;
        validate_gateway_validity_coverage(&gateway_not_after, &contest_end)?;
        Ok(Self {
            gateway_hostname: raw.gateway_hostname,
            gateway_not_after,
        })
    }

    pub(crate) fn gateway_hostname(&self) -> &str {
        &self.gateway_hostname
    }

    pub(crate) const fn gateway_not_after(&self) -> &GatewayNotAfter {
        &self.gateway_not_after
    }
}

#[derive(Deserialize)]
struct RawGatewaySiteConfig {
    gateway_hostname: String,
    gateway_not_after: String,
    contest_end: String,
}

fn validate_gateway_validity_coverage(
    gateway_not_after: &GatewayNotAfter,
    contest_end: &GatewayNotAfter,
) -> Result<(), ConfigError> {
    let required_not_after = contest_end
        .timestamp()
        .checked_add(Duration::seconds(GATEWAY_VALIDITY_MARGIN_SECONDS))
        .ok_or(ConfigError::GatewayValidityCoverageTooShort)?;
    if gateway_not_after.timestamp() < required_not_after {
        return Err(ConfigError::GatewayValidityCoverageTooShort);
    }
    Ok(())
}

// The Server endpoint is parsed as `SocketAddr`, so it has no reusable DNS
// hostname parser. This site-only validator therefore closes the LDH grammar.
fn is_canonical_dns_hostname(value: &str) -> bool {
    if value.is_empty()
        || value.len() > 253
        || value.ends_with('.')
        || value.parse::<std::net::IpAddr>().is_ok()
        || !value.bytes().any(|byte| byte.is_ascii_lowercase())
    {
        return false;
    }
    value.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && label
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            && label
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphanumeric)
            && label
                .as_bytes()
                .last()
                .is_some_and(u8::is_ascii_alphanumeric)
    })
}

/// Strictly validated RFC 3339 UTC policy timestamp.
#[derive(Clone)]
pub(crate) struct GatewayNotAfter {
    timestamp: OffsetDateTime,
}

impl GatewayNotAfter {
    fn parse(encoded: &str) -> Option<Self> {
        // Strict shell over the library parser. The frozen contract is narrower than
        // RFC 3339 and than the library's leniency: an uppercase `T` separator, a
        // literal trailing `Z` (no numeric offsets, which also excludes lowercase
        // `z`), at most nine fractional digits (the library silently truncates
        // longer fractions), no leap second (the library folds `:60` to
        // 59.999999999), and years 1970..=9999.
        let bytes = encoded.as_bytes();
        if bytes.len() < 20
            || bytes.get(10) != Some(&b'T')
            || bytes.last() != Some(&b'Z')
            || bytes.get(17..19) == Some(b"60")
        {
            return None;
        }
        if let Some(digits) = bytes.get(20..bytes.len() - 1)
            && (bytes.get(19) != Some(&b'.') || digits.is_empty() || digits.len() > 9)
        {
            return None;
        }
        let timestamp = OffsetDateTime::parse(encoded, &Rfc3339).ok()?;
        if !(1970..=9999).contains(&timestamp.year()) {
            return None;
        }
        Some(Self { timestamp })
    }

    pub(crate) const fn timestamp(&self) -> OffsetDateTime {
        self.timestamp
    }
}

/// Redacted Server configuration failure.
#[derive(Debug, Clone, PartialEq, Eq, Snafu)]
pub enum ConfigError {
    #[snafu(display(
        "cannot read UTF-8 configuration ({kind:?}); check file existence and read permissions"
    ))]
    ReadFailed { kind: std::io::ErrorKind },
    #[snafu(display("missing required field `{field}`"))]
    MissingField { field: String },
    #[snafu(display("invalid TOML syntax or field type near line {line}, column {column}"))]
    DecodeFailed { line: usize, column: usize },
    #[snafu(display("listen.https must be an IP address and port, such as 0.0.0.0:8443"))]
    InvalidListenAddress,
    #[snafu(display(
        "runtime.domjudge_origin must be a canonical HTTPS origin without credentials, path, trailing slash, query or fragment"
    ))]
    InvalidDomjudgeOrigin,
    #[snafu(display("storage.database must be an absolute path"))]
    RelativeDatabasePath,
    #[snafu(display("storage.organization_logos must be an absolute path"))]
    RelativeOrganizationLogosPath,
    #[snafu(display("storage.root_key must be an absolute path"))]
    RelativeVaultMasterKeyPath,
    #[snafu(display("tls.certificate must be an absolute path"))]
    RelativeTlsCertificatePath,
    #[snafu(display("tls.private_key must be an absolute path"))]
    RelativeTlsPrivateKeyPath,
    #[snafu(display("trust.control_root must be an absolute path"))]
    RelativeControlRootPath,
    #[snafu(display("trust.local_origin_root must be an absolute path"))]
    RelativeLocalOriginRootPath,
    #[snafu(display("tls.private_key must have a parent directory for the Origin CA files"))]
    InvalidPrivateKeysDirectory,
    #[snafu(display(
        "site.gateway_hostname must be a lowercase DNS hostname without a trailing dot"
    ))]
    InvalidGatewayHostname,
    #[snafu(display(
        "site.gateway_not_after must be a valid UTC timestamp such as 2026-12-08T10:00:00Z"
    ))]
    InvalidGatewayNotAfter,
    #[snafu(display(
        "site.contest_end must be a valid UTC timestamp such as 2026-12-06T10:00:00Z"
    ))]
    InvalidContestEnd,
    #[snafu(display(
        "site.gateway_not_after must cover site.contest_end plus at least 86400 seconds"
    ))]
    GatewayValidityCoverageTooShort,
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
    };

    use snafu::Snafu;
    use uuid::Uuid;

    use super::{ConfigError, GatewaySiteConfig, LogLevel, ServerConfig};

    impl GatewaySiteConfig {
        pub(crate) fn for_test(
            gateway_hostname: &str,
            gateway_not_after: &str,
            contest_end: &str,
        ) -> Result<Self, ConfigError> {
            Self::validate(super::RawGatewaySiteConfig {
                gateway_hostname: gateway_hostname.to_owned(),
                gateway_not_after: gateway_not_after.to_owned(),
                contest_end: contest_end.to_owned(),
            })
        }
    }

    const VALID_CONFIG: &str = r#"
[listen]
https = "127.0.0.1:8443"

[storage]
database = "/var/lib/natsume-server/natsume.db"
root_key = "/var/lib/natsume-server/keys/server-root.key"
organization_logos = "/var/lib/natsume-server/organization-logos"

[tls]
certificate = "/var/lib/natsume-server/keys/server-tls-leaf.der"
private_key = "/var/lib/natsume-server/keys/server-tls-key.pk8"

[trust]
control_root = "/etc/natsume/trust/control-ca.crt"
local_origin_root = "/etc/natsume/trust/local-origin-ca.crt"

[site]
gateway_hostname = "gateway.contest.example"
gateway_not_after = "2028-02-29T23:59:58.123456789Z"
contest_end = "2028-02-28T23:59:58.123456789Z"

[runtime]
domjudge_origin = "https://judge.contest.example"
"#;

    #[test]
    fn valid_file_parses() -> Result<(), TestFailure> {
        let fixture = ConfigFixture::new(VALID_CONFIG)?;
        let config = ServerConfig::load_from(fixture.path())
            .map_err(|_| TestFailure::UnexpectedConfigurationFailure)?;
        assert_eq!(config.domjudge_origin(), "https://judge.contest.example");
        Ok(())
    }

    #[test]
    fn missing_runtime_configuration_is_rejected() -> Result<(), TestFailure> {
        for (contents, field) in [
            (
                VALID_CONFIG.replace(
                    "[runtime]\ndomjudge_origin = \"https://judge.contest.example\"",
                    "",
                ),
                "runtime",
            ),
            (
                VALID_CONFIG.replace("domjudge_origin = \"https://judge.contest.example\"", ""),
                "domjudge_origin",
            ),
        ] {
            let fixture = ConfigFixture::new(&contents)?;
            assert_config_error(
                fixture.path(),
                &ConfigError::MissingField {
                    field: field.to_owned(),
                },
                &[],
            )?;
        }
        Ok(())
    }

    #[test]
    fn invalid_runtime_origin_is_rejected_without_echoing_input() -> Result<(), TestFailure> {
        for origin in [
            "http://judge.contest.example",
            "https://user:password-canary@judge.contest.example",
            "https://judge.contest.example/login",
            "https://judge.contest.example/",
            "https://judge.contest.example?contest=1",
            "https://judge.contest.example#fragment",
            "https://JUDGE.contest.example",
            "https://judge.contest.example:443",
            "",
        ] {
            let fixture =
                ConfigFixture::new(&VALID_CONFIG.replace("https://judge.contest.example", origin))?;
            assert_config_error(
                fixture.path(),
                &ConfigError::InvalidDomjudgeOrigin,
                &["password-canary", "judge.contest.example"],
            )?;
        }
        Ok(())
    }

    #[test]
    fn deployment_example_parses_and_derives_fixed_origin_filenames() -> Result<(), TestFailure> {
        let config = ServerConfig::load_from(Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../packaging/server/config.example.toml"
        )))
        .map_err(|_| TestFailure::UnexpectedConfigurationFailure)?;
        if config
            .origin_ca_certificate_path()
            .map_err(|_| TestFailure::UnexpectedConfigurationFailure)?
            .as_path()
            != Path::new("/var/lib/natsume-server/keys/origin-ca.der")
            || config
                .origin_ca_private_key_path()
                .map_err(|_| TestFailure::UnexpectedConfigurationFailure)?
                .as_path()
                != Path::new("/var/lib/natsume-server/keys/origin-ca-key.pk8")
        {
            return Err(TestFailure::OriginCaPathsChanged);
        }
        Ok(())
    }

    #[test]
    fn site_issuance_policy_is_loaded_from_the_single_server_config() -> Result<(), TestFailure> {
        let fixture = ConfigFixture::new(VALID_CONFIG)?;
        let config = ServerConfig::load_from(fixture.path())
            .map_err(|_| TestFailure::UnexpectedConfigurationFailure)?;
        let site = config.site();
        let timestamp = site.gateway_not_after().timestamp();
        if site.gateway_hostname() != "gateway.contest.example"
            || timestamp.year() != 2028
            || u8::from(timestamp.month()) != 2
            || timestamp.day() != 29
            || timestamp.hour() != 23
            || timestamp.minute() != 59
            || timestamp.second() != 58
            || timestamp.nanosecond() != 123_456_789
        {
            return Err(TestFailure::SitePolicyChanged);
        }
        Ok(())
    }

    #[test]
    fn invalid_site_policy_is_rejected_without_echoing_input() -> Result<(), TestFailure> {
        for (contents, expected, canary) in [
            (
                VALID_CONFIG.replace(
                    "gateway.contest.example",
                    "Gateway.invalid-host-canary.example",
                ),
                ConfigError::InvalidGatewayHostname,
                "invalid-host-canary",
            ),
            (
                VALID_CONFIG.replace(
                    "2028-02-29T23:59:58.123456789Z",
                    "2028-02-30T23:59:58Z-not-after-canary",
                ),
                ConfigError::InvalidGatewayNotAfter,
                "not-after-canary",
            ),
            (
                VALID_CONFIG.replace(
                    "2028-02-28T23:59:58.123456789Z",
                    "2028-02-30T23:59:58Z-contest-end-canary",
                ),
                ConfigError::InvalidContestEnd,
                "contest-end-canary",
            ),
            (
                VALID_CONFIG.replace(
                    "2028-02-28T23:59:58.123456789Z",
                    "2028-02-29T00:00:00Z-coverage-canary",
                ),
                ConfigError::InvalidContestEnd,
                "coverage-canary",
            ),
            (
                VALID_CONFIG.replace("2028-02-28T23:59:58.123456789Z", "2028-02-29T00:00:00Z"),
                ConfigError::GatewayValidityCoverageTooShort,
                "gateway.contest.example",
            ),
            (
                VALID_CONFIG.replace(
                    "2028-02-28T23:59:58.123456789Z",
                    "2028-02-28T23:59:58.223456789Z",
                ),
                ConfigError::GatewayValidityCoverageTooShort,
                "gateway.contest.example",
            ),
            // The strict shell is narrower than RFC 3339: numeric offsets, lowercase
            // separators, pre-epoch years, over-long fractions, and leap seconds are
            // all rejected even where the grammar or the library would accept them.
            (
                VALID_CONFIG.replace(
                    "2028-02-29T23:59:58.123456789Z",
                    "2028-02-29T23:59:58.123456789+00:00",
                ),
                ConfigError::InvalidGatewayNotAfter,
                "gateway.contest.example",
            ),
            (
                VALID_CONFIG.replace(
                    "2028-02-29T23:59:58.123456789Z",
                    "2028-02-29t23:59:58.123456789Z",
                ),
                ConfigError::InvalidGatewayNotAfter,
                "gateway.contest.example",
            ),
            (
                VALID_CONFIG.replace("2028-02-29T23:59:58.123456789Z", "1969-12-31T23:59:59Z"),
                ConfigError::InvalidGatewayNotAfter,
                "gateway.contest.example",
            ),
            (
                VALID_CONFIG.replace(
                    "2028-02-29T23:59:58.123456789Z",
                    "2028-02-29T23:59:58.1234567891Z",
                ),
                ConfigError::InvalidGatewayNotAfter,
                "gateway.contest.example",
            ),
            (
                VALID_CONFIG.replace("2028-02-29T23:59:58.123456789Z", "2028-02-29T23:59:60Z"),
                ConfigError::InvalidGatewayNotAfter,
                "gateway.contest.example",
            ),
        ] {
            let fixture = ConfigFixture::new(&contents)?;
            let Err(error) = ServerConfig::load_from(fixture.path()) else {
                return Err(TestFailure::ExpectedConfigurationFailure);
            };
            let display = error.to_string();
            let debug = format!("{error:?}");
            if error != expected
                || contains_canary(&display, canary)
                || contains_canary(&debug, canary)
            {
                return Err(TestFailure::ErrorWasNotRedacted);
            }
        }
        Ok(())
    }

    fn contains_canary(value: &str, canary: &str) -> bool {
        value
            .as_bytes()
            .windows(canary.len())
            .any(|window| window == canary.as_bytes())
    }

    #[test]
    fn log_levels_are_closed_and_unknown_values_are_redacted() -> Result<(), TestFailure> {
        for (encoded, expected) in [
            ("error", LogLevel::Error),
            ("warn", LogLevel::Warn),
            ("info", LogLevel::Info),
            ("debug", LogLevel::Debug),
            ("trace", LogLevel::Trace),
        ] {
            let fixture =
                ConfigFixture::new(&format!("{VALID_CONFIG}\n[log]\nlevel = \"{encoded}\"\n"))?;
            let config = ServerConfig::load_from(fixture.path())
                .map_err(|_| TestFailure::UnexpectedConfigurationFailure)?;
            if config.log_level() != expected {
                return Err(TestFailure::LogLevelContractChanged);
            }
        }

        let fixture = ConfigFixture::new(&format!(
            "{VALID_CONFIG}\n[log]\nlevel = \"unknown-log-level-canary\"\n"
        ))?;
        assert_config_error(
            fixture.path(),
            &ConfigError::DecodeFailed {
                line: VALID_CONFIG.lines().count() + 3,
                column: 9,
            },
            &["unknown-log-level-canary"],
        )
    }

    #[test]
    fn missing_log_section_or_level_defaults_to_info() -> Result<(), TestFailure> {
        for encoded in [VALID_CONFIG.to_owned(), format!("{VALID_CONFIG}\n[log]\n")] {
            let fixture = ConfigFixture::new(&encoded)?;
            let config = ServerConfig::load_from(fixture.path())
                .map_err(|_| TestFailure::UnexpectedConfigurationFailure)?;
            if config.log_level() != LogLevel::Info {
                return Err(TestFailure::LogLevelContractChanged);
            }
        }
        Ok(())
    }

    #[test]
    fn missing_file_fails_redacted() -> Result<(), TestFailure> {
        let directory = TestDirectory::new()?;
        assert_config_error(
            &directory.path.join("missing-config-canary.toml"),
            &ConfigError::ReadFailed {
                kind: std::io::ErrorKind::NotFound,
            },
            &["missing-config-canary"],
        )
    }

    #[test]
    fn malformed_toml_fails_redacted() -> Result<(), TestFailure> {
        let fixture = ConfigFixture::new("malformed-value-canary = [")?;
        assert_config_error(
            fixture.path(),
            &ConfigError::DecodeFailed {
                line: 1,
                column: "malformed-value-canary = [".len() + 1,
            },
            &["malformed-value-canary"],
        )
    }

    #[test]
    fn missing_required_key_fails_redacted() -> Result<(), TestFailure> {
        let fixture = ConfigFixture::new(&VALID_CONFIG.replace(
            "private_key = \"/var/lib/natsume-server/keys/server-tls-key.pk8\"",
            "missing-key-canary = true",
        ))?;
        assert_config_error(
            fixture.path(),
            &ConfigError::MissingField {
                field: "private_key".to_owned(),
            },
            &["missing-key-canary"],
        )
    }

    #[test]
    fn decode_errors_identify_missing_fields_and_syntax_locations() -> Result<(), TestFailure> {
        for (contents, expected) in [
            (
                VALID_CONFIG.replace("domjudge_origin = \"https://judge.contest.example\"", ""),
                "missing required field `domjudge_origin`",
            ),
            ("[runtime\n".to_owned(), "line 1, column 9"),
        ] {
            let fixture = ConfigFixture::new(&contents)?;
            let error = ServerConfig::load_from(fixture.path())
                .err()
                .ok_or(TestFailure::ExpectedConfigurationFailure)?;
            assert!(error.to_string().contains(expected), "{error}");
        }
        Ok(())
    }

    #[test]
    fn http_upstream_error_identifies_the_https_requirement() -> Result<(), TestFailure> {
        let fixture = ConfigFixture::new(
            &VALID_CONFIG.replace("https://judge.contest.example", "http://10.12.13.166"),
        )?;
        let error = ServerConfig::load_from(fixture.path())
            .err()
            .ok_or(TestFailure::ExpectedConfigurationFailure)?;
        let message = error.to_string();
        assert!(message.contains("runtime.domjudge_origin"));
        assert!(message.contains("HTTPS"));
        assert!(!message.contains("10.12.13.166"));
        Ok(())
    }

    #[test]
    fn organization_logos_requires_an_absolute_path_but_not_an_existing_directory()
    -> Result<(), TestFailure> {
        let fixture = ConfigFixture::new(VALID_CONFIG)?;
        let config = ServerConfig::load_from(fixture.path())
            .map_err(|_| TestFailure::ExpectedConfigurationFailure)?;
        assert_eq!(
            config.organization_logos_path(),
            std::path::Path::new("/var/lib/natsume-server/organization-logos")
        );
        let relative = ConfigFixture::new(&VALID_CONFIG.replace(
            "/var/lib/natsume-server/organization-logos",
            "relative-logo-canary",
        ))?;
        assert_config_error(
            relative.path(),
            &ConfigError::RelativeOrganizationLogosPath,
            &["relative-logo-canary"],
        )
    }

    #[test]
    fn relative_path_fails_redacted() -> Result<(), TestFailure> {
        let fixture = ConfigFixture::new(&VALID_CONFIG.replace(
            "/var/lib/natsume-server/natsume.db",
            "relative-path-canary.db",
        ))?;
        assert_config_error(
            fixture.path(),
            &ConfigError::RelativeDatabasePath,
            &["relative-path-canary"],
        )
    }

    #[test]
    fn unparseable_listen_address_fails_redacted() -> Result<(), TestFailure> {
        let fixture =
            ConfigFixture::new(&VALID_CONFIG.replace("127.0.0.1:8443", "invalid-listen-canary"))?;
        assert_config_error(
            fixture.path(),
            &ConfigError::InvalidListenAddress,
            &["invalid-listen-canary"],
        )
    }

    #[test]
    fn unknown_extra_section_is_tolerated() -> Result<(), TestFailure> {
        let mut config = VALID_CONFIG.to_owned();
        config.push_str("\n[future_phase]\nfuture-key-canary = \"future-value-canary\"\n");
        let fixture = ConfigFixture::new(&config)?;
        ServerConfig::load_from(fixture.path())
            .map_err(|_| TestFailure::UnexpectedConfigurationFailure)?;
        Ok(())
    }

    fn assert_config_error(
        path: &std::path::Path,
        expected: &ConfigError,
        canaries: &[&str],
    ) -> Result<(), TestFailure> {
        let Err(error) = ServerConfig::load_from(path) else {
            return Err(TestFailure::ExpectedConfigurationFailure);
        };
        if &error != expected {
            eprintln!("expected {expected:?}, received {error:?}");
            return Err(TestFailure::UnexpectedConfigurationFailure);
        }

        let display = error.to_string();
        let debug = format!("{error:?}");
        let path_canary = path.to_string_lossy();
        if display.contains(path_canary.as_ref()) || debug.contains(path_canary.as_ref()) {
            return Err(TestFailure::ErrorWasNotRedacted);
        }
        if canaries
            .iter()
            .any(|canary| display.contains(canary) || debug.contains(canary))
        {
            return Err(TestFailure::ErrorWasNotRedacted);
        }
        Ok(())
    }

    struct ConfigFixture {
        _directory: TestDirectory,
        path: PathBuf,
    }

    impl ConfigFixture {
        fn new(contents: &str) -> Result<Self, TestFailure> {
            let directory = TestDirectory::new()?;
            let path = directory.path.join("config.toml");
            fs::write(&path, contents).map_err(|_| TestFailure::FixtureCreationFailed)?;
            Ok(Self {
                _directory: directory,
                path,
            })
        }

        fn path(&self) -> &std::path::Path {
            &self.path
        }
    }

    struct TestDirectory {
        path: PathBuf,
    }

    impl TestDirectory {
        fn new() -> Result<Self, TestFailure> {
            let path =
                std::env::temp_dir().join(format!("natsume-server-config-test-{}", Uuid::now_v7()));
            fs::create_dir(&path).map_err(|_| TestFailure::FixtureCreationFailed)?;
            Ok(Self { path })
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _cleanup_result = fs::remove_dir_all(&self.path);
        }
    }

    #[derive(Debug, Snafu)]
    enum TestFailure {
        #[snafu(display("the configuration fixture could not be created"))]
        FixtureCreationFailed,
        #[snafu(display("a configuration failure was expected"))]
        ExpectedConfigurationFailure,
        #[snafu(display("the configuration result was unexpected"))]
        UnexpectedConfigurationFailure,
        #[snafu(display("a configuration error exposed rejected context"))]
        ErrorWasNotRedacted,
        #[snafu(display("the logging level configuration contract changed"))]
        LogLevelContractChanged,
        #[snafu(display("the fixed Origin CA paths changed"))]
        OriginCaPathsChanged,
        #[snafu(display("the parsed site issuance policy changed"))]
        SitePolicyChanged,
    }
}
