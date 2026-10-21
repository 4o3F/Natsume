use std::{
    fs::File,
    io::Read,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

use rustix::{
    fs::{Mode, OFlags, open},
    process::geteuid,
};
use serde::Deserialize;
use url::Url;

use crate::{Error, Result};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub server_url: String,
    pub token_file: PathBuf,
    pub ca_certificate: Option<PathBuf>,
}

impl Config {
    /// Loads deployment configuration from the supplied path.
    ///
    /// # Errors
    /// Rejects unsafe deployment files, invalid TOML, unknown fields and non-HTTPS origins.
    pub fn load(path: &Path) -> Result<Self> {
        let bytes = read_regular(path, 16 * 1024, Permission::Deployment)?;
        let text = std::str::from_utf8(&bytes).map_err(|_| Error::Configuration)?;
        let config: Self = toml::from_str(text).map_err(|_| Error::Configuration)?;
        config.origin()?;
        if !config.token_file.is_absolute()
            || config
                .ca_certificate
                .as_ref()
                .is_some_and(|path| !path.is_absolute())
        {
            return Err(Error::Configuration);
        }
        Ok(config)
    }

    pub(crate) fn origin(&self) -> Result<Url> {
        let url = Url::parse(&self.server_url).map_err(|_| Error::Configuration)?;
        if url.scheme() != "https"
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
            || self.server_url.contains(['\\', '\r', '\n', '\t'])
        {
            return Err(Error::Configuration);
        }
        // Reject userinfo and paths even when URL parsing would normalize them away.
        let origin = url.origin().ascii_serialization();
        if self.server_url != origin && self.server_url != format!("{origin}/") {
            return Err(Error::Configuration);
        }
        Ok(url)
    }

    pub(crate) fn client(&self) -> Result<(reqwest::Client, String)> {
        let token = read_regular(&self.token_file, 256, Permission::Secret)?;
        let token = std::str::from_utf8(&token).map_err(|_| Error::Configuration)?;
        let token = token.strip_suffix('\n').unwrap_or(token);
        let parts: Vec<_> = token.split('.').collect();
        if parts.len() != 3
            || parts[0] != "nps1"
            || !crate::wire::uuid_v7(parts[1])
            || !crate::wire::sha256(parts[2])
        {
            return Err(Error::Configuration);
        }
        let mut builder = reqwest::Client::builder()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy();
        if let Some(path) = &self.ca_certificate {
            let pem = read_regular(path, 128 * 1024, Permission::Deployment)?;
            let certificates =
                reqwest::Certificate::from_pem_bundle(&pem).map_err(|_| Error::Configuration)?;
            if certificates.is_empty() {
                return Err(Error::Configuration);
            }
            // Explicit CA configuration is exclusive, not an additional permissive root set.
            builder = builder.tls_certs_only(certificates);
        }
        let client = builder.build().map_err(|_| Error::Configuration)?;
        Ok((client, token.to_owned()))
    }
}

#[derive(Clone, Copy)]
pub(crate) enum Permission {
    Source,
    Deployment,
    Secret,
}

pub(crate) fn read_regular(path: &Path, limit: usize, permission: Permission) -> Result<Vec<u8>> {
    // NONBLOCK prevents a FIFO open from blocking before fstat rejects it.
    let fd = open(
        path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| Error::UnsafeFile)?;
    let file = File::from(fd);
    let metadata = file.metadata().map_err(|_| Error::UnsafeFile)?;
    if !metadata.is_file() || metadata.len() > limit as u64 {
        return Err(Error::UnsafeFile);
    }
    if !matches!(permission, Permission::Source) {
        let mode = metadata.mode() & 0o7777;
        let unsafe_mode = if matches!(permission, Permission::Secret) {
            !matches!(mode, 0o400 | 0o600)
        } else {
            mode & 0o022 != 0
        };
        if (metadata.uid() != 0 && metadata.uid() != geteuid().as_raw()) || unsafe_mode {
            return Err(Error::Configuration);
        }
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::UnsafeFile)?;
    if bytes.len() > limit {
        return Err(Error::UnsafeFile);
    }
    Ok(bytes)
}
