//! Deployment application and rollback for first-install setup.

use std::path::PathBuf;

use snafu::{IntoError as _, ResultExt as _, Snafu};

use super::{
    file::{self, CreatedFile, DirectoryRemoval, FileError, RollbackDirectory},
    plan::DeploymentPlan,
};

/// Creation records retained for rollback after subsequent installation steps fail.
#[derive(Debug)]
pub struct ApplyReport {
    created_files: Vec<CreatedFile>,
    created_directories: Vec<PathBuf>,
    rollback_directory: RollbackDirectory,
}

/// Removed paths, expected retained directories, and entries that rollback could not safely delete.
#[derive(Debug, Default)]
pub struct RollbackOutcome {
    /// Paths successfully removed in rollback order.
    pub removed: Vec<PathBuf>,
    /// Directories created by this attempt but populated by subsequent steps, such as
    /// bootstrap's `server-root.key`, and therefore preserved as expected.
    pub kept: Vec<PathBuf>,
    /// Entries that could not safely be removed because of a change or filesystem error.
    pub failures: Vec<RollbackFailure>,
}

/// A rollback rejection retaining the affected path and original cause.
#[derive(Debug, Snafu)]
#[snafu(display("cannot remove deployment entry: {}", path.display()))]
pub struct RollbackFailure {
    /// Entry that could not safely be removed.
    pub path: PathBuf,
    /// Original removal failure retaining its underlying cause.
    pub source: FileError,
}

/// Failed application step, original cause, and results of reversing prior creations.
#[derive(Debug, Snafu)]
#[non_exhaustive]
pub enum ApplyError {
    /// Rollback storage could not be prepared before deployment file publication.
    #[snafu(display("cannot prepare rollback storage: {}", path.display()))]
    PrepareRollback {
        /// Rejected quarantine path.
        path: PathBuf,
        /// Original preparation or filesystem compatibility failure.
        source: FileError,
        /// No deployment files have been written.
        rollback: Box<RollbackOutcome>,
        /// Creation records retained after the completed rollback for the owning wizard.
        report: Option<Box<ApplyReport>>,
    },
    /// A planned directory could not be created or safely reused.
    #[snafu(display("cannot ensure deployment directory: {}", path.display()))]
    EnsureDirectory {
        /// Destination of the failed operation.
        path: PathBuf,
        /// Original filesystem failure.
        source: FileError,
        /// Results of reversing entries created before this failure.
        rollback: Box<RollbackOutcome>,
        /// Creation records retained after the completed rollback for the owning wizard.
        report: Option<Box<ApplyReport>>,
    },
    /// A planned file could not be published.
    #[snafu(display("cannot create deployment file: {}", path.display()))]
    CreateFile {
        /// Destination of the failed operation.
        path: PathBuf,
        /// Original filesystem failure.
        source: FileError,
        /// Results of reversing entries created before this failure.
        rollback: Box<RollbackOutcome>,
        /// Creation records retained after the completed rollback for the owning wizard.
        report: Option<Box<ApplyReport>>,
    },
}

impl ApplyError {
    /// Transfers creation records without repeating the rollback already recorded by this error.
    pub(crate) fn take_report(&mut self) -> Option<ApplyReport> {
        let report = match self {
            Self::PrepareRollback { report, .. }
            | Self::EnsureDirectory { report, .. }
            | Self::CreateFile { report, .. } => report,
        };
        report.take().map(|report| *report)
    }
}

