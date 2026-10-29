#![cfg(windows)]
#[cfg(windows)]
use std::io::Write as _;
use std::{fs, path::PathBuf, time::Duration};

use natsume_photo_client::{
    hot_folder::{Error as HotFolderError, HotFolder, HotFolderSnapshot, IngestionState},
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

async fn fixture() -> Result<Fixture> {
    let directory = tempfile::tempdir().context(IoSnafu)?;
    let roster = Roster::parse(include_bytes!("fixtures/roster.json")).context(RosterSnafu)?;
    let workspace = Workspace::create(&directory.path().join("工作区"), &roster)
        .await
        .context(WorkspaceSnafu)?;
    let hot = directory.path().join("相机 来图");
    fs::create_dir(&hot).context(IoSnafu)?;
    Ok(Fixture {
        _directory: directory,
        workspace,
        hot,
    })
}

async fn wait(
    folder: &HotFolder,
    predicate: impl Fn(&HotFolderSnapshot) -> bool,
) -> Result<HotFolderSnapshot> {
    folder
        .wait_until(TIMEOUT, predicate)
        .await
        .context(HotFolderSnafu)?
        .context(TimeoutSnafu)
}

#[tokio::test]
async fn rejects_nonempty_folders_files_subdirectories_and_workspace_overlap() -> Result {
    let fixture = fixture().await?;
    let path = fixture.hot.join("unrelated.txt");
    fs::write(&path, b"must remain").context(IoSnafu)?;
    assert!(matches!(
        HotFolder::prepare(&fixture.workspace, &fixture.hot).await,
        Err(HotFolderError::NotEmpty)
    ));
    assert_eq!(fs::read(&path).context(IoSnafu)?, b"must remain");
    fs::remove_file(&path).context(IoSnafu)?;
    fs::create_dir(&path).context(IoSnafu)?;
    assert!(matches!(
        HotFolder::prepare(&fixture.workspace, &fixture.hot).await,
        Err(HotFolderError::NotEmpty)
    ));
    assert!(path.is_dir());
    let nested = fixture.workspace.root().join("empty-hot");
    fs::create_dir(&nested).context(IoSnafu)?;
    assert!(matches!(
        HotFolder::prepare(&fixture.workspace, &nested).await,
        Err(HotFolderError::UnsafePath)
    ));
    let ancestor = fixture
        .workspace
        .root()
        .parent()
        .context(FixtureSnafu)?
        .to_owned();
    assert!(matches!(
        HotFolder::prepare(&fixture.workspace, &ancestor).await,
        Err(HotFolderError::UnsafePath)
    ));
    assert!(matches!(
        HotFolder::prepare(&fixture.workspace, std::path::Path::new("relative")).await,
        Err(HotFolderError::UnsafePath)
    ));
    assert!(!fixture.workspace.root().join("captures").exists());
    Ok(())
}

#[tokio::test]
async fn real_native_notifications_and_automatic_archive_preserve_source_and_database() -> Result {
    let fixture = fixture().await?;
    let database_path = fixture.workspace.root().join("workspace.sqlite3");
    let database_before = fs::read(&database_path).context(IoSnafu)?;
    let folder = HotFolder::prepare(&fixture.workspace, &fixture.hot)
        .await
        .context(HotFolderSnafu)?;
    folder.select_team(TEAM_A).await.context(HotFolderSnafu)?;
    let group = folder.start_group().await.context(HotFolderSnafu)?;
    let path = fixture.hot.join("真实照片.PNG");
    image::RgbImage::new(8, 6).save(&path).context(ImageSnafu)?;
    let state = wait(&folder, |state| state.photos.len() == 1).await?;
    // WinRT coalesces notifications and may miss the first immediate change.
    // Verify archival independently, then require a real notification from a
    // bounded sequence of subsequent changes (these files are never imported).
    let mut notified = state.notifications > 0;
    for index in 0..5 {
        if notified {
            break;
        }
        fs::write(
            fixture.hot.join(format!("notification-{index}.txt")),
            b"probe",
        )
        .context(IoSnafu)?;
        notified = folder
            .wait_until(Duration::from_secs(2), |s| s.notifications > 0)
            .await
            .context(HotFolderSnafu)?
            .is_some();
    }
    assert!(
        notified,
        "native notifications must reach the ingestion worker"
    );
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
    folder.abandon_group(group).await.context(HotFolderSnafu)?;
    folder.shutdown().await.context(HotFolderSnafu)?;
    assert!(path.is_file());
    assert!(photo.path.is_file());
    Ok(())
}

#[cfg(windows)]
#[tokio::test]
async fn segmented_windows_write_blocks_group_closure_until_writer_closes() -> Result {
    use std::os::windows::fs::OpenOptionsExt as _;
    let fixture = fixture().await?;
    let folder = HotFolder::prepare(&fixture.workspace, &fixture.hot)
        .await
        .context(HotFolderSnafu)?;
    folder.select_team(TEAM_A).await.context(HotFolderSnafu)?;
    let group = folder.start_group().await.context(HotFolderSnafu)?;
    let path = fixture.hot.join("partial.jpg");
    let mut writer = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .share_mode(1)
        .open(&path)
        .context(IoSnafu)?;
    writer.write_all(b"first segment ").context(IoSnafu)?;
    writer.flush().context(IoSnafu)?;
    let partial = wait(&folder, |state| state.pending_tasks == 1).await?;
    assert!(partial.photos.is_empty());
    assert!(folder.abandon_group(group).await.is_err());
    writer.write_all(b"second segment").context(IoSnafu)?;
    writer.sync_all().context(IoSnafu)?;
    drop(writer);
    let complete = wait(&folder, |state| {
        state.photos.len() == 1 && state.pending_tasks == 0
    })
    .await?;
    assert_eq!(
        fs::read(&complete.photos[0].path).context(IoSnafu)?,
        b"first segment second segment"
    );
    folder.abandon_group(group).await.context(HotFolderSnafu)?;
    Ok(())
}

#[tokio::test]
async fn arrivals_without_group_and_reused_path_across_groups_stay_for_review() -> Result {
    let fixture = fixture().await?;
    let folder = HotFolder::prepare(&fixture.workspace, &fixture.hot)
        .await
        .context(HotFolderSnafu)?;
    let waiting = fixture.hot.join("before-selection.jpg");
    fs::write(&waiting, b"unknown team").context(IoSnafu)?;
    let initial = wait(&folder, |state| state.photos.len() == 1).await?;
    assert!(initial.photos[0].group_id.is_none());
    folder.select_team(TEAM_A).await.context(HotFolderSnafu)?;
    let first = folder.start_group().await.context(HotFolderSnafu)?;
    folder
        .assign_reviewed(initial.photos[0].id, first)
        .await
        .context(HotFolderSnafu)?;
    assert!(
        folder
            .assign_reviewed(initial.photos[0].id, first)
            .await
            .is_err()
    );
    let reused = fixture.hot.join("same-name.jpg");
    fs::write(&reused, b"first group").context(IoSnafu)?;
    wait(&folder, |state| state.photos.len() == 2).await?;
    folder.abandon_group(first).await.context(HotFolderSnafu)?;
    // A file physically present before starting the next group is found by the
    // boundary reconciliation even if native notification delivery is delayed.
    fs::write(fixture.hot.join("between.jpg"), b"late arrival").context(IoSnafu)?;
    folder.select_team(TEAM_B).await.context(HotFolderSnafu)?;
    let second = folder.start_group().await.context(HotFolderSnafu)?;
    fs::write(&reused, b"overwritten across boundary").context(IoSnafu)?;
    let state = wait(&folder, |state| state.photos.len() == 4).await?;
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
            .await
            .context(HotFolderSnafu)?;
    }
    Ok(())
}

