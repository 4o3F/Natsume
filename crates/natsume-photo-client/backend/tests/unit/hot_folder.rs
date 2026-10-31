use super::super::*;
use super::*;
use crate::{capture_session::Error, hot_folder::archive};
use snafu::Snafu;
use std::time::Instant;
type Inner = CaptureState;
use crate::roster::Roster;

#[derive(Debug, Snafu)]
enum TestError {
    Io { source: std::io::Error },
    Roster { source: crate::roster::Error },
    Capture { source: capture::Error },
    HotFolder { source: Error },
    Archive { source: archive::ArchiveError },
    Workspace { source: crate::workspace::Error },
    Fixture,
}
type TestResult<T = ()> = std::result::Result<T, TestError>;

fn fixture() -> TestResult<(tempfile::TempDir, Inner)> {
    let directory = tempfile::tempdir().context(IoSnafu)?;
    let hot_folder = directory.path().join("hot");
    let archive = directory.path().join("archive");
    fs::create_dir(&hot_folder).context(IoSnafu)?;
    fs::create_dir(&archive).context(IoSnafu)?;
    let roster = Roster::parse(include_bytes!("../fixtures/roster.json")).context(RosterSnafu)?;
    let workspace = Workspace::create_with_logos(
        directory.path().join("store-fixture"),
        roster.teams().to_vec(),
        directory.path(),
        Vec::new(),
    )
    .context(WorkspaceSnafu)?;
    Ok((
        directory,
        Inner {
            session: Session::new(roster),
            ingestion: Ingestion {
                hot_folder: fs::canonicalize(hot_folder).context(IoSnafu)?,
                archive: fs::canonicalize(archive).context(IoSnafu)?,
                pending: BTreeMap::new(),
                seen: BTreeMap::new(),
                checked: BTreeMap::new(),
                last_audit: Instant::now(),
                lost: BTreeSet::new(),
                photos: Vec::new(),
                missing_sources: Vec::new(),
                state: IngestionState::Running,
                uncertain: false,
            },
            tasks: BTreeMap::new(),
            photo_checks: BTreeMap::new(),
            validation_tasks: BTreeMap::new(),
            manual_selection: None,
            presentation: Presentation::new(),
            finals: workspace.final_store(),
            confirmed: None,
            cleanup: crate::capture_session::Cleanup::default(),
        },
    ))
}

fn start(inner: &mut Inner, index: usize) -> TestResult<GroupId> {
    let account_id = if index == 0 {
        "0199a000-0000-7000-8000-000000000001"
    } else {
        "0199a000-0000-7000-8000-000000000002"
    };
    inner
        .session
        .select_team(account_id)
        .context(CaptureSnafu)?;
    inner.session.start_group().context(CaptureSnafu)
}

fn settle(inner: &mut Inner) -> TestResult {
    inner.scan().context(HotFolderSnafu)?;
    // Advance only the fixture's observation clock, never sleep or add test fields.
    for pending in inner.ingestion.pending.values_mut() {
        pending.stable_since = Instant::now().checked_sub(INTERVAL).context(FixtureSnafu)?;
    }
    inner.scan().context(HotFolderSnafu)
}