/// Ensures planned directories and publishes keys, trust anchors, then configuration.
///
/// # Errors
/// Returns the failing step and rollback outcome when a filesystem operation fails.
pub fn apply_plan(plan: &DeploymentPlan) -> Result<ApplyReport, ApplyError> {
    let keys = &plan.directories[3].path;
    let quarantine =
        RollbackDirectory::open(&plan.rollback_directory.path, plan.rollback_directory.owner)
            .and_then(|quarantine| {
                quarantine.check_filesystem(keys)?;
                Ok(quarantine)
            })
            .context(PrepareRollbackSnafu {
                path: &plan.rollback_directory.path,
                rollback: Box::new(RollbackOutcome::default()),
                report: None,
            })?;
    let mut report = ApplyReport {
        created_files: Vec::new(),
        created_directories: Vec::new(),
        rollback_directory: quarantine,
    };
    for directory in plan.prerequisites.iter().chain(&plan.directories) {
        match file::ensure_directory(&directory.path, directory.owner, directory.mode) {
            Ok(created) => report.created_directories.extend(created),
            Err(source) => {
                tracing::error!(path = ?directory.path, "cannot ensure deployment directory");
                return Err(source).context(EnsureDirectorySnafu {
                    path: &directory.path,
                    rollback: Box::new(rollback(&report)),
                    report: Some(Box::new(report)),
                });
            }
        }
    }
    if let Err(source) = report.rollback_directory.check_filesystem(keys) {
        return Err(source).context(PrepareRollbackSnafu {
            path: &plan.rollback_directory.path,
            rollback: Box::new(rollback(&report)),
            report: Some(Box::new(report)),
        });
    }
    for planned in &plan.files {
        match file::create_recorded_file(
            &planned.spec,
            &planned.contents,
            &mut report.created_files,
        ) {
            Ok(()) => {}
            Err(source) => {
                tracing::error!(path = ?planned.spec.path, "cannot create deployment file");
                return Err(source).context(CreateFileSnafu {
                    path: &planned.spec.path,
                    rollback: Box::new(rollback(&report)),
                    report: Some(Box::new(report)),
                });
            }
        }
    }
    Ok(report)
}

/// Reverses this attempt's creations, preserving modified files and nonempty directories.
#[must_use]
pub fn rollback(report: &ApplyReport) -> RollbackOutcome {
    let mut outcome = RollbackOutcome::default();
    {
        let quarantine = &report.rollback_directory;
        for created in report.created_files.iter().rev() {
            match quarantine.remove_deployment_file(created) {
                Ok(()) => outcome.removed.push(created.path.clone()),
                Err(source) => {
                    tracing::warn!(path = ?created.path, "cannot roll back deployment file");
                    outcome.failures.push(
                        RollbackFailureSnafu {
                            path: &created.path,
                        }
                        .into_error(source),
                    );
                }
            }
        }
    }
    report.rollback_directory.cleanup();
    for path in report.created_directories.iter().rev() {
        match file::remove_created_directory(path) {
            Ok(DirectoryRemoval::Removed) => outcome.removed.push(path.clone()),
            Ok(DirectoryRemoval::KeptNotEmpty) => outcome.kept.push(path.clone()),
            Err(source) => {
                tracing::warn!(?path, "cannot roll back deployment directory");
                outcome
                    .failures
                    .push(RollbackFailureSnafu { path }.into_error(source));
            }
        }
    }
    outcome
}

#[cfg(test)]
mod tests {
    use std::{
        error::Error as _,
        fs,
        os::unix::fs::{MetadataExt as _, PermissionsExt as _, symlink},
    };

    use super::*;
    use crate::{
        config,
        deployment::plan::{
            detect_existing_deployment,
            tests::{Fixture, TestError, ownership},
        },
        pki::import,
    };

    #[test]
    fn quarantine_device_mismatch_rejects_before_deployment_writes() -> Result<(), TestError> {
        let fixture = Fixture::new()?;
        let plan = fixture.plan()?;
        file::tests::mismatch_rollback_device(fs::metadata(fixture.root.path())?.dev());
        assert!(matches!(
            apply_plan(&plan),
            Err(ApplyError::PrepareRollback {
                source: FileError::RollbackFilesystem { .. },
                ..
            })
        ));
        assert!(detect_existing_deployment(fixture.root.path())?.is_empty());
        assert!(!plan.directories[0].path.exists());
        assert!(!plan.rollback_directory.path.exists());
        Ok(())
    }

