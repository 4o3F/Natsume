//! Deployment planning for first-install setup.

use std::{
    fmt, fs, io,
    path::{Path, PathBuf},
};

use snafu::{ResultExt as _, Snafu, ensure};
use zeroize::Zeroizing;

use super::file::{FileSpec, Ownership};
use crate::{
    config,
    pki::{Certificate, PrivateKey},
};

const DEPLOYMENT_FILES: [&str; 7] = [
    config::ORIGIN_CA_DER,
    config::ORIGIN_CA_KEY,
    config::SERVER_TLS_LEAF,
    config::SERVER_TLS_KEY,
    config::CONTROL_CA_TRUST,
    config::LOCAL_ORIGIN_TRUST,
    config::CONFIG_PATH,
];
const ROOT: Ownership = Ownership { uid: 0, gid: 0 };

/// First-install detection failure retaining filesystem causes.
#[derive(Debug, Snafu)]
#[non_exhaustive]
pub enum PlanError {
    /// One or more deployment destinations are already occupied.
    #[snafu(display(
        "setup is only for first installation; use the operations manual for existing deployments: {paths:?}"
    ))]
    AlreadyDeployed {
        /// Occupied deployment destinations beneath the target root.
        paths: Vec<PathBuf>,
    },
    /// A required real directory is absent or invalid; install the Server Deb before setup.
    #[snafu(display("required directory missing or invalid; install the Server Deb first: {}", path.display()))]
    MissingPrerequisite {
        /// Required directory beneath the target root.
        path: PathBuf,
    },
    /// A deployment path could not be inspected.
    #[snafu(display("cannot inspect deployment path: {}", path.display()))]
    Inspect {
        /// Path whose filesystem metadata could not be read.
        path: PathBuf,
        /// Original filesystem inspection cause.
        source: io::Error,
    },
}

/// Validated PKI materials and validated Server configuration borrowed for planning.
#[derive(Clone, Copy)]
pub struct PlanMaterials<'a> {
    /// Public Control CA trust anchor.
    pub control_ca: &'a Certificate,
    /// Server TLS leaf certificate.
    pub tls_leaf: &'a Certificate,
    /// Private key matching the Server TLS leaf.
    pub tls_key: &'a PrivateKey,
    /// Online Local Origin CA certificate and public trust anchor.
    pub origin_ca: &'a Certificate,
    /// Online private key matching the Local Origin CA.
    pub origin_key: &'a PrivateKey,
    /// Validated Server configuration text.
    pub config: &'a str,
}

impl fmt::Debug for PlanMaterials<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PlanMaterials { <redacted> }")
    }
}

pub(super) struct PlannedFile {
    pub(super) spec: FileSpec,
    pub(super) contents: Zeroizing<Vec<u8>>,
}

impl fmt::Debug for PlannedFile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.spec.fmt(formatter)
    }
}

#[derive(Debug)]
pub(super) struct PlannedDirectory {
    pub(super) path: PathBuf,
    pub(super) owner: Ownership,
    pub(super) mode: u32,
}

/// Ordered first-install files and directories, with contents excluded from diagnostics.
#[derive(Debug)]
pub struct DeploymentPlan {
    pub(super) files: Vec<PlannedFile>,
    pub(super) directories: Vec<PlannedDirectory>,
}

/// Finds occupied deployment file paths, including dangling symbolic links.
///
/// # Errors
/// Returns the path and source when filesystem inspection fails.
pub fn detect_existing_deployment(target_root: &Path) -> Result<Vec<PathBuf>, PlanError> {
    let mut existing = Vec::new();
    for destination in DEPLOYMENT_FILES {
        let path = rooted(target_root, destination);
        match fs::symlink_metadata(&path) {
            Ok(_) => existing.push(path),
            Err(source) if source.kind() == io::ErrorKind::NotFound => {}
            Err(source) => return Err(source).context(InspectSnafu { path }),
        }
    }
    Ok(existing)
}