#[test]
fn validation_is_bounded_cached_and_missing_archives_release_group_tasks() -> TestResult {
    let (_directory, mut inner) = fixture()?;
    let group = start(&mut inner, 0)?;
    fs::write(
        inner.ingestion.hot_folder.join("one.jpg"),
        b"first invalid image",
    )
    .context(IoSnafu)?;
    fs::write(
        inner.ingestion.hot_folder.join("two.jpg"),
        b"second invalid image",
    )
    .context(IoSnafu)?;
    settle(&mut inner)?;
    assert_eq!(inner.ingestion.photos.len(), 2);
    assert_eq!(
        inner.photo_checks.len(),
        1,
        "at most one full decode per worker turn"
    );
    assert!(matches!(
        inner.session.abandon_group(group),
        Err(capture::Error::PendingTasks { count: 1 })
    ));
    let first = inner
        .photo_checks
        .values()
        .next()
        .context(FixtureSnafu)?
        .clone();
    assert!(
        matches!(
            inner.apply(Action::SelectCandidate(group, first.photo_id()), true),
            Err(Error::CandidateUnavailable)
        ),
        "rejected content is never selectable"
    );
    let missing = inner
        .ingestion
        .photos
        .iter()
        .find(|photo| !inner.photo_checks.contains_key(&photo.id))
        .context(FixtureSnafu)?
        .clone();
    assert!(
        matches!(
            inner.apply(Action::SelectCandidate(group, missing.id), true),
            Err(Error::CandidateUnavailable)
        ),
        "an archived but unprocessed photo is not selectable"
    );
    fs::remove_file(&missing.path).context(IoSnafu)?;
    inner.reconcile();
    assert_eq!(inner.photo_checks.len(), 2);
    let missing_check = inner.photo_checks.get(&missing.id).context(FixtureSnafu)?;
    assert!(matches!(
        missing_check.result(),
        Err(PhotoValidationError::Io { .. })
    ));
    assert!(!missing_check.retryable());
    assert_eq!(
        inner.session.group().context(FixtureSnafu)?.pending_tasks(),
        0
    );
    inner.reconcile();
    let cached = inner
        .photo_checks
        .get(&first.photo_id())
        .context(FixtureSnafu)?;
    assert!(
        std::ptr::eq(
            first.result().err().context(FixtureSnafu)?,
            cached.result().err().context(FixtureSnafu)?
        ),
        "unchanged rejected content must not be decoded every reconciliation"
    );
    inner.session.abandon_group(group).context(CaptureSnafu)?;
    Ok(())
}

#[test]
fn deleting_a_pending_source_releases_its_task_without_losing_archives() -> TestResult {
    let (_directory, mut inner) = fixture()?;
    let group = start(&mut inner, 0)?;
    fs::write(
        inner.ingestion.hot_folder.join("archived.jpg"),
        b"keep this archive",
    )
    .context(IoSnafu)?;
    settle(&mut inner)?;
    let archived = inner.ingestion.photos[0].clone();
    let pending = inner.ingestion.hot_folder.join("unfinished.jpg");
    fs::write(&pending, b"not archived").context(IoSnafu)?;
    inner.scan().context(HotFolderSnafu)?;
    assert_eq!(
        inner.session.group().context(FixtureSnafu)?.pending_tasks(),
        1
    );
    fs::remove_file(&pending).context(IoSnafu)?;
    for _ in 0..2 {
        inner.reconcile();
    }
    assert!(
        !inner.session.is_paused(),
        "a deleted pending source must not permanently pause the session"
    );
    assert_eq!(inner.ingestion.state, IngestionState::Running);
    assert!(inner.ingestion.pending.is_empty());
    let snapshot = inner.snapshot(0);
    assert_eq!(
        snapshot.missing_sources.len(),
        1,
        "repeated scans must not duplicate the loss"
    );
    assert_eq!(snapshot.missing_sources[0].source, pending);
    assert_eq!(snapshot.missing_sources[0].group_id, Some(group));
    assert_eq!(
        inner.session.group().context(FixtureSnafu)?.pending_tasks(),
        0
    );
    assert_eq!(
        inner.ingestion.photos.len(),
        1,
        "a vanished source must not become an archived photo"
    );
    inner
        .apply(Action::Abandon(group), true)
        .context(HotFolderSnafu)?;
    let next = start(&mut inner, 1)?;
    assert_ne!(next, group);
    assert_eq!(inner.ingestion.photos[0].group_id, Some(group));
    assert_eq!(
        fs::read(archived.path).context(IoSnafu)?,
        b"keep this archive"
    );
    Ok(())
}

