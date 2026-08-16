use std::{fs, net::SocketAddr, path::Path, path::PathBuf};

use serde::Deserialize;
use snafu::Snafu;
use time::{Duration, OffsetDateTime, format_description::well_known::Rfc3339};

const CONFIG_PATH: &str = "/etc/natsume-server/config.toml";
pub(crate) const ORIGIN_CA_CERTIFICATE_FILENAME: &str = "origin-ca.der";
pub(crate) const ORIGIN_CA_PRIVATE_KEY_FILENAME: &str = "origin-ca-key.pk8";
pub(crate) const GATEWAY_VALIDITY_MARGIN_SECONDS: i64 = 86_400;

/// Validated configuration consumed by the Stage 3 Server startup.
pub struct ServerConfig {
    listen_address: SocketAddr,
    log_level: LogLevel,
    database_path: PathBuf,
    vault_master_key_path: PathBuf,
    tls_certificate_path: PathBuf,
    tls_private_key_path: PathBuf,
    site_config_path: PathBuf,
    control_root_path: PathBuf,
    local_origin_root_path: PathBuf,
}

impl ServerConfig {
    /// Loads the fixed package-owned Server configuration.
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
    pub(crate) fn load_from(path: &Path) -> Result<Self, ConfigError> {
        let encoded = fs::read_to_string(path).map_err(|_| ConfigError::ReadFailed)?;
        let raw: RawServerConfig =
            toml::from_str(&encoded).map_err(|_| ConfigError::DecodeFailed)?;
        Self::validate(raw)
    }

    fn validate(raw: RawServerConfig) -> Result<Self, ConfigError> {
        let listen_address = raw
            .listen
            .https
            .parse::<SocketAddr>()
            .map_err(|_| ConfigError::InvalidListenAddress)?;
        let config = Self {
            listen_address,
            log_level: raw.log.level,
            database_path: raw.storage.database,
            vault_master_key_path: raw.storage.root_key,
            tls_certificate_path: raw.tls.certificate,
            tls_private_key_path: raw.tls.private_key,
            site_config_path: raw.site.config,
            control_root_path: raw.site.control_root,
            local_origin_root_path: raw.site.local_origin_root,
        };
        config.validate_paths()?;
        Ok(config)
    }