/// Builds an in-memory plan beneath an existing root without writing deployment files.
///
/// Callers supply validated materials and configuration; production uses `/` as the root.
///
/// # Errors
/// Requires real `/etc` and `/var/lib/natsume-server` directories supplied by the Server Deb.
/// Rejects missing prerequisites, existing deployment files, and unsuccessful filesystem inspections.
pub fn build_plan(
    target_root: &Path,
    service: Ownership,
    materials: PlanMaterials<'_>,
) -> Result<DeploymentPlan, PlanError> {
    for prerequisite in [config::SYSTEM_CONFIG_DIRECTORY, config::STATE_DIRECTORY] {
        let path = rooted(target_root, prerequisite);
        match fs::symlink_metadata(&path) {
            Ok(metadata) => ensure!(metadata.is_dir(), MissingPrerequisiteSnafu { path }),
            Err(source)
                if matches!(
                    source.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                ) =>
            {
                return MissingPrerequisiteSnafu { path }.fail();
            }
            Err(source) => return Err(source).context(InspectSnafu { path }),
        }
    }
    let paths = detect_existing_deployment(target_root)?;
    ensure!(paths.is_empty(), AlreadyDeployedSnafu { paths });
    let files = vec![
        planned_file(
            target_root,
            config::ORIGIN_CA_DER,
            service,
            0o600,
            materials.origin_ca.der(),
        ),
        planned_file(
            target_root,
            config::ORIGIN_CA_KEY,
            service,
            0o600,
            materials.origin_key.pkcs8_der(),
        ),
        planned_file(
            target_root,
            config::SERVER_TLS_LEAF,
            service,
            0o600,
            materials.tls_leaf.der(),
        ),
        planned_file(
            target_root,
            config::SERVER_TLS_KEY,
            service,
            0o600,
            materials.tls_key.pkcs8_der(),
        ),
        planned_file(
            target_root,
            config::CONTROL_CA_TRUST,
            ROOT,
            0o644,
            materials.control_ca.to_pem().as_bytes(),
        ),
        planned_file(
            target_root,
            config::LOCAL_ORIGIN_TRUST,
            ROOT,
            0o644,
            materials.origin_ca.to_pem().as_bytes(),
        ),
        planned_file(
            target_root,
            config::CONFIG_PATH,
            ROOT,
            0o644,
            materials.config.as_bytes(),
        ),
    ];
    let directories = vec![
        PlannedDirectory {
            path: rooted(target_root, config::CONFIG_DIRECTORY),
            owner: ROOT,
            mode: 0o755,
        },
        PlannedDirectory {
            path: rooted(target_root, config::TRUST_PARENT_DIRECTORY),
            owner: ROOT,
            mode: 0o755,
        },
        PlannedDirectory {
            path: rooted(target_root, config::TRUST_DIRECTORY),
            owner: ROOT,
            mode: 0o755,
        },
        PlannedDirectory {
            path: rooted(target_root, config::KEYS_DIRECTORY),
            owner: service,
            mode: 0o700,
        },
    ];
    Ok(DeploymentPlan { files, directories })
}

fn planned_file(
    target_root: &Path,
    destination: &str,
    owner: Ownership,
    mode: u32,
    contents: &[u8],
) -> PlannedFile {
    PlannedFile {
        spec: FileSpec {
            path: rooted(target_root, destination),
            owner,
            mode,
        },
        contents: Zeroizing::new(contents.to_vec()),
    }
}

fn rooted(target_root: &Path, destination: &str) -> PathBuf {
    target_root.join(destination.trim_start_matches('/'))
}

#[cfg(test)]
pub(super) mod tests {
    use std::{error::Error as _, net::Ipv4Addr, os::unix::fs::symlink};

    use snafu::OptionExt as _;
    use tempfile::TempDir;
    use time::{Duration, OffsetDateTime, format_description::well_known::Rfc3339};

    use super::*;
    use crate::{
        config::{ConfigRenderError, DeploymentParameters},
        pki::{generate, import},
    };

