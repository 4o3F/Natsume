use super::*;

#[tokio::test]
async fn pending_ingestion_blocks_retirement_until_the_confirmed_group_drains() -> Result {
    use std::os::windows::fs::OpenOptionsExt as _;
    let fixture = fixture().await?;
    let (folder, group) = prepare_team(&fixture).await?;
    let confirm = intent(&folder, &fixture.hot, "first.png", b"2026:10:31 10:00:01").await?;
    let candidate = confirm.request().image().path().to_owned();
    folder
        .confirm_photo(confirm)
        .await
        .context(HotFolderSnafu)?;
    let late = fixture.hot.join("late.png");
    write_timed_png(&late, b"2026:10:31 10:00:02")?;
    let writing = fs::OpenOptions::new()
        .write(true)
        .share_mode(0)
        .open(&late)
        .context(IoSnafu)?;
    wait(&folder, |s| s.pending_tasks > 0).await?;
    assert!(matches!(
        folder.finish_group(group).await,
        Err(HotFolderError::Capture {
            source: crate::capture::Error::PendingTasks { .. }
        })
    ));
    assert!(candidate.is_file());
    assert_eq!(
        folder.snapshot().context(HotFolderSnafu)?.group_id,
        Some(group)
    );
    drop(writing);
    wait(&folder, |s| {
        s.pending_tasks == 0 && s.candidates().count() == 2
    })
    .await?;
    let report = folder.finish_group(group).await.context(HotFolderSnafu)?;
    assert!(report.failures.is_empty(), "{report:?}");
    assert!(!candidate.exists());
    folder.shutdown().await.context(HotFolderSnafu)?;
    Ok(())
}

#[tokio::test]
async fn unreferenced_complete_final_is_reconciled_and_a_later_confirmation_republishes_it()
-> Result {
    let fixture = fixture().await?;
    let folder = CaptureSession::prepare(&fixture.workspace, &fixture.hot)
        .await
        .context(HotFolderSnafu)?;
    let old = first_final(&fixture, &folder).await?;
    let group = folder.start_group().await.context(HotFolderSnafu)?;
    image::RgbImage::from_pixel(8, 6, image::Rgb([90, 100, 110]))
        .save(fixture.hot.join("new.png"))
        .context(ImageSnafu)?;
    let state = wait(&folder, |s| {
        s.candidates().count() == 1 && s.pending_tasks == 0
    })
    .await?;
    let request = frame(&state)?;
    let orphan = fixture
        .workspace
        .root()
        .join(format!("finals/{}.png", request.image().sha256()));
    fs::copy(request.image().path(), &orphan).context(IoSnafu)?;
    let report = folder.retry_cleanup().await.context(HotFolderSnafu)?;
    assert!(report.failures.is_empty(), "{report:?}");
    assert!(!orphan.exists());
    assert!(request.image().path().is_file());
    assert!(fixture.workspace.root().join(old.path()).is_file());
    folder
        .acknowledge_presentation(receipt(&request))
        .await
        .context(HotFolderSnafu)?;
    let confirm = folder
        .prepare_confirmation(request)
        .await
        .context(HotFolderSnafu)?;
    let saved = folder
        .confirm_photo(confirm)
        .await
        .context(HotFolderSnafu)?;
    assert_eq!(fixture.workspace.root().join(saved.path()), orphan);
    assert!(orphan.is_file());
    assert!(
        folder
            .finish_group(group)
            .await
            .context(HotFolderSnafu)?
            .failures
            .is_empty()
    );
    assert!(!fixture.workspace.root().join(old.path()).exists());
    folder.shutdown().await.context(HotFolderSnafu)?;
    Ok(())
}