    #[test]
    fn quarantine_creation_failure_rejects_before_deployment_writes() -> Result<(), TestError> {
        let fixture = Fixture::new()?;
        let plan = fixture.plan()?;
        fs::write(&plan.rollback_directory.path, b"occupied")?;
        assert!(matches!(
            apply_plan(&plan),
            Err(ApplyError::PrepareRollback { .. })
        ));
        assert!(detect_existing_deployment(fixture.root.path())?.is_empty());
        assert!(!plan.directories[0].path.exists());
        Ok(())
    }

    #[test]
    fn protected_etc_on_separate_device_rolls_back_without_quarantine() -> Result<(), TestError> {
        let fixture = Fixture::new()?;
        let _device = file::tests::separate_device(fixture.path("/etc"));
        let report = apply_plan(&fixture.plan()?)?;
        assert_ne!(
            report.created_files[0].device,
            report.created_files[6].device
        );
        let outcome = rollback(&report);
        assert!(outcome.failures.is_empty(), "{outcome:?}");
        assert!(detect_existing_deployment(fixture.root.path())?.is_empty());
        Ok(())
    }

    #[test]
    fn report_drop_preserves_quarantine_name_reused_after_rollback() -> Result<(), TestError> {
        let fixture = Fixture::new()?;
        let plan = fixture.plan()?;
        let report = apply_plan(&plan)?;
        let outcome = rollback(&report);
        assert!(outcome.failures.is_empty(), "{outcome:?}");
        fs::create_dir(&plan.rollback_directory.path)?;
        drop(report);
        assert!(plan.rollback_directory.path.is_dir());
        Ok(())
    }

    #[test]
    fn applies_complete_materials_and_metadata_then_rolls_back_in_reverse() -> Result<(), TestError>
    {
        let fixture = Fixture::new()?;
        let plan = fixture.plan()?;
        let report = apply_plan(&plan)?;
        assert_eq!(report.created_files.len(), 7);
        assert_eq!(detect_existing_deployment(fixture.root.path())?.len(), 7);
        for planned in &plan.files {
            let contents = fs::read(&planned.spec.path)?;
            assert_eq!(contents, *planned.contents);
            let metadata = fs::metadata(&planned.spec.path)?;
            assert_eq!(metadata.mode() & 0o7777, planned.spec.mode);
            assert_eq!(metadata.uid(), ownership().uid);
            assert_eq!(metadata.gid(), ownership().gid);
        }
        for directory in &plan.directories {
            let metadata = fs::metadata(&directory.path)?;
            assert_eq!(metadata.mode() & 0o7777, directory.mode);
            assert_eq!(metadata.uid(), ownership().uid);
            assert_eq!(metadata.gid(), ownership().gid);
        }
        for (path, expected) in [
            (config::CONTROL_CA_TRUST, &fixture.control),
            (config::LOCAL_ORIGIN_TRUST, &fixture.origin),
            (config::ORIGIN_CA_DER, &fixture.origin),
            (config::SERVER_TLS_LEAF, &fixture.leaf),
        ] {
            assert_eq!(
                import::import_certificate(&fs::read(fixture.path(path))?)?,
                *expected
            );
        }
        for (path, expected, certificate) in [
            (config::ORIGIN_CA_KEY, &fixture.origin_key, &fixture.origin),
            (config::SERVER_TLS_KEY, &fixture.leaf_key, &fixture.leaf),
        ] {
            let key = import::import_private_key(&fs::read(fixture.path(path))?, None)?;
            assert_eq!(key.pkcs8_der(), expected.pkcs8_der());
            assert!(key.matches(certificate));
        }
        let outcome = rollback(&report);
        assert!(outcome.failures.is_empty(), "{outcome:?}");
        let expected: Vec<_> = report
            .created_files
            .iter()
            .rev()
            .map(|file| file.path.clone())
            .chain(report.created_directories.iter().rev().cloned())
            .collect();
        assert_eq!(outcome.removed, expected);
        assert_eq!(fs::read_dir(fixture.root.path())?.count(), 2);
        Ok(())
    }