    #[derive(Debug, Snafu)]
    pub(in crate::deployment) enum TestError {
        #[snafu(context(false), display("fixture I/O failed"))]
        Io { source: io::Error },
        #[snafu(context(false), display("fixture PKI generation failed"))]
        Generate { source: generate::GenerateError },
        #[snafu(context(false), display("fixture PKI parsing failed"))]
        Import { source: import::ImportError },
        #[snafu(context(false), display("fixture config failed"))]
        Config { source: ConfigRenderError },
        #[snafu(context(false), display("fixture time formatting failed"))]
        Time { source: time::error::Format },
        #[snafu(context(false), display("fixture plan failed"))]
        Plan { source: PlanError },
        #[snafu(context(false), display("fixture PEM encoding failed"))]
        Pem { source: pkcs8::der::pem::Error },
        #[snafu(context(false), display("fixture apply failed"))]
        Apply {
            source: super::super::apply::ApplyError,
        },
        #[snafu(display("fixture path lacks parent"))]
        MissingParent,
    }

    pub(in crate::deployment) struct Fixture {
        pub root: TempDir,
        pub control: Certificate,
        pub leaf: Certificate,
        pub leaf_key: PrivateKey,
        pub origin: Certificate,
        pub origin_key: PrivateKey,
        pub config: String,
    }

    impl Fixture {
        pub fn new() -> Result<Self, TestError> {
            let root = tempfile::tempdir()?;
            fs::create_dir_all(rooted(root.path(), config::SYSTEM_CONFIG_DIRECTORY))?;
            fs::create_dir_all(rooted(root.path(), config::STATE_DIRECTORY))?;
            let now = OffsetDateTime::now_utc();
            let params = DeploymentParameters::new(
                vec![Ipv4Addr::LOCALHOST.into()],
                8443,
                "gateway.contest.example".to_owned(),
                "https://judge.contest.example".to_owned(),
                (now + Duration::days(1)).format(&Rfc3339)?,
                (now + Duration::days(3)).format(&Rfc3339)?,
                now,
            )?;
            let config = config::render_server_config(&params)?;
            config::validate_server_config(&config)?;
            let (control, control_key) = generate::generate_ca("Control CA", "Natsume", now)?;
            let (origin, origin_key) = generate::generate_ca("Origin CA", "Natsume", now)?;
            let (leaf, leaf_key) = generate::issue_server_leaf(
                &control,
                &control_key,
                params.server_ips(),
                params.gateway_not_after_timestamp(),
                now,
            )?;
            Ok(Self {
                root,
                control,
                leaf,
                leaf_key,
                origin,
                origin_key,
                config,
            })
        }

        pub fn materials(&self) -> PlanMaterials<'_> {
            PlanMaterials {
                control_ca: &self.control,
                tls_leaf: &self.leaf,
                tls_key: &self.leaf_key,
                origin_ca: &self.origin,
                origin_key: &self.origin_key,
                config: &self.config,
            }
        }

        pub fn path(&self, destination: &str) -> PathBuf {
            rooted(self.root.path(), destination)
        }

        pub fn plan(&self) -> Result<DeploymentPlan, TestError> {
            Ok(build_plan(self.root.path(), ownership(), self.materials())?)
        }

        pub fn unprivileged_plan(&self) -> Result<DeploymentPlan, TestError> {
            let mut plan = self.plan()?;
            for file in &mut plan.files {
                file.spec.owner = ownership();
            }
            for directory in &mut plan.directories {
                directory.owner = ownership();
            }
            Ok(plan)
        }