#[tokio::test]
async fn queued_source_deletion_rechecks_references_when_the_next_team_reuses_the_archive() -> Result
{
    use std::os::windows::fs::OpenOptionsExt as _;
    let fixture = fixture().await?;
    let (folder, group) = prepare_team(&fixture).await?;
    let confirm = intent(&folder, &fixture.hot, "one.png", b"2026:10:31 10:00:01").await?;
    let archive = folder.snapshot().context(HotFolderSnafu)?.photos[0]
        .path
        .clone();
    let held = fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(&archive)
        .context(IoSnafu)?;
    folder
        .confirm_photo(confirm)
        .await
        .context(HotFolderSnafu)?;
    assert!(
        folder
            .finish_group(group)
            .await
            .context(HotFolderSnafu)?
            .pending_paths
            .contains(&archive)
    );
    folder.select_team(TEAM_B).await.context(HotFolderSnafu)?;
    let b = folder.start_group().await.context(HotFolderSnafu)?;
    fs::copy(
        fixture.hot.join("one.png"),
        fixture.hot.join("same-source.png"),
    )
    .context(IoSnafu)?;
    let state = wait(&folder, |s| {
        s.candidates().count() == 1 && s.pending_tasks == 0
    })
    .await?;
    assert_eq!(state.photos[0].path, archive);
    drop(held);
    let report = folder.retry_cleanup().await.context(HotFolderSnafu)?;
    assert!(report.failures.is_empty(), "{report:?}");
    assert!(report.pending_paths.contains(&archive));
    assert!(archive.is_file());
    let request = frame(&state)?;
    folder
        .acknowledge_presentation(receipt(&request))
        .await
        .context(HotFolderSnafu)?;
    let confirm = folder
        .prepare_confirmation(request)
        .await
        .context(HotFolderSnafu)?;
    folder
        .confirm_photo(confirm)
        .await
        .context(HotFolderSnafu)?;
    let report = folder.finish_group(b).await.context(HotFolderSnafu)?;
    assert!(report.failures.is_empty(), "{report:?}");
    assert!(report.pending_paths.is_empty());
    assert!(!archive.exists());
    folder.shutdown().await.context(HotFolderSnafu)?;
    Ok(())
}

async fn prepare_team(fixture: &Fixture) -> Result<(CaptureSession, crate::capture::GroupId)> {
    let folder = CaptureSession::prepare(&fixture.workspace, &fixture.hot)
        .await
        .context(HotFolderSnafu)?;
    folder.select_team(TEAM_A).await.context(HotFolderSnafu)?;
    let group = folder.start_group().await.context(HotFolderSnafu)?;
    folder.attach_display().await.context(HotFolderSnafu)?;
    Ok((folder, group))
}

#[tokio::test]
async fn finishing_removes_group_copies_but_preserves_originals_finals_and_foreign_files() -> Result
{
    let fixture = fixture().await?;
    let (folder, group) = prepare_team(&fixture).await?;
    intent(&folder, &fixture.hot, "one.png", b"2026:10:31 10:00:01").await?;
    let confirm = intent(&folder, &fixture.hot, "two.png", b"2026:10:31 10:00:02").await?;
    let state = folder.snapshot().context(HotFolderSnafu)?;
    let paths: Vec<_> = state
        .candidates()
        .flat_map(|c| [c.image().path().to_owned(), c.thumbnail().path().to_owned()])
        .chain(state.photos.iter().map(|p| p.path.clone()))
        .collect();
    let originals = [
        fs::read(fixture.hot.join("one.png")).context(IoSnafu)?,
        fs::read(fixture.hot.join("two.png")).context(IoSnafu)?,
    ];
    let saved = folder
        .confirm_photo(confirm)
        .await
        .context(HotFolderSnafu)?;
    let foreign = fixture.workspace.root().join("finals/readme.txt");
    fs::write(&foreign, b"leave this alone").context(IoSnafu)?;
    let report = folder.finish_group(group).await.context(HotFolderSnafu)?;
    assert!(report.failures.is_empty(), "{report:?}");
    assert!(report.pending_paths.is_empty());
    assert!(report.removed_files >= 6);
    assert!(paths.iter().all(|path| !path.exists()));
    assert!(fixture.workspace.root().join(saved.path()).is_file());
    assert_eq!(fs::read(&foreign).context(IoSnafu)?, b"leave this alone");
    assert_eq!(
        fs::read(fixture.hot.join("one.png")).context(IoSnafu)?,
        originals[0]
    );
    assert_eq!(
        fs::read(fixture.hot.join("two.png")).context(IoSnafu)?,
        originals[1]
    );
    folder.select_team(TEAM_B).await.context(HotFolderSnafu)?;
    let b = folder.start_group().await.context(HotFolderSnafu)?;
    folder
        .request(crate::capture_session::Action::Scan)
        .await
        .context(HotFolderSnafu)?;
    assert_eq!(
        folder
            .snapshot()
            .context(HotFolderSnafu)?
            .candidates()
            .count(),
        0
    );
    folder.abandon_group(b).await.context(HotFolderSnafu)?;
    folder.shutdown().await.context(HotFolderSnafu)?;
    Ok(())
}

