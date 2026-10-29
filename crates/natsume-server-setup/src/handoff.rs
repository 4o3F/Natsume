//! Public Client configuration and trust anchors exported with reproducible SHA-256 checksums.

use crate::{
    config::{self, ConfigRenderError, DeploymentParameters},
    deployment::file::{self, CreatedFile, FileError, FileSpec, Ownership},
    pki::Certificate,
};
use sha2::{Digest as _, Sha256};
use snafu::{ResultExt as _, Snafu};
use std::{
    fmt::Write as _,
    fs, io,
    path::{Path, PathBuf},
};
use time::{OffsetDateTime, format_description};

/// Certificate identity information suitable for administrator handoff and comparison.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertificateSummary {
    /// SHA-256 of the complete DER certificate, in display format.
    pub fingerprint: String,
    /// First valid instant recorded in the certificate.
    pub not_before: OffsetDateTime,
    /// Last valid instant recorded in the certificate.
    pub not_after: OffsetDateTime,
}

impl From<&Certificate> for CertificateSummary {
    fn from(certificate: &Certificate) -> Self {
        Self {
            fingerprint: certificate.sha256_fingerprint(),
            not_before: certificate.not_before(),
            not_after: certificate.not_after(),
        }
    }
}

/// Completed export location and public identities to verify on Client machines.
#[derive(Debug)]
pub struct HandoffReport {
    /// Unique UTC timestamp directory containing only public installation material.
    pub directory: PathBuf,
    /// Control trust anchor identity used for Server authentication.
    pub control_ca: CertificateSummary,
    /// Origin trust anchor identity used for Gateway authentication.
    pub origin_ca: CertificateSummary,
    /// File creation records available until the wizard finishes reporting installation.
    pub(crate) created_files: Vec<CreatedFile>,
    /// Directories created by this export, retained for attempt-scoped rollback.
    pub(crate) created_directories: Vec<PathBuf>,
}

/// Writes a new public Client handoff without replacing an existing timestamp directory.
///
/// # Errors
/// Rejects unsafe PKI metadata, occupied directories, rendering errors, and filesystem failures.
pub fn write_handoff(
    target_root: &Path,
    pki_dir: &Path,
    params: &DeploymentParameters,
    control_ca: &Certificate,
    origin_ca: &Certificate,
    now: OffsetDateTime,
    owner: Ownership,
) -> Result<HandoffReport, HandoffError> {
    let pki_path = file::rooted_pki(target_root, pki_dir).context(PkiSnafu)?;
    let format =
        format_description::parse_borrowed::<2>("[year][month][day]T[hour][minute][second]Z")
            .context(TimestampFormatSnafu)?;
    let timestamp = now
        .to_offset(time::UtcOffset::UTC)
        .format(&format)
        .context(TimestampSnafu)?;
    let directory = pki_path.join("handoff").join(timestamp);
    let mut directories =
        file::ensure_directory(&pki_path, owner, 0o700).context(FilesystemSnafu)?;
    file::check_directory(&pki_path, owner, 0o700).context(PkiSnafu)?;
    match fs::symlink_metadata(&directory) {
        Ok(_) => return ExistingSnafu { path: directory }.fail(),
        Err(source) if source.kind() == io::ErrorKind::NotFound => {}
        Err(source) => return Err(source).context(InspectSnafu { path: directory }),
    }
    directories.extend(file::ensure_directory(&directory, owner, 0o755).context(FilesystemSnafu)?);
    let mut files = Vec::new();
    let result = (|| {
        let contents = [
            (
                "config.toml",
                config::render_client_config(params).context(ConfigSnafu)?,
            ),
            ("control-ca.crt", control_ca.to_pem()),
            ("local-origin-ca.crt", origin_ca.to_pem()),
        ];
        let mut checksums = String::new();
        for (name, text) in &contents {
            let digest = hex::encode(Sha256::digest(text.as_bytes()));
            writeln!(checksums, "{digest}  {name}").context(ChecksumSnafu)?;
            files.push(
                file::create_new_file(
                    &FileSpec {
                        path: directory.join(name),
                        owner,
                        mode: 0o644,
                    },
                    text.as_bytes(),
                )
                .context(FilesystemSnafu)?,
            );
        }
        files.push(
            file::create_new_file(
                &FileSpec {
                    path: directory.join("SHA256SUMS"),
                    owner,
                    mode: 0o644,
                },
                checksums.as_bytes(),
            )
            .context(FilesystemSnafu)?,
        );
        Ok(HandoffReport {
            directory: directory.clone(),
            control_ca: control_ca.into(),
            origin_ca: origin_ca.into(),
            created_files: files.clone(),
            created_directories: directories.clone(),
        })
    })();
    if result.is_err() {
        file::cleanup(&files, &directories);
    }
    result
}

