use std::{fs, io::Write as _, path::PathBuf, time::Duration};

use natsume_photo_client::{
    hot_folder::{Error as HotFolderError, HotFolder, Issue, Snapshot},
    roster::Roster,
    workspace::Workspace,
};
use snafu::{OptionExt as _, ResultExt as _, Snafu};

const TEAM_A: &str = "0199a000-0000-7000-8000-000000000001";
const TEAM_B: &str = "0199a000-0000-7000-8000-000000000002";
const TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Snafu)]
enum Error {
    Io {
        source: std::io::Error,
    },
    Roster {
        source: natsume_photo_client::roster::Error,
    },
    Workspace {
        source: natsume_photo_client::workspace::Error,
    },
    HotFolder {
        source: HotFolderError,
    },
    Image {
        source: image::ImageError,
    },
    Timeout,
    Fixture,
}
type Result<T = ()> = std::result::Result<T, Error>;

struct Fixture {
    _directory: tempfile::TempDir,
    workspace: Workspace,
    hot: PathBuf,
}

fn fixture() -> Result<Fixture> {
    let directory = tempfile::tempdir().context(IoSnafu)?;
    let roster = Roster::parse(include_bytes!("fixtures/roster.json")).context(RosterSnafu)?;
    let workspace =
        Workspace::create(&directory.path().join("工作区"), &roster).context(WorkspaceSnafu)?;
    let hot = directory.path().join("相机 来图");
    fs::create_dir(&hot).context(IoSnafu)?;
    Ok(Fixture {
        _directory: directory,
        workspace,
        hot,
    })
}

fn wait(folder: &HotFolder, predicate: impl Fn(&Snapshot) -> bool) -> Result<Snapshot> {
    folder
        .wait_until(TIMEOUT, predicate)
        .context(HotFolderSnafu)?
        .context(TimeoutSnafu)
}

#[test]
fn rejects_nonempty_folders_files_subdirectories_and_workspace_overlap() -> Result {
    let mut fixture = fixture()?;
    let path = fixture.hot.join("unrelated.txt");
    fs::write(&path, b"must remain").context(IoSnafu)?;
    assert!(matches!(
        HotFolder::prepare(&mut fixture.workspace, &fixture.hot),
        Err(HotFolderError::NotEmpty)
    ));
    assert_eq!(fs::read(&path).context(IoSnafu)?, b"must remain");
    fs::remove_file(&path).context(IoSnafu)?;
    fs::create_dir(&path).context(IoSnafu)?;
    assert!(matches!(
        HotFolder::prepare(&mut fixture.workspace, &fixture.hot),
        Err(HotFolderError::NotEmpty)
    ));
    assert!(path.is_dir());
    let nested = fixture.workspace.root().join("empty-hot");
    fs::create_dir(&nested).context(IoSnafu)?;
    assert!(matches!(
        HotFolder::prepare(&mut fixture.workspace, &nested),
        Err(HotFolderError::UnsafePath)
    ));
    let ancestor = fixture
        .workspace
        .root()
        .parent()
        .context(FixtureSnafu)?
        .to_owned();
    assert!(matches!(
        HotFolder::prepare(&mut fixture.workspace, &ancestor),
        Err(HotFolderError::UnsafePath)
    ));
    assert!(matches!(
        HotFolder::prepare(&mut fixture.workspace, std::path::Path::new("relative")),
        Err(HotFolderError::UnsafePath)
    ));
    assert!(!fixture.workspace.root().join("captures").exists());
    Ok(())
}

#[test]
fn real_native_notifications_and_automatic_archive_preserve_source_and_database() -> Result {
    let mut fixture = fixture()?;
    let database_path = fixture.workspace.root().join("workspace.sqlite3");
    let database_before = fs::read(&database_path).context(IoSnafu)?;
    let folder =
        HotFolder::prepare(&mut fixture.workspace, &fixture.hot).context(HotFolderSnafu)?;
    folder.select_team(TEAM_A).context(HotFolderSnafu)?;
    let group = folder.start_group().context(HotFolderSnafu)?;
    let path = fixture.hot.join("真实照片.PNG");
    image::RgbImage::new(8, 6).save(&path).context(ImageSnafu)?;
    let state = wait(&folder, |state| {
        state.photos.len() == 1 && state.notifications > 0
    })?;
    let photo = &state.photos[0];
    assert_eq!(photo.group_id, Some(group));
    assert_eq!(photo.source.file_name(), path.file_name());
    assert_eq!(state.account_id.as_deref(), Some(TEAM_A));
    assert_eq!(state.pending_tasks, 0);
    assert_eq!(
        fs::read(&photo.path).context(IoSnafu)?,
        fs::read(&path).context(IoSnafu)?
    );
    assert_eq!(fs::read(database_path).context(IoSnafu)?, database_before);
    assert!(photo.path.starts_with(fixture.workspace.root()));
    folder.abandon_group(group).context(HotFolderSnafu)?;
    drop(folder);
    assert!(path.is_file());
    assert!(photo.path.is_file());
    Ok(())
}

