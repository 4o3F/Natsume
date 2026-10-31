#[cfg(windows)]
use std::io::Write as _;
use std::{fs, path::PathBuf, time::Duration};

use crate::capture_session::CaptureSession;
use crate::{
    capture_session::{Error as HotFolderError, HotFolderSnapshot, IngestionState, PhotoCandidate},
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
        source: crate::roster::Error,
    },
    Workspace {
        source: crate::workspace::Error,
    },
    HotFolder {
        source: HotFolderError,
    },
    Image {
        source: image::ImageError,
    },
    Candidate {
        source: crate::capture_session::CandidateError,
    },
    Exif {
        source: exif::Error,
    },
    Database {
        source: diesel::ConnectionError,
    },
    Query {
        source: diesel::result::Error,
    },
    Timeout,
    Fixture,
}

fn write_timed_png(path: &std::path::Path, time: &[u8]) -> Result<()> {
    use image::ImageEncoder as _;
    let fields = [
        exif::Field {
            tag: exif::Tag::DateTimeOriginal,
            ifd_num: exif::In::PRIMARY,
            value: exif::Value::Ascii(vec![time.to_vec()]),
        },
        exif::Field {
            tag: exif::Tag::OffsetTimeOriginal,
            ifd_num: exif::In::PRIMARY,
            value: exif::Value::Ascii(vec![b"+00:00".to_vec()]),
        },
    ];
    let mut metadata = exif::experimental::Writer::new();
    for field in &fields {
        metadata.push_field(field);
    }
    let mut bytes = std::io::Cursor::new(Vec::new());
    metadata.write(&mut bytes, false).context(ExifSnafu)?;
    let mut encoder = image::codecs::png::PngEncoder::new(fs::File::create(path).context(IoSnafu)?);
    encoder
        .set_exif_metadata(bytes.into_inner())
        .map_err(image::ImageError::Unsupported)
        .context(ImageSnafu)?;
    encoder
        .write_image(&[30; 8 * 6 * 3], 8, 6, image::ExtendedColorType::Rgb8)
        .context(ImageSnafu)
}

#[tokio::test]
async fn candidate_selection_uses_capture_time_and_manual_choices_survive_new_images() -> Result {
    use crate::capture_session::{CandidateSelection, PhotoTimeSource};
    let fixture = fixture().await?;
    let folder = CaptureSession::prepare(&fixture.workspace, &fixture.hot)
        .await
        .context(HotFolderSnafu)?;
    folder.select_team(TEAM_A).await.context(HotFolderSnafu)?;
    let group = folder.start_group().await.context(HotFolderSnafu)?;
    assert_eq!(
        folder.snapshot().context(HotFolderSnafu)?.selection,
        CandidateSelection::Automatic { photo_id: None }
    );

    write_timed_png(&fixture.hot.join("new.png"), b"2026:10:31 10:00:02")?;
    let snapshot = wait(&folder, |s| {
        s.candidates().count() == 1 && s.pending_tasks == 0
    })
    .await?;
    let newer = snapshot.selection.photo_id().context(FixtureSnafu)?;
    assert_eq!(
        snapshot
            .candidates()
            .next()
            .context(FixtureSnafu)?
            .captured_at()
            .source(),
        PhotoTimeSource::ExifOriginal
    );

    // This older exposure finishes processing later; it must not steal selection.
    write_timed_png(&fixture.hot.join("old.png"), b"2026:10:31 10:00:01")?;
    let snapshot = wait(&folder, |s| {
        s.candidates().count() == 2 && s.pending_tasks == 0
    })
    .await?;
    assert_eq!(snapshot.selection.photo_id(), Some(newer));
    let older = snapshot
        .candidates()
        .last()
        .context(FixtureSnafu)?
        .photo_id();
    folder
        .select_candidate(group, older)
        .await
        .context(HotFolderSnafu)?;
    assert_eq!(
        folder.snapshot().context(HotFolderSnafu)?.selection,
        CandidateSelection::Manual { photo_id: older }
    );

    write_timed_png(&fixture.hot.join("latest.png"), b"2026:10:31 10:00:03")?;
    let snapshot = wait(&folder, |s| {
        s.candidates().count() == 3 && s.pending_tasks == 0
    })
    .await?;
    let latest = snapshot
        .candidates()
        .next()
        .context(FixtureSnafu)?
        .photo_id();
    assert_eq!(
        snapshot.selection,
        CandidateSelection::Manual { photo_id: older }
    );
    assert!(matches!(
        folder.select_candidate(group, uuid::Uuid::now_v7()).await,
        Err(HotFolderError::CandidateUnavailable)
    ));
    assert_eq!(
        folder
            .snapshot()
            .context(HotFolderSnafu)?
            .selection
            .photo_id(),
        Some(older)
    );
    folder
        .use_latest_candidate(group)
        .await
        .context(HotFolderSnafu)?;
    assert_eq!(
        folder.snapshot().context(HotFolderSnafu)?.selection,
        CandidateSelection::Automatic {
            photo_id: Some(latest)
        }
    );

    folder.abandon_group(group).await.context(HotFolderSnafu)?;
    folder.shutdown().await.context(HotFolderSnafu)?;
    Ok(())
}