    fn validate_paths(&self) -> Result<(), ConfigError> {
        require_absolute(&self.database_path, ConfigError::RelativeDatabasePath)?;
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
        require_absolute(&self.site_config_path, ConfigError::RelativeSiteConfigPath)?;
        require_absolute(
            &self.control_root_path,
            ConfigError::RelativeControlRootPath,
        )?;
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

    pub(crate) fn site_config_path(&self) -> &Path {
        &self.site_config_path
    }

    pub(crate) fn local_origin_root_path(&self) -> &Path {
        &self.local_origin_root_path
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
    site: RawSitePathsConfig,
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
}

#[derive(Deserialize)]
struct RawTlsConfig {
    certificate: PathBuf,
    private_key: PathBuf,
}

#[derive(Deserialize)]
struct RawSitePathsConfig {
    config: PathBuf,
    control_root: PathBuf,
    local_origin_root: PathBuf,
}

/// Validated installation policy used by the Gateway certificate issuer.
#[derive(Clone)]
pub(crate) struct GatewaySiteConfig {
    gateway_hostname: String,
    gateway_not_after: GatewayNotAfter,
    contest_end: GatewayNotAfter,
}

impl GatewaySiteConfig {
    /// Loads the shared site file and validates the three issuance-owned keys.
    ///
    /// Other site keys remain owned by their respective Client consumers and
    /// are deliberately ignored by this Server projection.
    pub(crate) fn load_from(path: &Path) -> Result<Self, SiteConfigError> {
        let encoded = fs::read_to_string(path).map_err(|_| SiteConfigError::ReadFailed)?;
        let raw: RawGatewaySiteConfig =
            toml::from_str(&encoded).map_err(|_| SiteConfigError::DecodeFailed)?;
        if !is_canonical_dns_hostname(&raw.gateway_hostname) {
            return Err(SiteConfigError::InvalidGatewayHostname);
        }
        let gateway_not_after = GatewayNotAfter::parse(raw.gateway_not_after)
            .ok_or(SiteConfigError::InvalidGatewayNotAfter)?;
        let contest_end =
            GatewayNotAfter::parse(raw.contest_end).ok_or(SiteConfigError::InvalidContestEnd)?;
        validate_gateway_validity_coverage(&gateway_not_after, &contest_end)?;
        Ok(Self {
            gateway_hostname: raw.gateway_hostname,
            gateway_not_after,
            contest_end,
        })
    }

    pub(crate) fn gateway_hostname(&self) -> &str {
        &self.gateway_hostname
    }

    pub(crate) const fn gateway_not_after(&self) -> &GatewayNotAfter {
        &self.gateway_not_after
    }

    #[cfg(test)]
    pub(crate) const fn contest_end(&self) -> &GatewayNotAfter {
        &self.contest_end
    }

    pub(crate) fn has_required_validity_coverage(&self) -> bool {
        validate_gateway_validity_coverage(&self.gateway_not_after, &self.contest_end).is_ok()
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn for_test(
        gateway_hostname: &str,
        gateway_not_after: &str,
        contest_end: &str,
    ) -> Result<Self, SiteConfigError> {
        if !is_canonical_dns_hostname(gateway_hostname) {
            return Err(SiteConfigError::InvalidGatewayHostname);
        }
        let gateway_not_after = GatewayNotAfter::parse(gateway_not_after.to_owned())
            .ok_or(SiteConfigError::InvalidGatewayNotAfter)?;
        let contest_end = GatewayNotAfter::parse(contest_end.to_owned())
            .ok_or(SiteConfigError::InvalidContestEnd)?;
        validate_gateway_validity_coverage(&gateway_not_after, &contest_end)?;
        Ok(Self {
            gateway_hostname: gateway_hostname.to_owned(),
            gateway_not_after,
            contest_end,
        })
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
) -> Result<(), SiteConfigError> {
    let required_not_after = contest_end
        .timestamp()
        .checked_add(Duration::seconds(GATEWAY_VALIDITY_MARGIN_SECONDS))
        .ok_or(SiteConfigError::GatewayValidityCoverageTooShort)?;
    if gateway_not_after.timestamp() < required_not_after {
        return Err(SiteConfigError::GatewayValidityCoverageTooShort);
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

/// Parsed RFC 3339 UTC timestamp retained in its policy wire representation.
#[derive(Clone)]
pub(crate) struct GatewayNotAfter {
    encoded: String,
    timestamp: OffsetDateTime,
}

impl GatewayNotAfter {
    fn parse(encoded: String) -> Option<Self> {
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
        let timestamp = OffsetDateTime::parse(&encoded, &Rfc3339).ok()?;
        if !(1970..=9999).contains(&timestamp.year()) {
            return None;
        }
        Some(Self { encoded, timestamp })
    }

    pub(crate) fn encoded(&self) -> &str {
        &self.encoded
    }

    pub(crate) const fn unix_seconds(&self) -> i64 {
        self.timestamp.unix_timestamp()
    }

    pub(crate) const fn timestamp(&self) -> OffsetDateTime {
        self.timestamp
    }
}

/// Redacted shared-site configuration failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Snafu)]
#[snafu(module)]
pub(crate) enum SiteConfigError {
    #[snafu(display("the site configuration could not be read"))]
    ReadFailed,
    #[snafu(display("the site configuration could not be decoded"))]
    DecodeFailed,
    #[snafu(display("the Gateway hostname is invalid"))]
    InvalidGatewayHostname,
    #[snafu(display("the Gateway certificate not-after policy is invalid"))]
    InvalidGatewayNotAfter,
    #[snafu(display("the contest end policy is invalid"))]
    InvalidContestEnd,
    #[snafu(display("the Gateway certificate validity does not cover the contest margin"))]
    GatewayValidityCoverageTooShort,
}

/// Redacted Server configuration failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Snafu)]
pub enum ConfigError {
    #[snafu(display("the server configuration could not be read"))]
    ReadFailed,
    #[snafu(display("the server configuration could not be decoded"))]
    DecodeFailed,
    #[snafu(display("the configured listen address is invalid"))]
    InvalidListenAddress,
    #[snafu(display("the configured database path must be absolute"))]
    RelativeDatabasePath,
    #[snafu(display("the configured vault master key path must be absolute"))]
    RelativeVaultMasterKeyPath,
    #[snafu(display("the configured TLS certificate path must be absolute"))]
    RelativeTlsCertificatePath,
    #[snafu(display("the configured TLS private key path must be absolute"))]
    RelativeTlsPrivateKeyPath,
    #[snafu(display("the configured site file path must be absolute"))]
    RelativeSiteConfigPath,
    #[snafu(display("the configured control root path must be absolute"))]
    RelativeControlRootPath,
    #[snafu(display("the configured local origin root path must be absolute"))]
    RelativeLocalOriginRootPath,
    #[snafu(display("the configured private keys directory is invalid"))]
    InvalidPrivateKeysDirectory,
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
    };

    use snafu::Snafu;
    use uuid::Uuid;

    use super::{
        ConfigError, GatewaySiteConfig, LogLevel, OffsetDateTime, ServerConfig, SiteConfigError,
    };

    const VALID_CONFIG: &str = r#"
[listen]
https = "127.0.0.1:8443"

[storage]
database = "/var/lib/natsume-server/natsume.db"
root_key = "/var/lib/natsume-server/keys/server-root.key"

[tls]
certificate = "/var/lib/natsume-server/keys/server-tls-leaf.der"
private_key = "/var/lib/natsume-server/keys/server-tls-key.pk8"

[site]
config = "/etc/natsume/site.toml"
control_root = "/etc/natsume/trust/control-ca.crt"
local_origin_root = "/etc/natsume/trust/local-origin-ca.crt"
"#;

    const VALID_SITE_CONFIG: &str = r#"
schema_version = 1
fleet_namespace_uuid = "00000000-0000-4000-8000-000000000001"
gateway_hostname = "gateway.contest.example"
gateway_not_after = "2028-02-29T23:59:58.123456789Z"
contest_end = "2028-02-28T23:59:58.123456789Z"

[trust]
control_root_sha256 = "ignored-by-server"
"#;

    #[test]
    fn valid_file_parses() -> Result<(), TestFailure> {
        let fixture = ConfigFixture::new(VALID_CONFIG)?;
        ServerConfig::load_from(fixture.path())
            .map_err(|_| TestFailure::UnexpectedConfigurationFailure)?;
        Ok(())
    }

    #[test]
    fn packaged_config_with_site_paths_parses_and_derives_fixed_origin_filenames()
    -> Result<(), TestFailure> {
        let config = ServerConfig::load_from(Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../packaging/server/rootfs/etc/natsume-server/config.toml"
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
    fn site_issuance_policy_is_strict_but_tolerates_other_consumers_keys() -> Result<(), TestFailure>
    {
        let fixture = ConfigFixture::new(VALID_SITE_CONFIG)?;
        let site = GatewaySiteConfig::load_from(fixture.path())
            .map_err(|_| TestFailure::UnexpectedConfigurationFailure)?;
        let timestamp = site.gateway_not_after().timestamp();
        if site.gateway_hostname() != "gateway.contest.example"
            || site.gateway_not_after().encoded() != "2028-02-29T23:59:58.123456789Z"
            || site.contest_end().encoded() != "2028-02-28T23:59:58.123456789Z"
            || timestamp.year() != 2028
            || u8::from(timestamp.month()) != 2
            || timestamp.day() != 29
            || timestamp.hour() != 23
            || timestamp.minute() != 59
            || timestamp.second() != 58
            || timestamp.nanosecond() != 123_456_789
            || OffsetDateTime::from_unix_timestamp(site.gateway_not_after().unix_seconds()).ok()
                != Some(timestamp - time::Duration::nanoseconds(123_456_789))
        {
            return Err(TestFailure::SitePolicyChanged);
        }
        Ok(())
    }

    #[test]
    fn invalid_site_policy_is_rejected_without_echoing_input() -> Result<(), TestFailure> {
        for (contents, expected, canary) in [
            (
                VALID_SITE_CONFIG.replace(
                    "gateway.contest.example",
                    "Gateway.invalid-host-canary.example",
                ),
                SiteConfigError::InvalidGatewayHostname,
                "invalid-host-canary",
            ),
            (
                VALID_SITE_CONFIG.replace(
                    "2028-02-29T23:59:58.123456789Z",
                    "2028-02-30T23:59:58Z-not-after-canary",
                ),
                SiteConfigError::InvalidGatewayNotAfter,
                "not-after-canary",
            ),
            (
                VALID_SITE_CONFIG.replace(
                    "2028-02-28T23:59:58.123456789Z",
                    "2028-02-30T23:59:58Z-contest-end-canary",
                ),
                SiteConfigError::InvalidContestEnd,
                "contest-end-canary",
            ),
            (
                VALID_SITE_CONFIG.replace(
                    "2028-02-28T23:59:58.123456789Z",
                    "2028-02-29T00:00:00Z-coverage-canary",
                ),
                SiteConfigError::InvalidContestEnd,
                "coverage-canary",
            ),
            (
                VALID_SITE_CONFIG.replace("2028-02-28T23:59:58.123456789Z", "2028-02-29T00:00:00Z"),
                SiteConfigError::GatewayValidityCoverageTooShort,
                "gateway.contest.example",
            ),
            (
                VALID_SITE_CONFIG.replace(
                    "2028-02-28T23:59:58.123456789Z",
                    "2028-02-28T23:59:58.223456789Z",
                ),
                SiteConfigError::GatewayValidityCoverageTooShort,
                "gateway.contest.example",
            ),
            // The strict shell is narrower than RFC 3339: numeric offsets, lowercase
            // separators, pre-epoch years, over-long fractions, and leap seconds are
            // all rejected even where the grammar or the library would accept them.
            (
                VALID_SITE_CONFIG.replace(
                    "2028-02-29T23:59:58.123456789Z",
                    "2028-02-29T23:59:58.123456789+00:00",
                ),
                SiteConfigError::InvalidGatewayNotAfter,
                "gateway.contest.example",
            ),
            (
                VALID_SITE_CONFIG.replace(
                    "2028-02-29T23:59:58.123456789Z",
                    "2028-02-29t23:59:58.123456789Z",
                ),
                SiteConfigError::InvalidGatewayNotAfter,
                "gateway.contest.example",
            ),
            (
                VALID_SITE_CONFIG.replace("2028-02-29T23:59:58.123456789Z", "1969-12-31T23:59:59Z"),
                SiteConfigError::InvalidGatewayNotAfter,
                "gateway.contest.example",
            ),
            (
                VALID_SITE_CONFIG.replace(
                    "2028-02-29T23:59:58.123456789Z",
                    "2028-02-29T23:59:58.1234567891Z",
                ),
                SiteConfigError::InvalidGatewayNotAfter,
                "gateway.contest.example",
            ),
            (
                VALID_SITE_CONFIG.replace("2028-02-29T23:59:58.123456789Z", "2028-02-29T23:59:60Z"),
                SiteConfigError::InvalidGatewayNotAfter,
                "gateway.contest.example",
            ),
        ] {
            let fixture = ConfigFixture::new(&contents)?;
            let Err(error) = GatewaySiteConfig::load_from(fixture.path()) else {
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
            ConfigError::DecodeFailed,
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
            ConfigError::ReadFailed,
            &["missing-config-canary"],
        )
    }

    #[test]
    fn malformed_toml_fails_redacted() -> Result<(), TestFailure> {
        let fixture = ConfigFixture::new("malformed-value-canary = [")?;
        assert_config_error(
            fixture.path(),
            ConfigError::DecodeFailed,
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
            ConfigError::DecodeFailed,
            &["missing-key-canary"],
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
            ConfigError::RelativeDatabasePath,
            &["relative-path-canary"],
        )
    }

    #[test]
    fn unparseable_listen_address_fails_redacted() -> Result<(), TestFailure> {
        let fixture =
            ConfigFixture::new(&VALID_CONFIG.replace("127.0.0.1:8443", "invalid-listen-canary"))?;
        assert_config_error(
            fixture.path(),
            ConfigError::InvalidListenAddress,
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
        expected: ConfigError,
        canaries: &[&str],
    ) -> Result<(), TestFailure> {
        let Err(error) = ServerConfig::load_from(path) else {
            return Err(TestFailure::ExpectedConfigurationFailure);
        };
        if error != expected {
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
