use super::*;
use snafu::{OptionExt as _, Snafu};
use std::{fs, os::windows::fs::OpenOptionsExt as _, sync::Arc};

#[derive(Debug, Snafu)]
enum TestError {
    Io { source: std::io::Error },
    Json { source: serde_json::Error },
    Image { source: image::ImageError },
    Roster { source: crate::roster::Error },
    Workspace { source: Error },
    Fixture,
}
type TestResult = std::result::Result<(), TestError>;

#[tokio::test]
async fn completed_publication_consumes_verified_reader_without_reading_logos_again() -> TestResult
{
    let directory = tempfile::tempdir().context(IoSnafu)?;
    let source = directory.path().join("school.png");
    image::RgbaImage::from_pixel(4, 2, image::Rgba([255, 0, 0, 255]))
        .save(&source)
        .context(ImageSnafu)?;
    let roster = Roster::parse(include_bytes!("../fixtures/roster.json")).context(RosterSnafu)?;
    let resources = directory.path().join("resources.json");
    fs::write(&resources, serde_json::to_vec(&serde_json::json!({
        "school_logos": [{"organization_id": roster.teams()[0].organization_id, "path": "school.png"}],
    })).context(JsonSnafu)?).context(IoSnafu)?;
    let staging = directory.path().join("staged");
    let workspace = Workspace::create_with_resources(&staging, &roster, &resources)
        .await
        .context(WorkspaceSnafu)?;
    let logo = workspace
        .logo_path(&roster.teams()[0].account_id)
        .await
        .context(WorkspaceSnafu)?
        .context(FixtureSnafu)?;
    drop(workspace);
    let verified = WorkspaceReader::inspect(&staging)
        .await
        .context(WorkspaceSnafu)?;
    // A resumed, already-published workspace needs ownership handoff, not another image read.
    let locked_logo = fs::OpenOptions::new()
        .read(true)
        .share_mode(4)
        .open(&logo)
        .context(IoSnafu)?;
    assert!(fs::read(&logo).is_err());
    let initialization_lease =
        Arc::new(fs::File::create(directory.path().join("initialization.lock")).context(IoSnafu)?);
    let target = staging.clone();
    let published = Workspace::publish_verified(verified, target.clone(), initialization_lease)
        .await
        .context(WorkspaceSnafu)?;
    assert_eq!(
        published.root(),
        fs::canonicalize(&target).context(IoSnafu)?
    );
    assert_eq!(
        published
            .roster()
            .await
            .context(WorkspaceSnafu)?
            .teams()
            .len(),
        2
    );
    assert!(matches!(
        Workspace::open(&target).await,
        Err(Error::Storage {
            source: StorageError::Busy
        })
    ));
    drop(locked_logo);
    assert!(
        published
            .logo_path(&roster.teams()[0].account_id)
            .await
            .context(WorkspaceSnafu)?
            .is_some()
    );
    Ok(())
}

#[tokio::test]
async fn publication_rejects_active_writers_and_preserves_existing_targets() -> TestResult {
    let directory = tempfile::tempdir().context(IoSnafu)?;
    let roster = Roster::parse(include_bytes!("../fixtures/roster.json")).context(RosterSnafu)?;
    let source = directory.path().join("staged");
    let target = directory.path().join("published");
    let workspace = Workspace::create(&source, &roster)
        .await
        .context(WorkspaceSnafu)?;
    let lease =
        Arc::new(fs::File::create(directory.path().join("initialization.lock")).context(IoSnafu)?);
    let verified = WorkspaceReader::inspect(&source)
        .await
        .context(WorkspaceSnafu)?;
    assert!(matches!(
        Workspace::publish_verified(verified, target.clone(), Arc::clone(&lease)).await,
        Err(Error::Storage {
            source: StorageError::Busy
        })
    ));
    assert!(!target.exists());
    drop(workspace);
    fs::create_dir(&target).context(IoSnafu)?;
    fs::write(target.join("keep.txt"), b"untouched").context(IoSnafu)?;
    let verified = WorkspaceReader::inspect(&source)
        .await
        .context(WorkspaceSnafu)?;
    assert!(matches!(
        Workspace::publish_verified(verified, target.clone(), lease).await,
        Err(Error::Storage {
            source: StorageError::Exists
        })
    ));
    assert_eq!(
        fs::read(target.join("keep.txt")).context(IoSnafu)?,
        b"untouched"
    );
    assert!(source.exists());
    drop(Workspace::open(&source).await.context(WorkspaceSnafu)?);
    Ok(())
}