#[test]
fn renaming_a_pending_source_releases_the_old_task_and_requires_review() -> TestResult {
    let (_directory, mut inner) = fixture()?;
    start(&mut inner, 0)?;
    let original = inner.ingestion.hot_folder.join("temporary.jpg");
    let renamed = inner.ingestion.hot_folder.join("final.jpg");
    fs::write(&original, b"camera renamed this source").context(IoSnafu)?;
    inner.scan().context(HotFolderSnafu)?;
    fs::rename(&original, &renamed).context(IoSnafu)?;
    inner.reconcile();
    assert!(
        !inner.ingestion.pending.contains_key(&original),
        "a vanished path must be retired rather than block all later reconciliation"
    );
    assert!(
        inner
            .ingestion
            .pending
            .get(&renamed)
            .context(FixtureSnafu)?
            .owner
            .is_none(),
        "a new path observed during source loss must require photographer review"
    );
    settle(&mut inner)?;
    inner.reconcile();
    assert!(!inner.session.is_paused());
    assert_eq!(
        inner.session.group().context(FixtureSnafu)?.pending_tasks(),
        0
    );
    assert_eq!(inner.ingestion.photos.len(), 1);
    assert!(inner.ingestion.photos[0].group_id.is_none());
    assert_eq!(
        fs::read(renamed).context(IoSnafu)?,
        b"camera renamed this source"
    );
    Ok(())
}

#[test]
fn reappearing_lost_source_requires_review_even_after_starting_a_group() -> TestResult {
    let (_directory, mut inner) = fixture()?;
    let source = inner.ingestion.hot_folder.join("unowned.png");
    fs::write(&source, b"unowned source").context(IoSnafu)?;
    inner.scan().context(HotFolderSnafu)?;
    fs::remove_file(&source).context(IoSnafu)?;
    inner.reconcile();
    assert_eq!(inner.ingestion.state, IngestionState::Running);
    assert_eq!(inner.ingestion.missing_sources.len(), 1);
    assert!(inner.ingestion.missing_sources[0].group_id.is_none());
    let group = start(&mut inner, 0)?;
    fs::write(&source, b"new observation").context(IoSnafu)?;
    settle(&mut inner)?;
    assert_eq!(inner.ingestion.photos.len(), 1);
    assert!(inner.ingestion.photos[0].group_id.is_none());
    assert!(inner.ingestion.missing_sources.is_empty());
    let photo = inner.ingestion.photos[0].id;
    inner
        .apply(Action::Assign(photo, group), true)
        .context(HotFolderSnafu)?;
    assert_eq!(inner.ingestion.photos[0].group_id, Some(group));
    Ok(())
}

#[test]
fn removing_a_pending_source_after_it_exceeds_the_limit_unblocks_the_group() -> TestResult {
    let (_directory, mut inner) = fixture()?;
    let group = start(&mut inner, 0)?;
    let source = inner.ingestion.hot_folder.join("growing.jpg");
    fs::write(&source, b"initial segment").context(IoSnafu)?;
    inner.scan().context(HotFolderSnafu)?;
    let file = fs::OpenOptions::new()
        .write(true)
        .open(&source)
        .context(IoSnafu)?;
    file.set_len(50 * 1024 * 1024 + 1).context(IoSnafu)?;
    drop(file);
    inner.reconcile();
    assert!(inner.session.is_paused());
    fs::remove_file(&source).context(IoSnafu)?;
    inner.reconcile();
    assert_eq!(inner.ingestion.missing_sources.len(), 1);
    assert!(inner.ingestion.photos.is_empty());
    inner
        .apply(Action::Abandon(group), true)
        .context(HotFolderSnafu)?;
    Ok(())
}

#[test]
fn reconciliation_without_notifications_and_duplicate_scans_share_ingestion() -> TestResult {
    let (_directory, mut inner) = fixture()?;
    let group = start(&mut inner, 0)?;
    let path = inner.ingestion.hot_folder.join("one.JPG");
    fs::write(&path, b"source bytes").context(IoSnafu)?;
    settle(&mut inner)?;
    for _ in 0..3 {
        inner.scan().context(HotFolderSnafu)?;
    }
    assert_eq!(inner.ingestion.photos.len(), 1);
    assert_eq!(inner.ingestion.photos[0].group_id, Some(group));
    assert_eq!(
        inner.session.group().context(FixtureSnafu)?.pending_tasks(),
        0
    );
    assert_eq!(
        fs::read_dir(&inner.ingestion.archive)
            .context(IoSnafu)?
            .count(),
        1
    );
    assert_eq!(fs::read(path).context(IoSnafu)?, b"source bytes");
    Ok(())
}

