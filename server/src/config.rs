use std::{fs, net::SocketAddr, path::Path, path::PathBuf};

use serde::Deserialize;
use snafu::Snafu;

const CONFIG_PATH: &str = "/etc/natsume-server/config.toml";

/// Validated configuration consumed by the Stage 3 Server startup.
pub struct ServerConfig {
    listen_address: SocketAddr,
    log_level: LogLevel,
    database_path: PathBuf,
    vault_master_key_path: PathBuf,
    tls_certificate_path: PathBuf,
    tls_private_key_path: PathBuf,
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
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use snafu::Snafu;
    use uuid::Uuid;

    use super::{ConfigError, LogLevel, ServerConfig};

    const VALID_CONFIG: &str = r#"
[listen]
https = "127.0.0.1:8443"

[storage]
database = "/var/lib/natsume-server/natsume.db"
root_key = "/var/lib/natsume-server/keys/server-root.key"

[tls]
certificate = "/var/lib/natsume-server/keys/server-tls-leaf.der"
private_key = "/var/lib/natsume-server/keys/server-tls-key.pk8"
"#;

    #[test]
    fn valid_file_parses() -> Result<(), TestFailure> {
        let fixture = ConfigFixture::new(VALID_CONFIG)?;
        ServerConfig::load_from(fixture.path())
            .map_err(|_| TestFailure::UnexpectedConfigurationFailure)?;
        Ok(())
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
    }
}