#[tokio::test]
async fn a_preview_lease_defers_deletion_without_undoing_confirmation_or_blocking_the_next_team()
-> Result {
    let fixture = fixture().await?;
    let (folder, group) = prepare_team(&fixture).await?;
    let confirm = intent(&folder, &fixture.hot, "one.png", b"2026:10:31 10:00:01").await?;
    let path = confirm.request().image().path().to_owned();
    let lease = confirm.request().image().open().context(CandidateSnafu)?;
    let saved = folder
        .confirm_photo(confirm)
        .await
        .context(HotFolderSnafu)?;
    let report = folder.finish_group(group).await.context(HotFolderSnafu)?;
    assert!(!report.failures.is_empty());
    assert!(report.pending_paths.contains(&path));
    let state = folder.snapshot().context(HotFolderSnafu)?;
    assert!(state.group_id.is_none());
    assert!(state.cleanup_pending > 0 && state.cleanup_failures > 0);
    assert_eq!(
        fixture
            .workspace
            .confirmed_photos()
            .await
            .context(WorkspaceSnafu)?,
        vec![saved]
    );
    folder.select_team(TEAM_B).await.context(HotFolderSnafu)?;
    let b = folder.start_group().await.context(HotFolderSnafu)?;
    drop(lease);
    let report = folder.retry_cleanup().await.context(HotFolderSnafu)?;
    assert!(report.failures.is_empty(), "{report:?}");
    assert!(report.pending_paths.is_empty());
    assert!(!path.exists());
    folder.abandon_group(b).await.context(HotFolderSnafu)?;
    folder.shutdown().await.context(HotFolderSnafu)?;
    Ok(())
}

#[tokio::test]
async fn retry_does_not_delete_an_unconfirmed_groups_files_or_unknown_directory_contents() -> Result
{
    let fixture = fixture().await?;
    let (folder, group) = prepare_team(&fixture).await?;
    let confirm = intent(&folder, &fixture.hot, "one.png", b"2026:10:31 10:00:01").await?;
    let image = confirm.request().image().path().to_owned();
    let foreign = image.parent().context(FixtureSnafu)?.join("foreign.txt");
    fs::write(&foreign, b"unknown").context(IoSnafu)?;
    let report = folder.retry_cleanup().await.context(HotFolderSnafu)?;
    assert!(report.failures.is_empty());
    assert!(image.is_file());
    assert!(folder.finish_group(group).await.is_err());
    assert!(image.is_file());
    folder
        .confirm_photo(confirm)
        .await
        .context(HotFolderSnafu)?;
    let report = folder.finish_group(group).await.context(HotFolderSnafu)?;
    assert!(!report.failures.is_empty());
    assert!(!image.exists());
    assert_eq!(fs::read(&foreign).context(IoSnafu)?, b"unknown");
    fs::remove_file(&foreign).context(IoSnafu)?;
    assert!(
        folder
            .retry_cleanup()
            .await
            .context(HotFolderSnafu)?
            .failures
            .is_empty()
    );
    folder.shutdown().await.context(HotFolderSnafu)?;
    Ok(())
}