#[tokio::test]
async fn candidate_selection_rejects_previous_groups_and_resets_manual_mode() -> Result {
    use crate::capture_session::CandidateSelection;
    let fixture = fixture().await?;
    let folder = CaptureSession::prepare(&fixture.workspace, &fixture.hot)
        .await
        .context(HotFolderSnafu)?;
    folder.select_team(TEAM_A).await.context(HotFolderSnafu)?;
    let group = folder.start_group().await.context(HotFolderSnafu)?;
    write_timed_png(&fixture.hot.join("one.png"), b"2026:10:31 10:00:00")?;
    let state = wait(&folder, |s| {
        s.candidates().count() == 1 && s.pending_tasks == 0
    })
    .await?;
    let older = state.selection.photo_id().context(FixtureSnafu)?;

    folder
        .select_candidate(group, older)
        .await
        .context(HotFolderSnafu)?;
    folder.abandon_group(group).await.context(HotFolderSnafu)?;
    folder.select_team(TEAM_B).await.context(HotFolderSnafu)?;
    let next = folder.start_group().await.context(HotFolderSnafu)?;
    assert_eq!(
        folder.snapshot().context(HotFolderSnafu)?.selection,
        CandidateSelection::Automatic { photo_id: None }
    );
    assert!(matches!(
        folder.select_candidate(group, older).await,
        Err(HotFolderError::CandidateUnavailable)
    ));
    assert!(matches!(
        folder.select_candidate(next, older).await,
        Err(HotFolderError::CandidateUnavailable)
    ));
    assert!(matches!(
        folder.use_latest_candidate(group).await,
        Err(HotFolderError::CandidateUnavailable)
    ));
    folder.abandon_group(next).await.context(HotFolderSnafu)?;
    folder.shutdown().await.context(HotFolderSnafu)?;
    Ok(())
}

#[tokio::test]
async fn equal_capture_times_have_a_stable_identity_order() -> Result {
    let fixture = fixture().await?;
    let folder = CaptureSession::prepare(&fixture.workspace, &fixture.hot)
        .await
        .context(HotFolderSnafu)?;
    folder.select_team(TEAM_A).await.context(HotFolderSnafu)?;
    let group = folder.start_group().await.context(HotFolderSnafu)?;
    write_timed_png(&fixture.hot.join("one.png"), b"2026:10:31 10:00:00")?;
    write_timed_png(&fixture.hot.join("two.png"), b"2026:10:31 10:00:00")?;
    let state = wait(&folder, |s| {
        s.candidates().count() == 2 && s.pending_tasks == 0
    })
    .await?;
    let actual: Vec<_> = state.candidates().map(PhotoCandidate::photo_id).collect();
    let mut sorted = actual.clone();
    sorted.sort_unstable_by(|left, right| right.cmp(left));
    assert_eq!(actual, sorted);
    assert_eq!(state.selection.photo_id(), actual.first().copied());
    assert_eq!(
        folder
            .snapshot()
            .context(HotFolderSnafu)?
            .candidates()
            .map(PhotoCandidate::photo_id)
            .collect::<Vec<_>>(),
        actual
    );
    folder.abandon_group(group).await.context(HotFolderSnafu)?;
    folder.shutdown().await.context(HotFolderSnafu)?;
    Ok(())
}