#[cfg(windows)]
#[test]
fn segmented_windows_write_blocks_group_closure_until_writer_closes() -> Result {
    use std::os::windows::fs::OpenOptionsExt as _;
    let mut fixture = fixture()?;
    let folder =
        HotFolder::prepare(&mut fixture.workspace, &fixture.hot).context(HotFolderSnafu)?;
    folder.select_team(TEAM_A).context(HotFolderSnafu)?;
    let group = folder.start_group().context(HotFolderSnafu)?;
    let path = fixture.hot.join("partial.jpg");
    let mut writer = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .share_mode(1)
        .open(&path)
        .context(IoSnafu)?;
    writer.write_all(b"first segment ").context(IoSnafu)?;
    writer.flush().context(IoSnafu)?;
    let partial = wait(&folder, |state| {
        state.notifications > 0 && state.pending_tasks == 1
    })?;
    assert!(partial.photos.is_empty());
    assert!(folder.abandon_group(group).is_err());
    writer.write_all(b"second segment").context(IoSnafu)?;
    writer.sync_all().context(IoSnafu)?;
    drop(writer);
    let complete = wait(&folder, |state| {
        state.photos.len() == 1 && state.pending_tasks == 0
    })?;
    assert_eq!(
        fs::read(&complete.photos[0].path).context(IoSnafu)?,
        b"first segment second segment"
    );
    folder.abandon_group(group).context(HotFolderSnafu)?;
    Ok(())
}

#[test]
fn arrivals_without_group_and_reused_path_across_groups_stay_for_review() -> Result {
    let mut fixture = fixture()?;
    let folder =
        HotFolder::prepare(&mut fixture.workspace, &fixture.hot).context(HotFolderSnafu)?;
    let waiting = fixture.hot.join("before-selection.jpg");
    fs::write(&waiting, b"unknown team").context(IoSnafu)?;
    let initial = wait(&folder, |state| state.photos.len() == 1)?;
    assert!(initial.photos[0].group_id.is_none());
    folder.select_team(TEAM_A).context(HotFolderSnafu)?;
    let first = folder.start_group().context(HotFolderSnafu)?;
    folder
        .assign_reviewed(initial.photos[0].id, first)
        .context(HotFolderSnafu)?;
    assert!(folder.assign_reviewed(initial.photos[0].id, first).is_err());
    let reused = fixture.hot.join("same-name.jpg");
    fs::write(&reused, b"first group").context(IoSnafu)?;
    wait(&folder, |state| state.photos.len() == 2)?;
    folder.abandon_group(first).context(HotFolderSnafu)?;
    // A file physically present before starting the next group is found by the
    // boundary reconciliation even if native notification delivery is delayed.
    fs::write(fixture.hot.join("between.jpg"), b"late arrival").context(IoSnafu)?;
    folder.select_team(TEAM_B).context(HotFolderSnafu)?;
    let second = folder.start_group().context(HotFolderSnafu)?;
    fs::write(&reused, b"overwritten across boundary").context(IoSnafu)?;
    let state = wait(&folder, |state| state.photos.len() == 4)?;
    assert_eq!(
        state
            .photos
            .iter()
            .filter(|photo| photo.group_id == Some(first))
            .count(),
        2
    );
    assert_eq!(
        state
            .photos
            .iter()
            .filter(|photo| photo.group_id.is_none())
            .count(),
        2
    );
    assert!(
        state
            .photos
            .iter()
            .all(|photo| photo.group_id != Some(second))
    );
    for photo in state.photos.iter().filter(|photo| photo.group_id.is_none()) {
        folder
            .assign_reviewed(photo.id, second)
            .context(HotFolderSnafu)?;
    }
    Ok(())
}