/// Failure to publish an administrator's public Client handoff.
#[derive(Debug, Snafu)]
pub enum HandoffError {
    /// The timestamp directory already contains an earlier export.
    #[snafu(display("handoff directory already exists: {}", path.display()))]
    Existing {
        /// Existing export directory left unchanged.
        path: PathBuf,
    },
    /// Inspecting the destination failed before publication.
    #[snafu(display("cannot inspect handoff path: {}", path.display()))]
    Inspect {
        /// Destination being inspected.
        path: PathBuf,
        /// Original filesystem inspection cause.
        source: io::Error,
    },
    /// PKI directory resolution or metadata was unsafe.
    #[snafu(display("unsafe administrator PKI directory"))]
    Pki {
        /// Original path or metadata rejection.
        source: FileError,
    },
    /// A protected publication primitive failed.
    #[snafu(display("handoff filesystem operation failed"))]
    Filesystem {
        /// Original safe-publication cause.
        source: FileError,
    },
    /// Client configuration could not be serialized.
    #[snafu(display("cannot render Client handoff configuration"))]
    Config {
        /// Original renderer cause.
        source: ConfigRenderError,
    },
    /// The fixed timestamp format could not be constructed.
    #[snafu(display("cannot construct handoff timestamp format"))]
    TimestampFormat {
        /// Original format-description parsing cause.
        source: time::error::InvalidFormatDescription,
    },
    /// The supplied timestamp could not be formatted.
    #[snafu(display("cannot format UTC handoff timestamp"))]
    Timestamp {
        /// Original timestamp formatting cause.
        source: time::error::Format,
    },
    /// Writing the checksum manifest failed.
    #[snafu(display("cannot format handoff checksum manifest"))]
    Checksum {
        /// Original formatting cause.
        source: std::fmt::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pki::generate::{self, GenerateError};
    use std::{net::Ipv4Addr, os::unix::fs::MetadataExt as _};

    #[derive(Debug, Snafu)]
    enum TestError {
        #[snafu(context(false), display("handoff fixture I/O failed"))]
        Io { source: io::Error },
        #[snafu(context(false), display("handoff fixture parameters failed"))]
        Config { source: ConfigRenderError },
        #[snafu(context(false), display("handoff fixture generation failed"))]
        Generate { source: GenerateError },
        #[snafu(context(false), display("handoff write failed"))]
        Handoff { source: HandoffError },
    }

    #[test]
    fn writes_public_material_metadata_and_verifiable_sorted_manifest() -> Result<(), TestError> {
        let root = tempfile::tempdir()?;
        let now = OffsetDateTime::UNIX_EPOCH;
        let owner = Ownership {
            uid: rustix::process::geteuid().as_raw(),
            gid: rustix::process::getegid().as_raw(),
        };
        let params = DeploymentParameters::new(
            vec![Ipv4Addr::LOCALHOST.into()],
            8443,
            "gateway.example.test".into(),
            "https://judge.example.test".into(),
            "2030-01-01T00:00:00Z".into(),
            "2030-01-02T00:00:00Z".into(),
            now,
        )?;
        let (control, _) = generate::generate_ca("Control", "Natsume", now)?;
        let (origin, _) = generate::generate_ca("Origin", "Natsume", now)?;
        let report = write_handoff(
            root.path(),
            Path::new("/pki"),
            &params,
            &control,
            &origin,
            now,
            owner,
        )?;
        assert_eq!(
            report.directory,
            root.path().join("pki/handoff/19700101T000000Z")
        );
        assert_eq!(report.control_ca.fingerprint, control.sha256_fingerprint());
        assert_eq!(report.origin_ca.not_after, origin.not_after());
        assert_eq!(
            fs::read_to_string(report.directory.join("config.toml"))?,
            config::render_client_config(&params)?
        );
        assert_eq!(
            fs::read_to_string(report.directory.join("control-ca.crt"))?,
            control.to_pem()
        );
        assert_eq!(
            fs::read_to_string(report.directory.join("local-origin-ca.crt"))?,
            origin.to_pem()
        );
        let manifest = fs::read_to_string(report.directory.join("SHA256SUMS"))?;
        let mut names = Vec::new();
        for line in manifest.lines() {
            let Some((digest, name)) = line.split_once("  ") else {
                panic!("invalid sha256sum line: {line}");
            };
            assert_eq!(
                digest,
                hex::encode(Sha256::digest(fs::read(report.directory.join(name))?))
            );
            names.push(name);
        }
        assert_eq!(
            names,
            ["config.toml", "control-ca.crt", "local-origin-ca.crt"]
        );
        for name in [
            "config.toml",
            "control-ca.crt",
            "local-origin-ca.crt",
            "SHA256SUMS",
        ] {
            let metadata = fs::metadata(report.directory.join(name))?;
            assert_eq!(metadata.mode() & 0o7777, 0o644);
            assert_eq!((metadata.uid(), metadata.gid()), (owner.uid, owner.gid));
        }
        for (directory, mode) in [
            (root.path().join("pki"), 0o700),
            (root.path().join("pki/handoff"), 0o755),
            (report.directory.clone(), 0o755),
        ] {
            assert_eq!(fs::metadata(directory)?.mode() & 0o7777, mode);
        }
        assert!(matches!(
            write_handoff(
                root.path(),
                Path::new("/pki"),
                &params,
                &control,
                &origin,
                now,
                owner
            ),
            Err(HandoffError::Existing { .. })
        ));
        assert_eq!(
            fs::read_to_string(report.directory.join("SHA256SUMS"))?,
            manifest
        );
        Ok(())
    }
}