#[test]
fn replacement_between_probe_and_copy_discards_temporary_file() -> TestResult {
    let (_directory, inner) = fixture()?;
    let path = inner.ingestion.hot_folder.join("one.png");
    fs::write(&path, b"before").context(IoSnafu)?;
    let version = archive::probe(&path, &inner.ingestion.hot_folder).context(ArchiveSnafu)?;
    fs::write(&path, b"after replacement").context(IoSnafu)?;
    assert!(matches!(
        archive::copy(
            &path,
            &inner.ingestion.hot_folder,
            &inner.ingestion.archive,
            &version
        ),
        Err(archive::ArchiveError::Changed)
    ));
    assert_eq!(
        fs::read_dir(&inner.ingestion.archive)
            .context(IoSnafu)?
            .count(),
        0
    );
    assert_eq!(fs::read(path).context(IoSnafu)?, b"after replacement");
    Ok(())
}

#[test]
fn identical_bytes_can_share_file_without_merging_group_ownership() -> TestResult {
    let (_directory, mut inner) = fixture()?;
    let first = start(&mut inner, 0)?;
    fs::write(inner.ingestion.hot_folder.join("one.jpg"), b"same image").context(IoSnafu)?;
    settle(&mut inner)?;
    inner.session.abandon_group(first).context(CaptureSnafu)?;
    let second = start(&mut inner, 1)?;
    fs::write(inner.ingestion.hot_folder.join("two.jpg"), b"same image").context(IoSnafu)?;
    settle(&mut inner)?;
    assert_eq!(inner.ingestion.photos.len(), 2);
    assert_eq!(inner.ingestion.photos[0].group_id, Some(first));
    assert_eq!(inner.ingestion.photos[1].group_id, Some(second));
    assert_ne!(inner.ingestion.photos[0].id, inner.ingestion.photos[1].id);
    assert_eq!(
        inner.ingestion.photos[0].path,
        inner.ingestion.photos[1].path
    );
    Ok(())
}

#[test]
fn same_path_overwrite_across_groups_requires_review() -> TestResult {
    let (_directory, mut inner) = fixture()?;
    let first = start(&mut inner, 0)?;
    let path = inner.ingestion.hot_folder.join("reused.jpg");
    fs::write(&path, b"first team").context(IoSnafu)?;
    settle(&mut inner)?;
    inner.session.abandon_group(first).context(CaptureSnafu)?;
    start(&mut inner, 1)?;
    fs::write(&path, b"second version").context(IoSnafu)?;
    settle(&mut inner)?;
    assert_eq!(inner.ingestion.photos.len(), 2);
    assert_eq!(inner.ingestion.photos[0].group_id, Some(first));
    assert!(inner.ingestion.photos[1].group_id.is_none());
    assert_eq!(
        fs::read(&inner.ingestion.photos[0].path).context(IoSnafu)?,
        b"first team"
    );
    Ok(())
}

#[test]
fn source_byte_limit_is_checked_before_allocating_source_contents() -> TestResult {
    let (_directory, inner) = fixture()?;
    let path = inner.ingestion.hot_folder.join("huge.jpg");
    let file = fs::File::create(&path).context(IoSnafu)?;
    file.set_len(50 * 1024 * 1024 + 1).context(IoSnafu)?;
    drop(file);
    assert!(matches!(
        archive::probe(&path, &inner.ingestion.hot_folder),
        Err(archive::ArchiveError::Limit)
    ));
    assert_eq!(
        fs::read_dir(&inner.ingestion.archive)
            .context(IoSnafu)?
            .count(),
        0
    );
    Ok(())
}