#[test]
fn archive_failure_pauses_and_automatic_retry_keeps_group_identity() -> Result {
    let mut fixture = fixture()?;
    let folder =
        HotFolder::prepare(&mut fixture.workspace, &fixture.hot).context(HotFolderSnafu)?;
    folder.select_team(TEAM_A).context(HotFolderSnafu)?;
    let group = folder.start_group().context(HotFolderSnafu)?;
    let archive = fixture.workspace.root().join("captures").join(
        folder
            .snapshot()
            .context(HotFolderSnafu)?
            .session_id
            .to_string(),
    );
    let saved = archive.with_extension("saved");
    fs::rename(&archive, &saved).context(IoSnafu)?;
    fs::write(&archive, b"simulate destination failure").context(IoSnafu)?;
    fs::write(fixture.hot.join("photo.jpg"), b"keep original").context(IoSnafu)?;
    let failed = wait(&folder, |state| state.paused && state.issue.is_some())?;
    assert_eq!(failed.group_id, Some(group));
    assert_eq!(failed.pending_tasks, 1);
    assert!(folder.abandon_group(group).is_err());
    assert!(folder.select_team(TEAM_B).is_err());
    fs::remove_file(&archive).context(IoSnafu)?;
    fs::rename(saved, &archive).context(IoSnafu)?;
    let recovered = wait(&folder, |state| !state.paused && state.photos.len() == 1)?;
    assert_eq!(recovered.photos[0].group_id, Some(group));
    assert_eq!(
        fs::read(&recovered.photos[0].path).context(IoSnafu)?,
        b"keep original"
    );
    Ok(())
}

#[test]
fn removed_monitor_directory_pauses_then_rewatches_and_quarantines_uncertain_files() -> Result {
    let mut fixture = fixture()?;
    let folder =
        HotFolder::prepare(&mut fixture.workspace, &fixture.hot).context(HotFolderSnafu)?;
    folder.select_team(TEAM_A).context(HotFolderSnafu)?;
    let group = folder.start_group().context(HotFolderSnafu)?;
    fs::remove_dir(&fixture.hot).context(IoSnafu)?;
    wait(&folder, |state| {
        state.paused && state.issue == Some(Issue::Monitor)
    })?;
    assert!(folder.abandon_group(group).is_err());
    fs::create_dir(&fixture.hot).context(IoSnafu)?;
    fs::write(
        fixture.hot.join("during-disconnect.png"),
        b"uncertain owner",
    )
    .context(IoSnafu)?;
    let state = wait(&folder, |state| !state.paused && state.photos.len() == 1)?;
    assert_eq!(state.group_id, Some(group));
    assert!(state.photos[0].group_id.is_none());
    folder
        .assign_reviewed(state.photos[0].id, group)
        .context(HotFolderSnafu)?;
    Ok(())
}

#[test]
fn restart_refuses_hot_folder_leftovers_and_does_not_restore_old_records() -> Result {
    let mut fixture = fixture()?;
    let folder =
        HotFolder::prepare(&mut fixture.workspace, &fixture.hot).context(HotFolderSnafu)?;
    let old_session = folder.snapshot().context(HotFolderSnafu)?.session_id;
    folder.select_team(TEAM_A).context(HotFolderSnafu)?;
    folder.start_group().context(HotFolderSnafu)?;
    let path = fixture.hot.join("unfinished-team.jpg");
    fs::write(&path, b"unconfirmed source").context(IoSnafu)?;
    let state = wait(&folder, |state| state.photos.len() == 1)?;
    drop(folder);
    assert!(matches!(
        HotFolder::prepare(&mut fixture.workspace, &fixture.hot),
        Err(HotFolderError::NotEmpty)
    ));
    assert!(path.is_file());
    assert!(state.photos[0].path.is_file());
    let fresh_hot = fixture.hot.with_extension("new-empty");
    fs::create_dir(&fresh_hot).context(IoSnafu)?;
    let fresh = HotFolder::prepare(&mut fixture.workspace, &fresh_hot).context(HotFolderSnafu)?;
    let state = fresh.snapshot().context(HotFolderSnafu)?;
    assert_ne!(state.session_id, old_session);
    assert!(state.group_id.is_none());
    assert!(state.photos.is_empty());
    assert_eq!(state.pending_tasks, 0);
    Ok(())
}

#[test]
fn unknown_extensions_are_not_imported_but_remain_in_source_folder() -> Result {
    let mut fixture = fixture()?;
    let folder =
        HotFolder::prepare(&mut fixture.workspace, &fixture.hot).context(HotFolderSnafu)?;
    fs::write(fixture.hot.join("camera.raw"), b"raw bytes").context(IoSnafu)?;
    fs::write(fixture.hot.join("supported.jpeg"), b"jpeg input").context(IoSnafu)?;
    let state = wait(&folder, |state| state.photos.len() == 1)?;
    assert_eq!(
        state.photos[0]
            .source
            .extension()
            .and_then(|ext| ext.to_str()),
        Some("jpeg")
    );
    assert!(fixture.hot.join("camera.raw").is_file());
    Ok(())
}
