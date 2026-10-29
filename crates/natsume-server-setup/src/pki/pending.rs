//! Persisted offline signing requests with bounded, symlink-resistant file access.

use super::{
    Certificate, PrivateKey,
    generate::SigningRequest,
    import::{self, ImportError},
};
use crate::{
    config::{self, ConfigRenderError, DeploymentParameters},
    deployment::file::{self, CreatedFile, DirectoryRemoval, FileError, FileSpec, Ownership},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use snafu::{OptionExt as _, ResultExt as _, Snafu, ensure};
use std::{
    fs, io,
    net::IpAddr,
    path::{Path, PathBuf},
};
use time::OffsetDateTime;
use x509_parser::prelude::FromDer as _;

const KEY_FILE: &str = "server-tls-key.pk8";
const CSR_FILE: &str = "server-tls.csr";
const CA_FILE: &str = "control-ca.crt";
const REQUEST_FILE: &str = "request.toml";
const MAX_PENDING_FILE: u64 = 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestRecord {
    format_version: u32,
    server_ips: Vec<IpAddr>,
    https_port: u16,
    gateway_hostname: String,
    domjudge_origin: String,
    contest_end: String,
    gateway_not_after: String,
    control_ca_sha256: String,
    control_ca_spki_sha256: String,
    csr_public_key_sha256: String,
}

/// A resumable Server signing request; private key bytes remain in a redacted container.
#[derive(Debug)]
pub struct PendingRequest {
    params: DeploymentParameters,
    control_ca: Certificate,
    key: PrivateKey,
    csr_pem: String,
    path: PathBuf,
    pki_dir: PathBuf,
    files: Vec<CreatedFile>,
}

impl PendingRequest {
    /// Publishes a new request without writing any system deployment paths or overwriting files.
    ///
    /// # Errors
    /// Rejects existing requests, invalid paths/materials, unsafe metadata, or filesystem failures.
    pub fn create(
        target_root: &Path,
        pki_dir: &Path,
        params: &DeploymentParameters,
        control_ca: &Certificate,
        key: &PrivateKey,
        csr: &SigningRequest,
        owner: Ownership,
    ) -> Result<Self, PendingError> {
        create_request(target_root, pki_dir, params, control_ca, key, csr, owner)
    }

    /// Loads and revalidates all persisted inputs; absence of pending is an ordinary result.
    ///
    /// # Errors
    /// Rejects symlinks, unsafe ownership/permissions, corrupt or inconsistent records, and expired parameters.
    pub fn load(
        target_root: &Path,
        pki_dir: &Path,
        now: OffsetDateTime,
        owner: Ownership,
    ) -> Result<Option<Self>, PendingError> {
        load_request(target_root, pki_dir, now, owner)
    }

    /// Checks direct issuance, matching key, and coverage of every pending Server IP.
    ///
    /// Full certificate policy remains the responsibility of `validate_materials` before deployment.
    ///
    /// # Errors
    /// Returns signature/provider errors or an explicit issuer, key, or SAN mismatch.
    pub fn accept_signed(&self, certificate: &Certificate) -> Result<(), PendingError> {
        let (_, issuer) =
            x509_parser::parse_x509_certificate(self.control_ca.der()).context(CertificateSnafu)?;
        let (_, leaf) =
            x509_parser::parse_x509_certificate(certificate.der()).context(CertificateSnafu)?;
        ensure!(leaf.issuer() == issuer.subject(), IssuerMismatchSnafu);
        leaf.verify_signature(Some(issuer.public_key()))
            .context(SignatureSnafu)?;
        ensure!(self.key.matches(certificate), KeyMismatchSnafu);
        let sans = certificate.ip_sans();
        ensure!(
            self.params.server_ips().iter().all(|ip| sans.contains(ip)),
            MissingIpSnafu
        );
        Ok(())
    }

    /// Removes only the four unchanged known files, preserving any unrelated directory entries.
    ///
    /// # Errors
    /// Rejects changed files, symlinks, a different target root, or filesystem failures.
    pub fn remove(self, target_root: &Path) -> Result<DirectoryRemoval, PendingError> {
        ensure!(
            file::rooted_pki(target_root, &self.pki_dir)
                .context(FilesystemSnafu)?
                .join("pending")
                == self.path,
            TargetMismatchSnafu
        );
        for created in &self.files {
            file::remove_created_file(created).context(FilesystemSnafu)?;
        }
        file::remove_created_directory(&self.path).context(FilesystemSnafu)
    }

    /// Borrows revalidated parameters for installation continuation.
    #[must_use]
    pub const fn params(&self) -> &DeploymentParameters {
        &self.params
    }
    /// Borrows the pinned offline issuer certificate.
    #[must_use]
    pub const fn control_ca(&self) -> &Certificate {
        &self.control_ca
    }
    /// Borrows private material without duplicating it or exposing it through Debug.
    #[must_use]
    pub const fn key(&self) -> &PrivateKey {
        &self.key
    }
    /// Borrows the public signing request exactly as persisted.
    #[must_use]
    pub fn csr_pem(&self) -> &str {
        &self.csr_pem
    }
    /// Returns the pending directory beneath the selected target root.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
    /// Returns the public request path for offline-signing instructions.
    #[must_use]
    pub fn csr_path(&self) -> PathBuf {
        self.path.join(CSR_FILE)
    }
    /// Returns the expected issuer certificate path for offline signing.
    #[must_use]
    pub fn control_ca_path(&self) -> PathBuf {
        self.path.join(CA_FILE)
    }
}

fn create_request(
    target_root: &Path,
    pki_dir: &Path,
    params: &DeploymentParameters,
    control_ca: &Certificate,
    key: &PrivateKey,
    csr: &SigningRequest,
    owner: Ownership,
) -> Result<PendingRequest, PendingError> {
    let pki_path = file::rooted_pki(target_root, pki_dir).context(FilesystemSnafu)?;
    let path = pki_path.join("pending");
    let record = RequestRecord {
        format_version: 1,
        server_ips: params.server_ips().to_vec(),
        https_port: params.https_port(),
        gateway_hostname: params.gateway_hostname().into(),
        domjudge_origin: params.domjudge_origin().into(),
        contest_end: params.contest_end().into(),
        gateway_not_after: params.gateway_not_after().into(),
        control_ca_sha256: hex::encode(Sha256::digest(control_ca.der())),
        control_ca_spki_sha256: hex::encode(control_ca.spki_sha256()),
        csr_public_key_sha256: hex::encode(Sha256::digest(&key.spki)),
    };
    verify_csr(csr.pem(), key, &record)?;
    let text = toml::to_string_pretty(&record).context(SerializeSnafu)?;
    let mut directories =
        file::ensure_directory(&pki_path, owner, 0o700).context(FilesystemSnafu)?;
    file::check_directory(&pki_path, owner, 0o700).context(FilesystemSnafu)?;
    match fs::symlink_metadata(&path) {
        Ok(_) => return ExistingSnafu { path }.fail(),
        Err(source) if source.kind() == io::ErrorKind::NotFound => {}
        Err(source) => return Err(source).context(IoSnafu { path }),
    }
    directories.extend(file::ensure_directory(&path, owner, 0o700).context(FilesystemSnafu)?);
    let mut files = Vec::new();
    let result = (|| {
        for (name, contents) in [
            (KEY_FILE, key.pkcs8_der()),
            (CSR_FILE, csr.pem().as_bytes()),
            (CA_FILE, control_ca.to_pem().as_bytes()),
            (REQUEST_FILE, text.as_bytes()),
        ] {
            files.push(
                file::create_new_file(
                    &FileSpec {
                        path: path.join(name),
                        owner,
                        mode: 0o600,
                    },
                    contents,
                )
                .context(FilesystemSnafu)?,
            );
        }
        load_request(target_root, pki_dir, OffsetDateTime::UNIX_EPOCH, owner)?
            .context(DisappearedSnafu)
    })();
    if result.is_err() {
        file::cleanup(&files, &directories);
    }
    result
}

fn load_request(
    target_root: &Path,
    pki_dir: &Path,
    now: OffsetDateTime,
    owner: Ownership,
) -> Result<Option<PendingRequest>, PendingError> {
    let pki_path = file::rooted_pki(target_root, pki_dir).context(FilesystemSnafu)?;
    let path = pki_path.join("pending");
    match fs::symlink_metadata(&path) {
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(source).context(IoSnafu { path }),
        Ok(_) => {}
    }
    file::check_directory(&pki_path, owner, 0o700).context(FilesystemSnafu)?;
    file::check_directory(&path, owner, 0o700).context(FilesystemSnafu)?;
    let mut files = Vec::new();
    let (record_bytes, created) =
        file::read_file(&path.join(REQUEST_FILE), owner, 0o600, MAX_PENDING_FILE)
            .context(FilesystemSnafu)?;
    files.push(created);
    let record: RequestRecord =
        toml::from_str(std::str::from_utf8(&record_bytes).context(TextSnafu)?)
            .context(RecordSnafu)?;
    ensure!(
        record.format_version == 1,
        VersionSnafu {
            version: record.format_version
        }
    );
    let params = DeploymentParameters::new(
        record.server_ips.clone(),
        record.https_port,
        record.gateway_hostname.clone(),
        record.domjudge_origin.clone(),
        record.contest_end.clone(),
        record.gateway_not_after.clone(),
        now,
    )
    .context(ParametersSnafu)?;
    config::validate_server_config(
        &config::render_server_config(&params).context(ParametersSnafu)?,
    )
    .context(ParametersSnafu)?;
    let (ca_bytes, created) = file::read_file(&path.join(CA_FILE), owner, 0o600, MAX_PENDING_FILE)
        .context(FilesystemSnafu)?;
    files.push(created);
    let control_ca = import::import_certificate(&ca_bytes).context(ImportSnafu)?;
    ensure!(
        hex::encode(Sha256::digest(control_ca.der())) == record.control_ca_sha256
            && hex::encode(control_ca.spki_sha256()) == record.control_ca_spki_sha256,
        FingerprintSnafu
    );
    let (key_bytes, created) =
        file::read_file(&path.join(KEY_FILE), owner, 0o600, MAX_PENDING_FILE)
            .context(FilesystemSnafu)?;
    files.push(created);
    let key = import::import_private_key(&key_bytes, None).context(ImportSnafu)?;
    ensure!(
        hex::encode(Sha256::digest(&key.spki)) == record.csr_public_key_sha256,
        KeyMismatchSnafu
    );
    let (csr_bytes, created) =
        file::read_file(&path.join(CSR_FILE), owner, 0o600, MAX_PENDING_FILE)
            .context(FilesystemSnafu)?;
    files.push(created);
    let csr_pem = std::str::from_utf8(&csr_bytes)
        .context(TextSnafu)?
        .to_owned();
    verify_csr(&csr_pem, &key, &record)?;
    Ok(Some(PendingRequest {
        params,
        control_ca,
        key,
        csr_pem,
        path,
        pki_dir: pki_dir.into(),
        files,
    }))
}

fn verify_csr(pem: &str, key: &PrivateKey, record: &RequestRecord) -> Result<(), PendingError> {
    let (remaining, block) =
        x509_parser::pem::parse_x509_pem(pem.as_bytes()).context(CsrPemSnafu)?;
    ensure!(
        remaining.trim_ascii().is_empty() && block.label == "CERTIFICATE REQUEST",
        InvalidCsrSnafu
    );
    let (remaining, csr) =
        x509_parser::certification_request::X509CertificationRequest::from_der(&block.contents)
            .context(CertificateSnafu)?;
    ensure!(remaining.is_empty(), InvalidCsrSnafu);
    csr.verify_signature().context(SignatureSnafu)?;
    ensure!(
        csr.certification_request_info.subject_pki.raw == key.spki
            && hex::encode(Sha256::digest(
                csr.certification_request_info.subject_pki.raw
            )) == record.csr_public_key_sha256,
        KeyMismatchSnafu
    );
    Ok(())
}

/// Failure to safely persist or resume an offline signing request.
#[derive(Debug, Snafu)]
pub enum PendingError {
    /// A previous request or another entry already occupies pending.
    #[snafu(display("pending request already exists: {}", path.display()))]
    Existing {
        /// Occupied pending directory.
        path: PathBuf,
    },
    /// A deployment filesystem primitive failed.
    #[snafu(display("pending request filesystem operation failed"))]
    Filesystem {
        /// Original safe-publication or removal cause.
        source: FileError,
    },
    /// Reading or inspecting a persisted input failed.
    #[snafu(display("cannot access pending input: {}", path.display()))]
    Io {
        /// Input path without file contents.
        path: PathBuf,
        /// Original operating system cause.
        source: io::Error,
    },
    /// A non-secret persisted text file is malformed.
    #[snafu(display("pending input is not valid UTF-8"))]
    Text {
        /// Original UTF-8 decoding cause.
        source: std::str::Utf8Error,
    },
    /// TOML encoding failed before publication.
    #[snafu(display("cannot encode pending request record"))]
    Serialize {
        /// Original TOML encoder cause.
        source: toml::ser::Error,
    },
    /// Persisted TOML is malformed or contains unexpected fields.
    #[snafu(display("invalid pending request record"))]
    Record {
        /// Original TOML/schema decoder cause.
        source: toml::de::Error,
    },
    /// A newer or invalid request format cannot be resumed.
    #[snafu(display("unsupported pending request version {version}"))]
    Version {
        /// Rejected format version.
        version: u32,
    },
    /// Parameters became invalid or expired since the request was created.
    #[snafu(display("pending parameters are invalid or expired; delete pending and start again"))]
    Parameters {
        /// Original parameter or Server-loader rejection.
        source: ConfigRenderError,
    },
    /// Persisted CA certificate does not match the request's issuer record.
    #[snafu(display("pending Control CA fingerprint mismatch"))]
    Fingerprint,
    /// Persisted key or returned certificate does not match the request public key.
    #[snafu(display("pending Server public key mismatch"))]
    KeyMismatch,
    /// A certificate or key could not be decoded.
    #[snafu(display("pending cryptographic material could not be decoded"))]
    Import {
        /// Original material parser cause.
        source: ImportError,
    },
    /// An X.509 certificate or CSR could not be decoded.
    #[snafu(display("pending certificate or CSR could not be parsed"))]
    Certificate {
        /// Original ASN.1 decoding cause.
        source: x509_parser::nom::Err<x509_parser::error::X509Error>,
    },
    /// The public signing request has malformed PEM encoding.
    #[snafu(display("pending CSR PEM could not be parsed"))]
    CsrPem {
        /// Original PEM parsing cause.
        source: x509_parser::nom::Err<x509_parser::error::PEMError>,
    },
    /// The request has extra material or an unexpected PEM label.
    #[snafu(display("invalid pending CSR encoding"))]
    InvalidCsr,
    /// A direct issuer or CSR signature could not be verified.
    #[snafu(display("pending signature verification failed"))]
    Signature {
        /// Original signature verifier cause.
        source: x509_parser::error::X509Error,
    },
    /// Returned certificate names another issuer.
    #[snafu(display("signed certificate does not name the pending Control CA"))]
    IssuerMismatch,
    /// Returned certificate omits an advertised endpoint address.
    #[snafu(display("signed certificate does not cover every pending Server IP"))]
    MissingIp,
    /// The requested removal root differs from the loaded request root.
    #[snafu(display("pending removal target root mismatch"))]
    TargetMismatch,
    /// Publication succeeded but the request vanished before its verification read.
    #[snafu(display("pending request disappeared during creation"))]
    Disappeared,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pki::generate::{self, GenerateError};
    use rcgen::{CertificateSigningRequestParams, Issuer};
    use rustls_pki_types::CertificateDer;
    use std::{
        net::Ipv4Addr,
        os::unix::fs::{MetadataExt as _, PermissionsExt as _, symlink},
    };
    use tempfile::TempDir;

    #[derive(Debug, Snafu)]
    enum TestError {
        #[snafu(context(false), display("pending fixture I/O failed"))]
        Io { source: io::Error },
        #[snafu(context(false), display("pending fixture parameters failed"))]
        Config { source: ConfigRenderError },
        #[snafu(context(false), display("pending fixture generation failed"))]
        Generate { source: GenerateError },
        #[snafu(context(false), display("pending request failed"))]
        Pending { source: PendingError },
        #[snafu(context(false), display("pending fixture certificate failed"))]
        Rcgen { source: rcgen::Error },
        #[snafu(context(false), display("pending fixture import failed"))]
        Import { source: ImportError },
        #[snafu(display("fixture request decode failed"))]
        TestRecord { source: toml::de::Error },
        #[snafu(display("fixture request encode failed"))]
        TestSerialize { source: toml::ser::Error },
        #[snafu(display("fixture time formatting failed"))]
        FixtureTime { source: time::error::Format },
        #[snafu(display("expected pending request is missing"))]
        Missing,
    }

    struct Fixture {
        root: TempDir,
        params: DeploymentParameters,
        ca: Certificate,
        ca_key: PrivateKey,
        key: PrivateKey,
        csr: SigningRequest,
        owner: Ownership,
        now: OffsetDateTime,
    }
    impl Fixture {
        fn new() -> Result<Self, TestError> {
            let root = tempfile::tempdir()?;
            let now = OffsetDateTime::now_utc();
            let params = DeploymentParameters::new(
                vec![
                    Ipv4Addr::LOCALHOST.into(),
                    std::net::Ipv6Addr::LOCALHOST.into(),
                ],
                8443,
                "gateway.example.test".into(),
                "https://judge.example.test".into(),
                (now + time::Duration::days(1))
                    .format(&time::format_description::well_known::Rfc3339)
                    .context(FixtureTimeSnafu)?,
                (now + time::Duration::days(2))
                    .format(&time::format_description::well_known::Rfc3339)
                    .context(FixtureTimeSnafu)?,
                now,
            )?;
            let (ca, ca_key) = generate::generate_ca("Pending CA", "Natsume", now)?;
            let (key, csr) = generate::create_csr(params.server_ips())?;
            Ok(Self {
                root,
                params,
                ca,
                ca_key,
                key,
                csr,
                owner: Ownership {
                    uid: rustix::process::geteuid().as_raw(),
                    gid: rustix::process::getegid().as_raw(),
                },
                now,
            })
        }
        fn create(&self) -> Result<PendingRequest, PendingError> {
            PendingRequest::create(
                self.root.path(),
                Path::new("/pki"),
                &self.params,
                &self.ca,
                &self.key,
                &self.csr,
                self.owner,
            )
        }
        fn load(&self) -> Result<Option<PendingRequest>, PendingError> {
            PendingRequest::load(self.root.path(), Path::new("/pki"), self.now, self.owner)
        }
        fn signed(
            &self,
            ca: &Certificate,
            ca_key: &PrivateKey,
            csr: &SigningRequest,
        ) -> Result<Certificate, TestError> {
            let mut request = CertificateSigningRequestParams::from_pem(csr.pem())?;
            request.params.not_before = self.now - time::Duration::hours(1);
            request.params.not_after = self.now + time::Duration::days(10);
            let issuer =
                Issuer::from_ca_cert_der(&CertificateDer::from(ca.der()), ca_key.key_pair()?)?;
            Ok(import::import_certificate(
                request.signed_by(&issuer)?.der(),
            )?)
        }
    }

    #[test]
    fn create_load_and_accept_roundtrip() -> Result<(), TestError> {
        let fixture = Fixture::new()?;
        assert!(fixture.load()?.is_none());
        let created = fixture.create()?;
        let loaded = fixture.load()?.context(MissingSnafu)?;
        assert_eq!(loaded.params(), &fixture.params);
        assert_eq!(loaded.control_ca(), &fixture.ca);
        assert_eq!(loaded.key().pkcs8_der(), fixture.key.pkcs8_der());
        assert_eq!(loaded.csr_pem(), fixture.csr.pem());
        assert_eq!(loaded.path(), created.path());
        assert!(!format!("{loaded:?}").contains(&hex::encode(fixture.key.pkcs8_der())));
        let signed = fixture.signed(&fixture.ca, &fixture.ca_key, &fixture.csr)?;
        loaded.accept_signed(&signed)?;
        for name in [KEY_FILE, CSR_FILE, CA_FILE, REQUEST_FILE] {
            let metadata = fs::metadata(loaded.path().join(name))?;
            assert_eq!(metadata.mode() & 0o7777, 0o600);
            assert_eq!(
                (metadata.uid(), metadata.gid()),
                (fixture.owner.uid, fixture.owner.gid)
            );
        }
        assert!(matches!(
            fixture.create(),
            Err(PendingError::Existing { .. })
        ));
        assert_eq!(
            loaded.remove(fixture.root.path())?,
            DirectoryRemoval::Removed
        );
        assert!(fixture.load()?.is_none());
        Ok(())
    }

    #[test]
    fn tampering_and_unknown_fields_are_rejected() -> Result<(), TestError> {
        for kind in ["version", "unknown", "hash", "ca", "key", "csr", "expired"] {
            let fixture = Fixture::new()?;
            let pending = fixture.create()?;
            let record_path = pending.path().join(REQUEST_FILE);
            match kind {
                "version" => {
                    let text = fs::read_to_string(&record_path)?
                        .replace("format_version = 1", "format_version = 2");
                    fs::write(record_path, text)?;
                }
                "unknown" => {
                    let mut text = fs::read_to_string(&record_path)?;
                    text.push_str("unknown = true\n");
                    fs::write(record_path, text)?;
                }
                "hash" => {
                    let mut record: RequestRecord =
                        toml::from_str(&fs::read_to_string(&record_path)?)
                            .context(TestRecordSnafu)?;
                    record.control_ca_sha256 = "00".into();
                    fs::write(
                        record_path,
                        toml::to_string(&record).context(TestSerializeSnafu)?,
                    )?;
                }
                "ca" => {
                    let (ca, _) = generate::generate_ca("Other", "Natsume", fixture.now)?;
                    fs::write(pending.path().join(CA_FILE), ca.to_pem())?;
                }
                "key" => {
                    let (key, _) = generate::create_csr(fixture.params.server_ips())?;
                    fs::write(pending.path().join(KEY_FILE), key.pkcs8_der())?;
                }
                "csr" => {
                    let (_, csr) = generate::create_csr(fixture.params.server_ips())?;
                    fs::write(pending.path().join(CSR_FILE), csr.pem())?;
                }
                "expired" => {
                    assert!(matches!(
                        PendingRequest::load(
                            fixture.root.path(),
                            Path::new("/pki"),
                            fixture.now + time::Duration::days(5),
                            fixture.owner
                        ),
                        Err(PendingError::Parameters { .. })
                    ));
                    continue;
                }
                _ => unreachable!("fixture case is finite"),
            }
            assert!(fixture.load().is_err(), "accepted tampered {kind}");
        }
        Ok(())
    }

    #[test]
    fn signed_certificate_must_have_issuer_key_and_all_ips() -> Result<(), TestError> {
        let fixture = Fixture::new()?;
        let pending = fixture.create()?;
        let (other_ca, other_key) = generate::generate_ca("Other CA", "Natsume", fixture.now)?;
        let wrong_issuer = fixture.signed(&other_ca, &other_key, &fixture.csr)?;
        assert!(pending.accept_signed(&wrong_issuer).is_err());
        let (impostor, impostor_key) = generate::generate_ca("Pending CA", "Natsume", fixture.now)?;
        let wrong_signature = fixture.signed(&impostor, &impostor_key, &fixture.csr)?;
        assert!(matches!(
            pending.accept_signed(&wrong_signature),
            Err(PendingError::Signature { .. })
        ));
        let (_, other_csr) = generate::create_csr(fixture.params.server_ips())?;
        let wrong_key = fixture.signed(&fixture.ca, &fixture.ca_key, &other_csr)?;
        assert!(matches!(
            pending.accept_signed(&wrong_key),
            Err(PendingError::KeyMismatch)
        ));
        let mut params = rcgen::CertificateParams::default();
        params.subject_alt_names = vec![rcgen::SanType::IpAddress(Ipv4Addr::LOCALHOST.into())];
        let request = params.serialize_request(&fixture.key.key_pair()?)?;
        let request = sign_request(&fixture, &request)?;
        assert!(matches!(
            pending.accept_signed(&request),
            Err(PendingError::MissingIp)
        ));
        Ok(())
    }

    fn sign_request(
        fixture: &Fixture,
        request: &rcgen::CertificateSigningRequest,
    ) -> Result<Certificate, TestError> {
        let csr = CertificateSigningRequestParams::from_der(request.der())?;
        let issuer = Issuer::from_ca_cert_der(
            &CertificateDer::from(fixture.ca.der()),
            fixture.ca_key.key_pair()?,
        )?;
        Ok(import::import_certificate(csr.signed_by(&issuer)?.der())?)
    }

    #[test]
    fn removal_preserves_other_entries_and_rejects_changed_files() -> Result<(), TestError> {
        let fixture = Fixture::new()?;
        let pending = fixture.create()?;
        let unrelated = pending.path().join("operator-notes");
        fs::write(&unrelated, b"keep")?;
        assert_eq!(
            pending.remove(fixture.root.path())?,
            DirectoryRemoval::KeptNotEmpty
        );
        assert_eq!(fs::read(&unrelated)?, b"keep");
        let fixture = Fixture::new()?;
        let pending = fixture.create()?;
        let key_path = pending.path().join(KEY_FILE);
        fs::remove_file(&key_path)?;
        let external = fixture.root.path().join("external");
        fs::write(&external, b"do not remove")?;
        symlink(&external, &key_path)?;
        assert!(pending.remove(fixture.root.path()).is_err());
        assert_eq!(fs::read(external)?, b"do not remove");
        Ok(())
    }

    #[test]
    fn loading_rejects_symlinks_and_loose_permissions() -> Result<(), TestError> {
        for name in [KEY_FILE, CSR_FILE, CA_FILE, REQUEST_FILE] {
            let fixture = Fixture::new()?;
            let pending = fixture.create()?;
            let path = pending.path().join(name);
            let external = fixture.root.path().join("saved");
            fs::rename(&path, &external)?;
            symlink(&external, &path)?;
            assert!(fixture.load().is_err(), "accepted symlink {name}");
        }
        let fixture = Fixture::new()?;
        let pending = fixture.create()?;
        fs::set_permissions(pending.path(), fs::Permissions::from_mode(0o755))?;
        assert!(matches!(
            fixture.load(),
            Err(PendingError::Filesystem {
                source: FileError::Metadata { .. }
            })
        ));
        Ok(())
    }
}