        pub fn write(&self, destination: &str, contents: &[u8]) -> Result<PathBuf, TestError> {
            let path = self.path(destination);
            fs::create_dir_all(path.parent().context(MissingParentSnafu)?)?;
            fs::write(&path, contents)?;
            Ok(path)
        }
    }

    pub(in crate::deployment) fn ownership() -> Ownership {
        Ownership {
            uid: rustix::process::geteuid().as_raw(),
            gid: rustix::process::getegid().as_raw(),
        }
    }

    #[test]
    fn plan_orders_files_and_sets_metadata_without_writing() -> Result<(), TestError> {
        let fixture = Fixture::new()?;
        let plan = fixture.plan()?;
        assert_eq!(plan.files.len(), 7);
        for (file, (destination, owner, mode)) in plan.files.iter().zip([
            (config::ORIGIN_CA_DER, ownership(), 0o600),
            (config::ORIGIN_CA_KEY, ownership(), 0o600),
            (config::SERVER_TLS_LEAF, ownership(), 0o600),
            (config::SERVER_TLS_KEY, ownership(), 0o600),
            (config::CONTROL_CA_TRUST, ROOT, 0o644),
            (config::LOCAL_ORIGIN_TRUST, ROOT, 0o644),
            (config::CONFIG_PATH, ROOT, 0o644),
        ]) {
            assert_eq!(file.spec.path, fixture.path(destination));
            assert_eq!(file.spec.mode, mode);
            assert_eq!(file.spec.owner, owner);
        }
        for directory in &plan.directories {
            let private = directory.path == fixture.path(config::KEYS_DIRECTORY);
            assert_eq!(directory.mode, if private { 0o700 } else { 0o755 });
            assert_eq!(directory.owner, if private { ownership() } else { ROOT });
        }
        assert_eq!(
            plan.directories
                .iter()
                .map(|directory| directory.path.clone())
                .collect::<Vec<_>>(),
            [
                config::CONFIG_DIRECTORY,
                config::TRUST_PARENT_DIRECTORY,
                config::TRUST_DIRECTORY,
                config::KEYS_DIRECTORY,
            ]
            .iter()
            .map(|path| fixture.path(path))
            .collect::<Vec<_>>()
        );
        assert!(
            plan.directories
                .iter()
                .all(|directory| !directory.path.exists())
        );
        assert_eq!(fs::read_dir(fixture.root.path())?.count(), 2);
        assert!(detect_existing_deployment(fixture.root.path())?.is_empty());
        Ok(())
    }

    #[test]
    fn missing_or_invalid_prerequisites_reject_plan() -> Result<(), TestError> {
        for prerequisite in [config::SYSTEM_CONFIG_DIRECTORY, config::STATE_DIRECTORY] {
            let fixture = Fixture::new()?;
            let path = fixture.path(prerequisite);
            fs::remove_dir(&path)?;
            assert!(matches!(fixture.plan(), Err(TestError::Plan {
                source: PlanError::MissingPrerequisite { path: missing },
            }) if missing == path));
            fs::write(&path, b"not a directory")?;
            assert!(matches!(fixture.plan(), Err(TestError::Plan {
                source: PlanError::MissingPrerequisite { path: missing },
            }) if missing == path));
            fs::remove_file(&path)?;
            symlink(fixture.root.path(), &path)?;
            assert!(matches!(fixture.plan(), Err(TestError::Plan {
                source: PlanError::MissingPrerequisite { path: missing },
            }) if missing == path));
        }
        Ok(())
    }

    #[test]
    fn serialized_materials_round_trip_and_debug_excludes_contents() -> Result<(), TestError> {
        let fixture = Fixture::new()?;
        let plan = fixture.plan()?;
        for file in &plan.files {
            match file.spec.path.as_path() {
                path if path == fixture.path(config::ORIGIN_CA_DER)
                    || path == fixture.path(config::LOCAL_ORIGIN_TRUST) =>
                {
                    assert_eq!(import::import_certificate(&file.contents)?, fixture.origin);
                }
                path if path == fixture.path(config::SERVER_TLS_LEAF) => {
                    assert_eq!(import::import_certificate(&file.contents)?, fixture.leaf);
                }
                path if path == fixture.path(config::CONTROL_CA_TRUST) => {
                    assert_eq!(import::import_certificate(&file.contents)?, fixture.control);
                }
                path if path == fixture.path(config::ORIGIN_CA_KEY) => {
                    let key = import::import_private_key(&file.contents, None)?;
                    assert_eq!(key.pkcs8_der(), fixture.origin_key.pkcs8_der());
                    assert!(key.matches(&fixture.origin));
                }
                path if path == fixture.path(config::SERVER_TLS_KEY) => {
                    let key = import::import_private_key(&file.contents, None)?;
                    assert_eq!(key.pkcs8_der(), fixture.leaf_key.pkcs8_der());
                    assert!(key.matches(&fixture.leaf));
                }
                path if path == fixture.path(config::CONFIG_PATH) => {
                    assert_eq!(&*file.contents, fixture.config.as_bytes());
                }
                path => panic!("unexpected deployment file: {path:?}"),
            }
        }
        let debug = format!("{plan:?} {:?}", fixture.materials());
        assert!(!debug.contains(&fixture.config));
        for file in &plan.files {
            assert!(!debug.contains(&format!("{:?}", *file.contents)));
        }
        for key in [&fixture.origin_key, &fixture.leaf_key] {
            assert!(!debug.contains(&hex::encode(key.pkcs8_der())));
            let pem = pkcs8::der::pem::encode_string(
                "PRIVATE KEY",
                pkcs8::LineEnding::LF,
                key.pkcs8_der(),
            )?;
            for line in pem.lines().filter(|line| !line.starts_with("-----")) {
                assert!(!debug.contains(line));
            }
        }
        Ok(())
    }

    #[test]
    fn each_existing_file_or_symlink_rejects_first_install() -> Result<(), TestError> {
        let fixture = Fixture::new()?;
        for destination in DEPLOYMENT_FILES {
            let path = fixture.path(destination);
            fs::create_dir_all(path.parent().context(MissingParentSnafu)?)?;
            for link in [false, true] {
                if link {
                    symlink(fixture.root.path().join("absent"), &path)?;
                } else {
                    fs::write(&path, b"preexisting")?;
                }
                assert_eq!(
                    detect_existing_deployment(fixture.root.path())?,
                    vec![path.clone()]
                );
                let Err(error) = fixture.plan() else {
                    panic!("existing deployment path accepted: {path:?}");
                };
                assert!(matches!(error, TestError::Plan {
                    source: PlanError::AlreadyDeployed { paths },
                } if paths == vec![path.clone()]));
                fs::remove_file(&path)?;
            }
        }
        Ok(())
    }

    #[test]
    fn detects_all_existing_paths_but_ignores_database_and_root_key() -> Result<(), TestError> {
        let fixture = Fixture::new()?;
        fixture.write(config::DATABASE, b"database")?;
        fixture.write(config::ROOT_KEY, b"vault key")?;
        assert!(detect_existing_deployment(fixture.root.path())?.is_empty());
        fixture.plan()?;
        for destination in DEPLOYMENT_FILES {
            fixture.write(destination, b"preexisting")?;
        }
        let expected: Vec<_> = DEPLOYMENT_FILES.map(|path| fixture.path(path)).into();
        assert_eq!(detect_existing_deployment(fixture.root.path())?, expected);
        assert!(matches!(fixture.plan(), Err(TestError::Plan {
            source: PlanError::AlreadyDeployed { paths },
        }) if paths == expected));
        Ok(())
    }

    #[test]
    fn inspection_error_preserves_source() -> Result<(), TestError> {
        let fixture = Fixture::new()?;
        fixture.write(config::KEYS_DIRECTORY, b"not a directory")?;
        let Err(error) = detect_existing_deployment(fixture.root.path()) else {
            panic!("non-directory parent must fail inspection");
        };
        assert!(
            error
                .source()
                .is_some_and(<dyn std::error::Error>::is::<io::Error>)
        );
        assert!(!error.to_string().contains("not a directory"));
        Ok(())
    }
}