    #[test]
    fn published_sync_failure_rolls_back_entire_plan() -> Result<(), TestError> {
        for count in 0..7 {
            let fixture = Fixture::new()?;
            let plan = fixture.plan()?;
            let _fault = file::tests::fail_publication_sync_after(count);
            let Err(error) = apply_plan(&plan) else {
                panic!("directory fsync fault must reject apply")
            };
            let ApplyError::CreateFile {
                source, rollback, ..
            } = error
            else {
                panic!("expected publication failure");
            };
            assert!(matches!(source, FileError::PublishedNotDurable { .. }));
            assert!(rollback.failures.is_empty(), "{rollback:?}");
            assert!(detect_existing_deployment(fixture.root.path())?.is_empty());
            assert_eq!(fs::read_dir(fixture.root.path())?.count(), 2);
        }
        Ok(())
    }

    #[test]
    fn every_file_conflict_rolls_back_only_this_attempt() -> Result<(), TestError> {
        let fixture = Fixture::new()?;
        for index in 0..7 {
            let database = fixture.write(config::DATABASE, b"database unchanged")?;
            let root_key = fixture.write(config::ROOT_KEY, b"root key unchanged")?;
            let unrelated =
                fixture.write("/var/lib/natsume-server/keys/unrelated", b"unrelated")?;
            let plan = fixture.plan()?;
            let conflict = &plan.files[index].spec.path;
            fs::create_dir_all(conflict.parent().ok_or(TestError::MissingParent)?)?;
            fs::write(conflict, b"conflict unchanged")?;
            let Err(error) = apply_plan(&plan) else {
                panic!("conflict at {conflict:?} must reject apply");
            };
            assert!(
                error
                    .source()
                    .is_some_and(<dyn std::error::Error>::is::<FileError>)
            );
            let ApplyError::CreateFile {
                path,
                source,
                rollback,
                ..
            } = error
            else {
                panic!("expected file conflict, got {error:?}");
            };
            assert_eq!(path, *conflict);
            assert!(matches!(source, FileError::AlreadyExists { .. }));
            assert!(rollback.failures.is_empty(), "{rollback:?}");
            let removed_files: Vec<_> = plan.files[..index]
                .iter()
                .rev()
                .map(|file| file.spec.path.clone())
                .collect();
            assert_eq!(&rollback.removed[..index], removed_files);
            assert_eq!(
                detect_existing_deployment(fixture.root.path())?,
                vec![conflict.clone()]
            );
            assert_eq!(fs::read(conflict)?, b"conflict unchanged");
            assert_eq!(fs::read(&database)?, b"database unchanged");
            assert_eq!(fs::read(&root_key)?, b"root key unchanged");
            assert_eq!(fs::read(&unrelated)?, b"unrelated");
            assert!(!error_display_contains_contents(&source));
            fs::remove_file(conflict)?;
        }
        Ok(())
    }

    #[test]
    fn directory_conflict_rolls_back_created_parents() -> Result<(), TestError> {
        let fixture = Fixture::new()?;
        let plan = fixture.plan()?;
        fixture.write("/etc/natsume", b"directory conflict")?;
        let Err(error) = apply_plan(&plan) else {
            panic!("non-directory trust parent must reject apply");
        };
        let ApplyError::EnsureDirectory {
            path,
            source,
            rollback,
            ..
        } = error
        else {
            panic!("expected directory failure, got {error:?}");
        };
        assert_eq!(path, fixture.path("/etc/natsume"));
        assert!(matches!(source, FileError::NotADirectory { .. }));
        assert!(rollback.failures.is_empty(), "{rollback:?}");
        assert!(fixture.path(config::STATE_DIRECTORY).is_dir());
        assert!(!fixture.path(config::KEYS_DIRECTORY).exists());
        assert_eq!(
            fs::read(fixture.path("/etc/natsume"))?,
            b"directory conflict"
        );
        assert!(detect_existing_deployment(fixture.root.path()).is_err());
        Ok(())
    }