#[tokio::test]
async fn archive_failure_pauses_and_automatic_retry_keeps_group_identity() -> Result {
    let fixture = fixture().await?;
    let folder = HotFolder::prepare(&fixture.workspace, &fixture.hot)
        .await
        .context(HotFolderSnafu)?;
    folder.select_team(TEAM_A).await.context(HotFolderSnafu)?;
    let group = folder.start_group().await.context(HotFolderSnafu)?;
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
    let failed = wait(&folder, |state| {
        state.paused && state.state != IngestionState::Running
    })
    .await?;
    assert_eq!(failed.group_id, Some(group));
    assert_eq!(failed.pending_tasks, 1);
    assert!(folder.abandon_group(group).await.is_err());
    assert!(folder.select_team(TEAM_B).await.is_err());
    fs::remove_file(&archive).context(IoSnafu)?;
    fs::rename(saved, &archive).context(IoSnafu)?;
    let recovered = wait(&folder, |state| !state.paused && state.photos.len() == 1).await?;
    assert_eq!(recovered.photos[0].group_id, Some(group));
    assert_eq!(
        fs::read(&recovered.photos[0].path).context(IoSnafu)?,
        b"keep original"
    );
    Ok(())
}

#[tokio::test]
async fn removed_monitor_directory_pauses_then_rewatches_and_quarantines_uncertain_files() -> Result
{
    let fixture = fixture().await?;
    let folder = HotFolder::prepare(&fixture.workspace, &fixture.hot)
        .await
        .context(HotFolderSnafu)?;
    folder.select_team(TEAM_A).await.context(HotFolderSnafu)?;
    let group = folder.start_group().await.context(HotFolderSnafu)?;
    fs::remove_dir(&fixture.hot).context(IoSnafu)?;
    wait(&folder, |state| {
        state.paused && state.state == IngestionState::RecoveringMonitor
    })
    .await?;
    assert!(folder.abandon_group(group).await.is_err());
    fs::create_dir(&fixture.hot).context(IoSnafu)?;
    fs::write(
        fixture.hot.join("during-disconnect.png"),
        b"uncertain owner",
    )
    .context(IoSnafu)?;
    let state = wait(&folder, |state| !state.paused && state.photos.len() == 1).await?;
    assert_eq!(state.group_id, Some(group));
    assert!(state.photos[0].group_id.is_none());
    folder
        .assign_reviewed(state.photos[0].id, group)
        .await
        .context(HotFolderSnafu)?;
    Ok(())
}

