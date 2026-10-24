use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use snafu::{ResultExt as _, ensure};
use std::io::Write as _;
use url::Url;

use super::error::{ConfigurationSnafu, InvalidSnafu, Result, StorageSnafu, TransportSnafu};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub server_url: String,
    pub queue_name: String,
    pub state_directory: PathBuf,
    pub ca_certificate: Option<PathBuf>,
}

impl Config {
    /// Loads a fixed server, queue and local-state location.
    ///
    /// # Errors
    /// Rejects invalid configuration, non-HTTPS origins and relative storage paths.
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|source: &mut std::io::Error| StorageSnafu {
            operation: if source.kind() == std::io::ErrorKind::NotFound {
                "read configuration: file not found; select the existing configuration, or create one only for a new deployment"
            } else {
                "read configuration"
            },
        })?;
        let config: Self = toml::from_str(&text).context(ConfigurationSnafu)?;
        config.validated()
    }

    pub(super) fn validated(mut self) -> Result<Self> {
        let config = &mut self;
        let origin = Url::parse(&config.server_url).map_err(|_| {
            InvalidSnafu {
                field: "server_url",
            }
            .build()
        })?;
        ensure!(
            origin.scheme() == "https"
                && origin.host_str().is_some()
                && origin.username().is_empty()
                && origin.password().is_none()
                && origin.path() == "/"
                && origin.query().is_none()
                && origin.fragment().is_none(),
            InvalidSnafu {
                field: "server_url"
            }
        );
        ensure!(
            !config.queue_name.is_empty()
                && config.queue_name.encode_utf16().count() <= 32767
                && !config.queue_name.chars().any(char::is_control),
            InvalidSnafu {
                field: "queue_name"
            }
        );
        ensure!(
            config.state_directory.is_absolute()
                && config
                    .ca_certificate
                    .as_ref()
                    .is_none_or(|path| path.is_absolute()),
            InvalidSnafu {
                field: "storage paths"
            }
        );
        config.server_url = origin.origin().ascii_serialization();
        Ok(self)
    }

    pub(super) fn save_new(&self, path: &Path) -> Result<()> {
        let text = toml::to_string(self).context(super::error::ConfigurationWriteSnafu)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).context(StorageSnafu {
                operation: "create configuration directory",
            })?;
        }
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .context(StorageSnafu {
                operation: "create new configuration; existing files are not overwritten",
            })?;
        file.write_all(text.as_bytes()).context(StorageSnafu {
            operation: "write configuration",
        })?;
        file.sync_all().context(StorageSnafu {
            operation: "sync configuration",
        })
    }

    pub(super) fn http(&self) -> Result<reqwest::Client> {
        let mut builder = reqwest::Client::builder()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(10));
        if let Some(path) = &self.ca_certificate {
            let bytes = std::fs::read(path).context(StorageSnafu {
                operation: "read CA certificate",
            })?;
            let certificates =
                reqwest::Certificate::from_pem_bundle(&bytes).context(TransportSnafu)?;
            ensure!(
                !certificates.is_empty(),
                InvalidSnafu {
                    field: "ca_certificate"
                }
            );
            for certificate in certificates {
                builder = builder.add_root_certificate(certificate);
            }
        }
        builder.build().context(TransportSnafu)
    }
}

#[cfg(test)]
mod tests {
    use super::super::error::ClientError;
    use super::*;

    #[test]
    fn missing_configuration_explains_the_next_step_without_exposing_source_details() -> Result<()>
    {
        let directory = tempfile::tempdir().context(StorageSnafu {
            operation: "create configuration test directory",
        })?;
        let path = directory.path().join("private-client-configuration.toml");
        match Config::load(&path) {
            Err(error @ ClientError::Storage { .. }) => {
                let message = error.to_string();
                assert!(message.contains("file not found"));
                assert!(message.contains("select the existing configuration"));
                assert!(!message.contains("private-client-configuration"));
                if let ClientError::Storage { source, .. } = error {
                    assert_eq!(source.kind(), std::io::ErrorKind::NotFound);
                }
            }
            _ => {
                return InvalidSnafu {
                    field: "missing configuration test result",
                }
                .fail();
            }
        }
        Ok(())
    }
}