#[test]
fn corrupt_existing_archive_is_never_overwritten() -> TestResult {
    let (_directory, inner) = fixture()?;
    let source = inner.ingestion.hot_folder.join("one.png");
    fs::write(&source, b"original image").context(IoSnafu)?;
    let version = archive::probe(&source, &inner.ingestion.hot_folder).context(ArchiveSnafu)?;
    let destination = inner
        .ingestion
        .archive
        .join(format!("{}.source", version.sha256));
    fs::write(&destination, b"corrupt archive").context(IoSnafu)?;
    assert!(matches!(
        archive::copy(
            &source,
            &inner.ingestion.hot_folder,
            &inner.ingestion.archive,
            &version
        ),
        Err(archive::ArchiveError::InvalidArchive)
    ));
    assert_eq!(fs::read(destination).context(IoSnafu)?, b"corrupt archive");
    assert_eq!(
        fs::read_dir(&inner.ingestion.archive)
            .context(IoSnafu)?
            .count(),
        1
    );
    Ok(())
}

#[test]
fn directory_failure_pauses_instead_of_dropping_missing_tasks() -> TestResult {
    let (_directory, mut inner) = fixture()?;
    start(&mut inner, 0)?;
    let path = inner.ingestion.hot_folder.join("unfinished.png");
    fs::write(&path, b"in progress").context(IoSnafu)?;
    inner.scan().context(HotFolderSnafu)?;
    let unavailable = inner.ingestion.hot_folder.with_file_name("unavailable-hot");
    assert_eq!(unavailable.parent(), inner.ingestion.hot_folder.parent());
    fs::rename(&inner.ingestion.hot_folder, &unavailable).context(IoSnafu)?;
    inner.reconcile();
    assert!(inner.session.is_paused());
    assert_eq!(
        inner.session.group().context(FixtureSnafu)?.pending_tasks(),
        1
    );
    assert!(inner.ingestion.missing_sources.is_empty());
    fs::rename(&unavailable, &inner.ingestion.hot_folder).context(IoSnafu)?;
    settle(&mut inner)?;
    inner.reconcile();
    assert!(!inner.session.is_paused());
    assert_eq!(inner.ingestion.photos.len(), 1);
    Ok(())
}

#[test]
fn unchanged_archived_sources_skip_content_reads_and_periodic_audits_are_bounded() -> TestResult {
    let (_directory, mut inner) = fixture()?;
    start(&mut inner, 0)?;
    for index in 0..100 {
        fs::write(
            inner.ingestion.hot_folder.join(format!("{index}.jpg")),
            format!("image-{index}"),
        )
        .context(IoSnafu)?;
    }
    settle(&mut inner)?;
    assert_eq!(inner.ingestion.photos.len(), 100);
    let mut reads = 0;
    for _ in 0..5 {
        inner
            .ingestion
            .scan_with_probe(
                inner.session.group().map(capture::Group::id),
                |path, root| {
                    reads += 1;
                    archive::probe(path, root)
                },
            )
            .context(HotFolderSnafu)?;
    }
    assert_eq!(
        reads, 0,
        "idle scans must not hash the existing camera originals"
    );
    inner.ingestion.last_audit = Instant::now()
        .checked_sub(Duration::from_secs(31))
        .context(FixtureSnafu)?;
    for checked in inner.ingestion.checked.values_mut() {
        *checked = Instant::now()
            .checked_sub(Duration::from_secs(31))
            .context(FixtureSnafu)?;
    }
    inner
        .ingestion
        .scan_with_probe(
            inner.session.group().map(capture::Group::id),
            |path, root| {
                reads += 1;
                archive::probe(path, root)
            },
        )
        .context(HotFolderSnafu)?;
    assert_eq!(reads, 1, "one pass must not audit every unchanged original");
    inner
        .ingestion
        .scan_with_probe(
            inner.session.group().map(capture::Group::id),
            |path, root| {
                reads += 1;
                archive::probe(path, root)
            },
        )
        .context(HotFolderSnafu)?;
    assert_eq!(
        reads, 1,
        "many stale originals must not cause continuous full-file auditing"
    );
    Ok(())
}