#[tokio::test]
async fn fallback_time_comes_from_the_source_version_not_the_archive_creation() -> Result {
    use crate::capture_session::PhotoTimeSource;
    use image::ImageEncoder as _;
    use std::os::windows::fs::OpenOptionsExt as _;
    let fixture = fixture().await?;
    let folder = CaptureSession::prepare(&fixture.workspace, &fixture.hot)
        .await
        .context(HotFolderSnafu)?;
    folder.select_team(TEAM_A).await.context(HotFolderSnafu)?;
    let group = folder.start_group().await.context(HotFolderSnafu)?;
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(0)
        .open(fixture.hot.join("no-exif.png"))
        .context(IoSnafu)?;
    image::codecs::png::PngEncoder::new(&file)
        .write_image(&[0; 12], 2, 2, image::ExtendedColorType::Rgb8)
        .context(ImageSnafu)?;
    let modified = std::time::UNIX_EPOCH + Duration::from_secs(1_000);
    file.set_times(fs::FileTimes::new().set_modified(modified))
        .context(IoSnafu)?;
    drop(file);
    let state = wait(&folder, |s| {
        s.candidates().count() == 1 && s.pending_tasks == 0
    })
    .await?;
    let candidate = state.candidates().next().context(FixtureSnafu)?;
    assert_eq!(candidate.captured_at().instant(), modified);
    assert_eq!(
        candidate.captured_at().source(),
        PhotoTimeSource::FileModified
    );
    assert_eq!(state.photos[0].modified, modified);
    folder.abandon_group(group).await.context(HotFolderSnafu)?;
    folder.shutdown().await.context(HotFolderSnafu)?;
    Ok(())
}
type Result<T = ()> = std::result::Result<T, Error>;

#[path = "presentation_session.rs"]
mod presentation_tests;

struct Fixture {
    _directory: tempfile::TempDir,
    workspace: Workspace,
    hot: PathBuf,
}

