use super::*;
use crate::roster::Roster;

#[derive(Debug, Snafu)]
enum TestError {
    Io { source: std::io::Error },
    Roster { source: crate::roster::Error },
    Capture { source: capture::Error },
    HotFolder { source: Error },
    Archive { source: archive::ArchiveError },
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
    Ok((
        directory,
        Inner {
            session: Session::new(roster),
            hot_folder: fs::canonicalize(hot_folder).context(IoSnafu)?,
            archive: fs::canonicalize(archive).context(IoSnafu)?,
            pending: BTreeMap::new(),
            seen: BTreeMap::new(),
            photos: Vec::new(),
            state: IngestionState::Running,
            uncertain: false,
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
    for pending in inner.pending.values_mut() {
        pending.stable_since = Instant::now().checked_sub(INTERVAL).context(FixtureSnafu)?;
    }
    inner.scan().context(HotFolderSnafu)
}

#[test]
fn reconciliation_without_notifications_and_duplicate_scans_share_ingestion() -> TestResult {
    let (_directory, mut inner) = fixture()?;
    let group = start(&mut inner, 0)?;
    let path = inner.hot_folder.join("one.JPG");
    fs::write(&path, b"source bytes").context(IoSnafu)?;
    settle(&mut inner)?;
    for _ in 0..3 {
        inner.scan().context(HotFolderSnafu)?;
    }
    assert_eq!(inner.photos.len(), 1);
    assert_eq!(inner.photos[0].group_id, Some(group));
    assert_eq!(
        inner.session.group().context(FixtureSnafu)?.pending_tasks(),
        0
    );
    assert_eq!(fs::read_dir(&inner.archive).context(IoSnafu)?.count(), 1);
    assert_eq!(fs::read(path).context(IoSnafu)?, b"source bytes");
    Ok(())
}

#[test]
fn replacement_between_probe_and_copy_discards_temporary_file() -> TestResult {
    let (_directory, inner) = fixture()?;
    let path = inner.hot_folder.join("one.png");
    fs::write(&path, b"before").context(IoSnafu)?;
    let version = archive::probe(&path, &inner.hot_folder).context(ArchiveSnafu)?;
    fs::write(&path, b"after replacement").context(IoSnafu)?;
    assert!(matches!(
        archive::copy(&path, &inner.hot_folder, &inner.archive, &version),
        Err(archive::ArchiveError::Changed)
    ));
    assert_eq!(fs::read_dir(&inner.archive).context(IoSnafu)?.count(), 0);
    assert_eq!(fs::read(path).context(IoSnafu)?, b"after replacement");
    Ok(())
}

#[test]
fn identical_bytes_can_share_file_without_merging_group_ownership() -> TestResult {
    let (_directory, mut inner) = fixture()?;
    let first = start(&mut inner, 0)?;
    fs::write(inner.hot_folder.join("one.jpg"), b"same image").context(IoSnafu)?;
    settle(&mut inner)?;
    inner.session.abandon_group(first).context(CaptureSnafu)?;
    let second = start(&mut inner, 1)?;
    fs::write(inner.hot_folder.join("two.jpg"), b"same image").context(IoSnafu)?;
    settle(&mut inner)?;
    assert_eq!(inner.photos.len(), 2);
    assert_eq!(inner.photos[0].group_id, Some(first));
    assert_eq!(inner.photos[1].group_id, Some(second));
    assert_ne!(inner.photos[0].id, inner.photos[1].id);
    assert_eq!(inner.photos[0].path, inner.photos[1].path);
    Ok(())
}

#[test]
fn same_path_overwrite_across_groups_requires_review() -> TestResult {
    let (_directory, mut inner) = fixture()?;
    let first = start(&mut inner, 0)?;
    let path = inner.hot_folder.join("reused.jpg");
    fs::write(&path, b"first team").context(IoSnafu)?;
    settle(&mut inner)?;
    inner.session.abandon_group(first).context(CaptureSnafu)?;
    start(&mut inner, 1)?;
    fs::write(&path, b"second version").context(IoSnafu)?;
    settle(&mut inner)?;
    assert_eq!(inner.photos.len(), 2);
    assert_eq!(inner.photos[0].group_id, Some(first));
    assert!(inner.photos[1].group_id.is_none());
    assert_eq!(
        fs::read(&inner.photos[0].path).context(IoSnafu)?,
        b"first team"
    );
    Ok(())
}

#[test]
fn source_byte_limit_is_checked_before_allocating_source_contents() -> TestResult {
    let (_directory, inner) = fixture()?;
    let path = inner.hot_folder.join("huge.jpg");
    let file = fs::File::create(&path).context(IoSnafu)?;
    file.set_len(50 * 1024 * 1024 + 1).context(IoSnafu)?;
    drop(file);
    assert!(matches!(
        archive::probe(&path, &inner.hot_folder),
        Err(archive::ArchiveError::Limit)
    ));
    assert_eq!(fs::read_dir(&inner.archive).context(IoSnafu)?.count(), 0);
    Ok(())
}

#[test]
fn corrupt_existing_archive_is_never_overwritten() -> TestResult {
    let (_directory, inner) = fixture()?;
    let source = inner.hot_folder.join("one.png");
    fs::write(&source, b"original image").context(IoSnafu)?;
    let version = archive::probe(&source, &inner.hot_folder).context(ArchiveSnafu)?;
    let destination = inner.archive.join(format!("{}.source", version.sha256));
    fs::write(&destination, b"corrupt archive").context(IoSnafu)?;
    assert!(matches!(
        archive::copy(&source, &inner.hot_folder, &inner.archive, &version),
        Err(archive::ArchiveError::InvalidArchive)
    ));
    assert_eq!(fs::read(destination).context(IoSnafu)?, b"corrupt archive");
    assert_eq!(fs::read_dir(&inner.archive).context(IoSnafu)?.count(), 1);
    Ok(())
}

#[test]
fn directory_failure_pauses_instead_of_dropping_missing_tasks() -> TestResult {
    let (_directory, mut inner) = fixture()?;
    start(&mut inner, 0)?;
    let path = inner.hot_folder.join("unfinished.png");
    fs::write(&path, b"in progress").context(IoSnafu)?;
    inner.scan().context(HotFolderSnafu)?;
    fs::remove_file(&path).context(IoSnafu)?;
    inner.reconcile();
    assert!(inner.session.is_paused());
    assert_eq!(
        inner.session.group().context(FixtureSnafu)?.pending_tasks(),
        1
    );
    fs::write(&path, b"restored").context(IoSnafu)?;
    settle(&mut inner)?;
    inner.reconcile();
    assert!(!inner.session.is_paused());
    assert_eq!(inner.photos.len(), 1);
    Ok(())
}