#[test]
fn missing_source_acknowledgement_does_not_erase_reappearance_uncertainty() -> TestResult {
    let (_directory, mut inner) = fixture()?;
    let a = start(&mut inner, 0)?;
    let source = inner.ingestion.hot_folder.join("lost.jpg");
    fs::write(&source, b"partial A photo").context(IoSnafu)?;
    inner.scan().context(HotFolderSnafu)?;
    fs::remove_file(&source).context(IoSnafu)?;
    inner.reconcile();
    inner
        .apply(Action::AcknowledgeMissing(source.clone()), true)
        .context(HotFolderSnafu)?;
    assert!(inner.ingestion.missing_sources.is_empty());
    inner
        .apply(Action::Abandon(a), true)
        .context(HotFolderSnafu)?;
    start(&mut inner, 1)?;
    fs::write(source, b"returning A photo").context(IoSnafu)?;
    settle(&mut inner)?;
    assert!(inner.ingestion.photos[0].group_id.is_none());
    Ok(())
}

#[tokio::test]
async fn shutdown_preserves_a_worker_failure_instead_of_returning_success() -> TestResult {
    let (directory, inner) = fixture()?;
    let roster = crate::roster::Roster::parse(include_bytes!("../fixtures/roster.json"))
        .unwrap_or_else(|error| panic!("fixture roster: {error:?}"));
    let workspace = crate::workspace::fixture::create(&directory.path().join("workspace"), &roster)
        .await
        .unwrap_or_else(|error| panic!("fixture workspace: {error:?}"));
    let (commands, _receive) = mpsc::channel(1);
    let (_publish, snapshots) = watch::channel(inner.snapshot(0));
    let capture = CaptureSession {
        _workspace_owner: workspace.writable_owner(),
        commands,
        snapshots,
        stop: CancellationToken::new(),
        worker: Some(tokio::spawn(async { Err(Error::Stopped) })),
    };
    assert!(matches!(capture.shutdown().await, Err(Error::Stopped)));
    Ok(())
}

#[test]
fn bounded_audit_detects_same_length_content_with_restored_timestamps() -> TestResult {
    let (_directory, mut inner) = fixture()?;
    let a = start(&mut inner, 0)?;
    let source = inner.ingestion.hot_folder.join("reused.jpg");
    fs::write(&source, b"photo-A").context(IoSnafu)?;
    settle(&mut inner)?;
    let original = inner
        .ingestion
        .seen
        .get(&source)
        .context(FixtureSnafu)?
        .version
        .clone();
    inner
        .apply(Action::Abandon(a), true)
        .context(HotFolderSnafu)?;
    start(&mut inner, 1)?;
    fs::write(&source, b"photo-B").context(IoSnafu)?;
    fs::OpenOptions::new()
        .write(true)
        .open(&source)
        .context(IoSnafu)?
        .set_modified(original.modified)
        .context(IoSnafu)?;
    assert!(
        archive::unchanged(&source, &original).context(ArchiveSnafu)?,
        "fixture must preserve the cheap metadata fingerprint"
    );
    inner.ingestion.last_audit = Instant::now()
        .checked_sub(Duration::from_secs(31))
        .context(FixtureSnafu)?;
    *inner
        .ingestion
        .checked
        .get_mut(&source)
        .context(FixtureSnafu)? = inner.ingestion.last_audit;
    inner.scan().context(HotFolderSnafu)?;
    settle(&mut inner)?;
    assert_eq!(inner.ingestion.photos.len(), 2);
    assert_ne!(
        inner.ingestion.photos[0].sha256,
        inner.ingestion.photos[1].sha256
    );
    assert!(
        inner.ingestion.photos[1].group_id.is_none(),
        "a rewritten old source must not become B's photo automatically"
    );
    Ok(())
}