#[tokio::test]
async fn restart_refuses_hot_folder_leftovers_and_does_not_restore_old_records() -> Result {
    let fixture = fixture().await?;
    let folder = HotFolder::prepare(&fixture.workspace, &fixture.hot)
        .await
        .context(HotFolderSnafu)?;
    let old_session = folder.snapshot().context(HotFolderSnafu)?.session_id;
    folder.select_team(TEAM_A).await.context(HotFolderSnafu)?;
    folder.start_group().await.context(HotFolderSnafu)?;
    let path = fixture.hot.join("unfinished-team.jpg");
    fs::write(&path, b"unconfirmed source").context(IoSnafu)?;
    let state = wait(&folder, |state| state.photos.len() == 1).await?;
    folder.shutdown().await.context(HotFolderSnafu)?;
    assert!(matches!(
        HotFolder::prepare(&fixture.workspace, &fixture.hot).await,
        Err(HotFolderError::NotEmpty)
    ));
    assert!(path.is_file());
    assert!(state.photos[0].path.is_file());
    let fresh_hot = fixture.hot.with_extension("new-empty");
    fs::create_dir(&fresh_hot).context(IoSnafu)?;
    let fresh = HotFolder::prepare(&fixture.workspace, &fresh_hot)
        .await
        .context(HotFolderSnafu)?;
    let state = fresh.snapshot().context(HotFolderSnafu)?;
    assert_ne!(state.session_id, old_session);
    assert!(state.group_id.is_none());
    assert!(state.photos.is_empty());
    assert_eq!(state.pending_tasks, 0);
    Ok(())
}

#[tokio::test]
async fn unknown_extensions_are_not_imported_but_remain_in_source_folder() -> Result {
    let fixture = fixture().await?;
    let folder = HotFolder::prepare(&fixture.workspace, &fixture.hot)
        .await
        .context(HotFolderSnafu)?;
    fs::write(fixture.hot.join("camera.raw"), b"raw bytes").context(IoSnafu)?;
    fs::write(fixture.hot.join("supported.jpeg"), b"jpeg input").context(IoSnafu)?;
    let state = wait(&folder, |state| state.photos.len() == 1).await?;
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

#[tokio::test]
async fn worker_reports_deleted_and_renamed_pending_sources_without_stalling() -> Result {
    use std::os::windows::fs::OpenOptionsExt as _;
    for rename in [false, true] {
        let fixture = fixture().await?;
        let folder = HotFolder::prepare(&fixture.workspace, &fixture.hot)
            .await
            .context(HotFolderSnafu)?;
        folder.select_team(TEAM_A).await.context(HotFolderSnafu)?;
        let group = folder.start_group().await.context(HotFolderSnafu)?;
        let source = fixture.hot.join("unfinished.jpg");
        // Permit external rename/deletion, but keep the writer open so archival
        // cannot complete before the test observes the pending task.
        let mut writer = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .share_mode(1 | 4)
            .open(&source)
            .context(IoSnafu)?;
        writer
            .write_all(b"unfinished source bytes")
            .context(IoSnafu)?;
        writer.flush().context(IoSnafu)?;
        wait(&folder, |s| s.pending_tasks == 1).await?;
        let renamed = fixture.hot.join("renamed.jpg");
        if rename {
            fs::rename(&source, &renamed).context(IoSnafu)?;
        } else {
            fs::remove_file(&source).context(IoSnafu)?;
        }
        drop(writer);
        let loss = wait(&folder, |s| s.missing_sources.len() == 1).await?;
        assert_eq!(
            loss.missing_sources[0].source.file_name(),
            source.file_name()
        );
        assert_eq!(loss.missing_sources[0].group_id, Some(group));
        assert_eq!(loss.pending_tasks, 0);
        if rename {
            let archived = wait(&folder, |s| s.photos.len() == 1).await?;
            assert!(archived.photos[0].group_id.is_none());
            assert!(renamed.is_file());
        }
        folder.abandon_group(group).await.context(HotFolderSnafu)?;
        folder.select_team(TEAM_B).await.context(HotFolderSnafu)?;
        let next = folder.start_group().await.context(HotFolderSnafu)?;
        assert_ne!(next, group);
        folder.shutdown().await.context(HotFolderSnafu)?;
        let remaining_sources = fs::read_dir(&fixture.hot).context(IoSnafu)?.count();
        assert_eq!(remaining_sources, usize::from(rename));
    }
    Ok(())
}