#[tokio::test]
async fn retakes_preserve_other_teams_shared_finals_and_an_open_export_handle() -> Result {
    use std::os::windows::fs::OpenOptionsExt as _;
    let fixture = fixture().await?;
    let (folder, group) = prepare_team(&fixture).await?;
    let confirm = intent(&folder, &fixture.hot, "a.png", b"2026:10:31 10:00:01").await?;
    let first = folder
        .confirm_photo(confirm)
        .await
        .context(HotFolderSnafu)?;
    folder.finish_group(group).await.context(HotFolderSnafu)?;
    folder.select_team(TEAM_B).await.context(HotFolderSnafu)?;
    let b = folder.start_group().await.context(HotFolderSnafu)?;
    let confirm = intent(&folder, &fixture.hot, "b.png", b"2026:10:31 10:00:02").await?;
    let second = folder
        .confirm_photo(confirm)
        .await
        .context(HotFolderSnafu)?;
    assert_eq!(first.path(), second.path());
    folder.finish_group(b).await.context(HotFolderSnafu)?;
    let old_path = fixture.workspace.root().join(first.path());
    let export_handle = fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(&old_path)
        .context(IoSnafu)?;
    for (account, filename) in [(TEAM_A, "retake-a.png"), (TEAM_B, "retake-b.png")] {
        folder.select_team(account).await.context(HotFolderSnafu)?;
        let retake = folder.start_group().await.context(HotFolderSnafu)?;
        image::RgbImage::from_pixel(8, 6, image::Rgb([80, 90, 100]))
            .save(fixture.hot.join(filename))
            .context(ImageSnafu)?;
        let state = wait(&folder, |s| {
            s.pending_tasks == 0 && s.candidates().count() == 1
        })
        .await?;
        let request = frame(&state)?;
        folder
            .acknowledge_presentation(receipt(&request))
            .await
            .context(HotFolderSnafu)?;
        let confirm = folder
            .prepare_confirmation(request)
            .await
            .context(HotFolderSnafu)?;
        folder
            .confirm_photo(confirm)
            .await
            .context(HotFolderSnafu)?;
        let report = folder.finish_group(retake).await.context(HotFolderSnafu)?;
        assert!(old_path.is_file());
        if account == TEAM_A {
            assert!(report.failures.is_empty(), "{report:?}");
        } else {
            assert!(report.pending_paths.contains(&old_path));
        }
    }
    drop(export_handle);
    let report = folder.retry_cleanup().await.context(HotFolderSnafu)?;
    assert!(report.failures.is_empty(), "{report:?}");
    assert!(!old_path.exists());
    assert_eq!(
        fixture
            .workspace
            .confirmed_photos()
            .await
            .context(WorkspaceSnafu)?
            .len(),
        2
    );
    folder.shutdown().await.context(HotFolderSnafu)?;
    Ok(())
}

#[tokio::test]
async fn corrupted_current_final_blocks_reconciliation_without_deleting_any_retired_copies()
-> Result {
    let fixture = fixture().await?;
    let (folder, group) = prepare_team(&fixture).await?;
    let confirm = intent(&folder, &fixture.hot, "a.png", b"2026:10:31 10:00:01").await?;
    let candidate = confirm.request().image().path().to_owned();
    let saved = folder
        .confirm_photo(confirm)
        .await
        .context(HotFolderSnafu)?;
    let final_path = fixture.workspace.root().join(saved.path());
    fs::write(&final_path, b"damaged").context(IoSnafu)?;
    let report = folder.finish_group(group).await.context(HotFolderSnafu)?;
    assert!(matches!(
        report.failures.first(),
        Some(crate::capture_session::CleanupError::Finals { .. })
    ));
    assert!(candidate.is_file());
    // Restore only this test's known exact file, then retry the queued cleanup.
    fs::copy(&candidate, &final_path).context(IoSnafu)?;
    assert!(
        folder
            .retry_cleanup()
            .await
            .context(HotFolderSnafu)?
            .failures
            .is_empty()
    );
    assert!(!candidate.exists());
    assert!(final_path.is_file());
    folder.shutdown().await.context(HotFolderSnafu)?;
    Ok(())
}