async fn fixture() -> Result<Fixture> {
    let directory = tempfile::tempdir().context(IoSnafu)?;
    let roster = Roster::parse(include_bytes!("../fixtures/roster.json")).context(RosterSnafu)?;
    let workspace = crate::workspace::fixture::create(&directory.path().join("工作区"), &roster)
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

#[tokio::test]
async fn awaited_commands_publish_their_snapshot_and_capture_keeps_workspace_ownership() -> Result {
    let fixture = fixture().await?;
    let root = fixture.workspace.root().to_owned();
    let folder = CaptureSession::prepare(&fixture.workspace, &fixture.hot)
        .await
        .context(HotFolderSnafu)?;
    folder.select_team(TEAM_A).await.context(HotFolderSnafu)?;
    let group = folder.start_group().await.context(HotFolderSnafu)?;
    assert_eq!(
        folder.snapshot().context(HotFolderSnafu)?.group_id,
        Some(group)
    );
    folder.abandon_group(group).await.context(HotFolderSnafu)?;
    assert!(
        folder
            .snapshot()
            .context(HotFolderSnafu)?
            .group_id
            .is_none()
    );
    drop(fixture.workspace);
    assert!(matches!(
        Workspace::open(&root).await,
        Err(crate::workspace::Error::Storage {
            source: crate::workspace::StorageError::Busy
        })
    ));
    folder.shutdown().await.context(HotFolderSnafu)?;
    drop(Workspace::open(&root).await.context(WorkspaceSnafu)?);
    Ok(())
}

async fn wait(
    folder: &CaptureSession,
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
        CaptureSession::prepare(&fixture.workspace, &fixture.hot).await,
        Err(HotFolderError::NotEmpty)
    ));
    assert_eq!(fs::read(&path).context(IoSnafu)?, b"must remain");
    fs::remove_file(&path).context(IoSnafu)?;
    fs::create_dir(&path).context(IoSnafu)?;
    assert!(matches!(
        CaptureSession::prepare(&fixture.workspace, &fixture.hot).await,
        Err(HotFolderError::NotEmpty)
    ));
    assert!(path.is_dir());
    let nested = fixture.workspace.root().join("empty-hot");
    fs::create_dir(&nested).context(IoSnafu)?;
    assert!(matches!(
        CaptureSession::prepare(&fixture.workspace, &nested).await,
        Err(HotFolderError::UnsafePath)
    ));
    let ancestor = fixture
        .workspace
        .root()
        .parent()
        .context(FixtureSnafu)?
        .to_owned();
    assert!(matches!(
        CaptureSession::prepare(&fixture.workspace, &ancestor).await,
        Err(HotFolderError::UnsafePath)
    ));
    assert!(matches!(
        CaptureSession::prepare(&fixture.workspace, std::path::Path::new("relative")).await,
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
    let folder = CaptureSession::prepare(&fixture.workspace, &fixture.hot)
        .await
        .context(HotFolderSnafu)?;
    folder.select_team(TEAM_A).await.context(HotFolderSnafu)?;
    let group = folder.start_group().await.context(HotFolderSnafu)?;
    let path = fixture.hot.join("真实照片.PNG");
    image::RgbImage::new(8, 6).save(&path).context(ImageSnafu)?;
    let state = wait(&folder, |state| {
        state.photos.len() == 1 && state.photo_checks.len() == 1
    })
    .await?;
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
    assert_eq!(state.photo_checks[0].photo_id(), photo.id);
    let candidate = state.candidates().next().context(FixtureSnafu)?;
    assert_eq!(candidate.photo_id(), photo.id);
    assert!(candidate.image().path().is_file());
    assert!(candidate.thumbnail().path().is_file());
    assert!(
        candidate
            .image()
            .path()
            .starts_with(fixture.workspace.root())
    );
    assert_eq!(
        state.photo_checks[0]
            .result()
            .map(crate::capture_session::PhotoCandidate::source_dimensions)
            .ok(),
        Some(crate::capture_session::PhotoDimensions {
            width: 8,
            height: 6,
            format: crate::capture_session::PhotoFormat::Png,
        })
    );
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

#[tokio::test]
async fn invalid_photos_do_not_block_valid_decodes_and_checks_are_scoped_to_the_active_group()
-> Result {
    use crate::capture_session::PhotoValidationError;

    let fixture = fixture().await?;
    let folder = CaptureSession::prepare(&fixture.workspace, &fixture.hot)
        .await
        .context(HotFolderSnafu)?;
    folder.select_team(TEAM_A).await.context(HotFolderSnafu)?;
    let group_a = folder.start_group().await.context(HotFolderSnafu)?;
    let bad = fixture.hot.join("broken.jpg");
    fs::write(&bad, b"not a real JPEG").context(IoSnafu)?;
    let good = fixture.hot.join("valid.png");
    image::RgbImage::new(4, 3).save(&good).context(ImageSnafu)?;
    let state = wait(&folder, |state| {
        state.photo_checks.len() == 2 && state.pending_tasks == 0
    })
    .await?;
    assert!(!state.paused);
    assert_eq!(state.candidates().count(), 1);
    assert_eq!(
        state
            .photo_checks
            .iter()
            .filter(|check| check.result().is_ok())
            .count(),
        1
    );
    assert_eq!(
        state
            .photo_checks
            .iter()
            .filter(|check| matches!(check.result(), Err(PhotoValidationError::Format)))
            .count(),
        1
    );
    assert!(
        state
            .photos
            .iter()
            .all(|photo| photo.group_id == Some(group_a))
    );
    folder
        .abandon_group(group_a)
        .await
        .context(HotFolderSnafu)?;
    assert!(
        folder
            .snapshot()
            .context(HotFolderSnafu)?
            .photo_checks
            .is_empty()
    );
    folder.select_team(TEAM_B).await.context(HotFolderSnafu)?;
    let group_b = folder.start_group().await.context(HotFolderSnafu)?;
    assert!(
        folder
            .snapshot()
            .context(HotFolderSnafu)?
            .photo_checks
            .is_empty()
    );
    let next = fixture.hot.join("next.png");
    image::RgbImage::new(2, 2).save(&next).context(ImageSnafu)?;
    let state = wait(&folder, |state| {
        state.photo_checks.len() == 1 && state.pending_tasks == 0
    })
    .await?;
    let check = &state.photo_checks[0];
    assert!(check.result().is_ok());
    assert_eq!(
        state
            .photos
            .iter()
            .find(|photo| photo.id == check.photo_id())
            .context(FixtureSnafu)?
            .group_id,
        Some(group_b)
    );
    assert_eq!(fs::read(&bad).context(IoSnafu)?, b"not a real JPEG");
    assert!(good.is_file() && next.is_file());
    folder
        .abandon_group(group_b)
        .await
        .context(HotFolderSnafu)?;
    folder.shutdown().await.context(HotFolderSnafu)?;
    Ok(())
}

#[cfg(windows)]
#[tokio::test]
async fn segmented_windows_write_blocks_group_closure_until_writer_closes() -> Result {
    use std::os::windows::fs::OpenOptionsExt as _;
    let fixture = fixture().await?;
    let folder = CaptureSession::prepare(&fixture.workspace, &fixture.hot)
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
    let folder = CaptureSession::prepare(&fixture.workspace, &fixture.hot)
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
    let folder = CaptureSession::prepare(&fixture.workspace, &fixture.hot)
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
    let folder = CaptureSession::prepare(&fixture.workspace, &fixture.hot)
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
    let folder = CaptureSession::prepare(&fixture.workspace, &fixture.hot)
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
        CaptureSession::prepare(&fixture.workspace, &fixture.hot).await,
        Err(HotFolderError::NotEmpty)
    ));
    assert!(path.is_file());
    assert!(state.photos[0].path.is_file());
    let fresh_hot = fixture.hot.with_extension("new-empty");
    fs::create_dir(&fresh_hot).context(IoSnafu)?;
    let fresh = CaptureSession::prepare(&fixture.workspace, &fresh_hot)
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
    let folder = CaptureSession::prepare(&fixture.workspace, &fixture.hot)
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
        let folder = CaptureSession::prepare(&fixture.workspace, &fixture.hot)
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