    #[test]
    fn existing_keys_directory_and_bootstrap_state_survive_rollback_and_retry()
    -> Result<(), TestError> {
        let fixture = Fixture::new()?;
        let database = fixture.write(config::DATABASE, b"initialized database")?;
        let root_key = fixture.write(config::ROOT_KEY, b"bootstrap key")?;
        let keys = fixture.path(config::KEYS_DIRECTORY);
        fs::set_permissions(&keys, fs::Permissions::from_mode(0o700))?;
        let before = fs::metadata(&keys)?;
        for _ in 0..2 {
            let report = apply_plan(&fixture.plan()?)?;
            assert!(!report.created_directories.contains(&keys));
            let outcome = rollback(&report);
            assert!(outcome.failures.is_empty(), "{outcome:?}");
            assert!(detect_existing_deployment(fixture.root.path())?.is_empty());
            let after = fs::metadata(&keys)?;
            assert_eq!(after.ino(), before.ino());
            assert_eq!(after.mode(), before.mode());
            assert_eq!(after.uid(), before.uid());
            assert_eq!(after.gid(), before.gid());
            assert_eq!(fs::read(&database)?, b"initialized database");
            assert_eq!(fs::read(&root_key)?, b"bootstrap key");
        }
        Ok(())
    }

    #[test]
    fn rollback_reports_modified_file_and_keeps_nonempty_directories() -> Result<(), TestError> {
        let fixture = Fixture::new()?;
        let report = apply_plan(&fixture.plan()?)?;
        let changed = fixture.path(config::SERVER_TLS_KEY);
        fs::write(&changed, b"external change")?;
        let outcome = rollback(&report);
        assert_eq!(fs::read(&changed)?, b"external change");
        assert!(
            outcome
                .failures
                .iter()
                .any(|failure| failure.path == changed
                    && matches!(failure.source, FileError::ChangedSinceCreation { .. }))
        );
        let expected_failures = [changed.clone()];
        assert_eq!(outcome.kept, [fixture.path(config::KEYS_DIRECTORY)]);
        assert_eq!(
            outcome
                .failures
                .iter()
                .map(|failure| &failure.path)
                .collect::<Vec<_>>(),
            expected_failures.iter().collect::<Vec<_>>()
        );
        assert_eq!(outcome.removed.len(), 9);
        assert!(
            outcome
                .failures
                .iter()
                .all(|failure| failure.source().is_some())
        );
        assert_eq!(
            detect_existing_deployment(fixture.root.path())?,
            vec![changed]
        );
        Ok(())
    }

    #[test]
    fn rollback_keeps_replaced_file_and_symlink() -> Result<(), TestError> {
        for link in [false, true] {
            let fixture = Fixture::new()?;
            let report = apply_plan(&fixture.plan()?)?;
            let changed = fixture.path(config::CONFIG_PATH);
            let contents = fs::read(&changed)?;
            let original = fs::File::open(&changed)?;
            fs::remove_file(&changed)?;
            if link {
                symlink(fixture.root.path().join("absent"), &changed)?;
            } else {
                fs::write(&changed, &contents)?;
            }
            let outcome = rollback(&report);
            assert!(
                outcome
                    .failures
                    .iter()
                    .any(|failure| failure.path == changed
                        && matches!(failure.source, FileError::ChangedSinceCreation { .. }))
            );
            if link {
                assert!(fs::symlink_metadata(&changed)?.is_symlink());
            } else {
                assert_eq!(fs::read(&changed)?, contents);
                assert_ne!(fs::metadata(&changed)?.ino(), original.metadata()?.ino());
            }
        }
        Ok(())
    }

    #[test]
    fn rollback_keeps_bootstrap_files_created_after_apply_without_failures() -> Result<(), TestError>
    {
        let fixture = Fixture::new()?;
        let report = apply_plan(&fixture.plan()?)?;
        assert!(
            report
                .created_directories
                .contains(&fixture.path(config::KEYS_DIRECTORY))
        );
        let unrelated = fixture.write("/var/lib/natsume-server/keys/unrelated", b"later file")?;
        let database = fixture.write(config::DATABASE, b"new database")?;
        let root_key = fixture.write(config::ROOT_KEY, b"new root key")?;
        let outcome = rollback(&report);
        assert_eq!(fs::read(database)?, b"new database");
        assert_eq!(fs::read(unrelated)?, b"later file");
        assert_eq!(outcome.kept, [fixture.path(config::KEYS_DIRECTORY)]);
        assert_eq!(fs::read(root_key)?, b"new root key");
        assert!(detect_existing_deployment(fixture.root.path())?.is_empty());
        assert!(outcome.failures.is_empty(), "{outcome:?}");
        Ok(())
    }

    #[test]
    fn apply_rejects_symlink_directory_without_writing_through_it() -> Result<(), TestError> {
        let fixture = Fixture::new()?;
        let outside = tempfile::tempdir()?;
        fs::create_dir_all(fixture.path("/var/lib/natsume-server"))?;
        let keys = fixture.path(config::KEYS_DIRECTORY);
        symlink(outside.path(), &keys)?;
        let plan = {
            fs::remove_file(&keys)?;
            let plan = fixture.plan()?;
            symlink(outside.path(), &keys)?;
            plan
        };
        let Err(error) = apply_plan(&plan) else {
            panic!("symlink keys directory must reject apply");
        };
        assert!(matches!(
            error,
            ApplyError::PrepareRollback {
                source: FileError::SymlinkRejected { .. },
                ..
            }
        ));
        assert!(fs::symlink_metadata(keys)?.is_symlink());
        assert_eq!(fs::read_dir(outside.path())?.count(), 0);
        Ok(())
    }

    #[test]
    fn directory_metadata_change_after_planning_is_rejected_and_rolls_back() -> Result<(), TestError>
    {
        let fixture = Fixture::new()?;
        let plan = fixture.plan()?;
        let trust = fixture.path("/etc/natsume/trust");
        fs::create_dir_all(&trust)?;
        fs::set_permissions(&trust, fs::Permissions::from_mode(0o555))?;
        let result = apply_plan(&plan);
        fs::set_permissions(&trust, fs::Permissions::from_mode(0o755))?;
        let Err(error) = result else {
            panic!("read-only trust directory must reject apply");
        };
        assert!(error.source().is_some());
        let ApplyError::EnsureDirectory {
            path,
            source,
            rollback,
            ..
        } = error
        else {
            panic!("expected directory metadata failure, got {error:?}");
        };
        assert_eq!(path, trust);
        assert!(matches!(source, FileError::DirectoryMetadata { .. }));
        assert!(rollback.failures.is_empty(), "{rollback:?}");
        assert!(detect_existing_deployment(fixture.root.path())?.is_empty());
        assert!(fixture.path(config::STATE_DIRECTORY).is_dir());
        assert!(!fixture.path(config::KEYS_DIRECTORY).exists());
        assert_eq!(fs::read_dir(trust)?.count(), 0);
        Ok(())
    }

    fn error_display_contains_contents(error: &FileError) -> bool {
        [
            "conflict unchanged",
            "database unchanged",
            "root key unchanged",
            "unrelated",
        ]
        .iter()
        .any(|contents| error.to_string().contains(contents))
    }
}
