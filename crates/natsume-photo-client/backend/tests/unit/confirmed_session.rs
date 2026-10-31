use super::*;
use crate::{
    capture_session::{ConfirmationIntent, FinalPhotoError},
    workspace::ConfirmedPhoto,
};
use diesel::{Connection as _, connection::SimpleConnection as _};
use diesel::{ExpressionMethods as _, QueryDsl as _, RunQueryDsl as _};

#[path = "cleanup_session.rs"]
mod cleanup_tests;

#[tokio::test]
async fn shared_result_entries_use_roster_accounts_and_keep_persisted_identity_after_reopen()
-> Result {
    let fixture = fixture().await?;
    let root = fixture.workspace.root().to_owned();
    let folder = CaptureSession::prepare(&fixture.workspace, &fixture.hot)
        .await
        .context(HotFolderSnafu)?;
    let saved = first_final(&fixture, &folder).await?;
    let entries = fixture
        .workspace
        .current_result_entries()
        .await
        .context(WorkspaceSnafu)?;
    assert_eq!(entries.len(), 1);
    let entry = entries.first().context(FixtureSnafu)?;
    let roster = fixture.workspace.roster().await.context(WorkspaceSnafu)?;
    let team = roster
        .teams()
        .iter()
        .find(|team| team.account_id == TEAM_A)
        .context(FixtureSnafu)?;
    assert_eq!(entry.account, team.account);
    assert_eq!(entry.account_id, saved.account_id());
    assert_eq!(&entry.result_id, saved.result_id());
    assert_eq!(&entry.submission_id, saved.submission_id());
    assert_eq!(entry.image_sha256.as_str(), saved.sha256());
    assert_eq!(entry.image_bytes, saved.bytes());
    assert_eq!(
        (entry.width, entry.height, entry.image_format),
        (
            saved.dimensions().width,
            saved.dimensions().height,
            saved.dimensions().format
        )
    );
    folder.shutdown().await.context(HotFolderSnafu)?;
    drop(fixture.workspace);
    let reopened = Workspace::open(&root).await.context(WorkspaceSnafu)?;
    assert_eq!(
        reopened
            .current_result_entries()
            .await
            .context(WorkspaceSnafu)?,
        entries
    );
    Ok(())
}

async fn intent(
    folder: &CaptureSession,
    hot: &std::path::Path,
    filename: &str,
    time: &[u8],
) -> Result<ConfirmationIntent> {
    let path = hot.join(filename);
    write_timed_png(&path, time)?;
    let path = fs::canonicalize(path).context(IoSnafu)?;
    let state = wait(folder, |state| {
        state.pending_tasks == 0
            && state.candidates().any(|candidate| {
                state
                    .photos
                    .iter()
                    .any(|photo| photo.id == candidate.photo_id() && photo.source == path)
            })
    })
    .await?;
    let request = frame(&state)?;
    folder
        .acknowledge_presentation(receipt(&request))
        .await
        .context(HotFolderSnafu)?;
    folder
        .prepare_confirmation(request)
        .await
        .context(HotFolderSnafu)
}

async fn first_final(fixture: &Fixture, folder: &CaptureSession) -> Result<ConfirmedPhoto> {
    folder.select_team(TEAM_A).await.context(HotFolderSnafu)?;
    let group = folder.start_group().await.context(HotFolderSnafu)?;
    folder.attach_display().await.context(HotFolderSnafu)?;
    let confirmed = intent(folder, &fixture.hot, "first.png", b"2026:10:31 10:00:01").await?;
    let saved = folder
        .confirm_photo(confirmed)
        .await
        .context(HotFolderSnafu)?;
    folder.finish_group(group).await.context(HotFolderSnafu)?;
    Ok(saved)
}

#[tokio::test]
async fn complete_confirmation_is_idempotent_freezes_selection_and_survives_reopen() -> Result {
    let fixture = fixture().await?;
    let root = fixture.workspace.root().to_owned();
    let folder = CaptureSession::prepare(&fixture.workspace, &fixture.hot)
        .await
        .context(HotFolderSnafu)?;
    folder.select_team(TEAM_A).await.context(HotFolderSnafu)?;
    let group = folder.start_group().await.context(HotFolderSnafu)?;
    folder.attach_display().await.context(HotFolderSnafu)?;
    let confirm = intent(&folder, &fixture.hot, "one.png", b"2026:10:31 10:00:01").await?;
    assert!(
        fixture
            .workspace
            .current_result_entries()
            .await
            .context(WorkspaceSnafu)?
            .is_empty()
    );
    let saved = folder
        .confirm_photo(confirm.clone())
        .await
        .context(HotFolderSnafu)?;
    assert_eq!(
        folder
            .confirm_photo(confirm.clone())
            .await
            .context(HotFolderSnafu)?,
        saved
    );
    assert_eq!(saved.revision(), 1);
    assert_eq!(saved.account_id(), TEAM_A);
    assert_eq!(
        folder.snapshot().context(HotFolderSnafu)?.confirmed,
        Some(saved.clone())
    );
    assert_eq!(
        fs::read(root.join(saved.path())).context(IoSnafu)?,
        fs::read(confirm.request().image().path()).context(IoSnafu)?
    );
    assert!(matches!(
        folder.use_latest_candidate(group).await,
        Err(HotFolderError::Finalized)
    ));
    assert!(matches!(
        folder
            .select_candidate(group, confirm.request().photo_id())
            .await,
        Err(HotFolderError::Finalized)
    ));
    write_timed_png(&fixture.hot.join("late.png"), b"2026:10:31 10:00:02")?;
    let state = wait(&folder, |s| {
        s.candidates().count() == 2 && s.pending_tasks == 0
    })
    .await?;
    assert_eq!(
        state.selection.photo_id(),
        Some(confirm.request().photo_id())
    );
    assert_eq!(
        fixture
            .workspace
            .confirmed_photos()
            .await
            .context(WorkspaceSnafu)?,
        vec![saved.clone()]
    );
    folder.finish_group(group).await.context(HotFolderSnafu)?;
    folder.shutdown().await.context(HotFolderSnafu)?;
    assert!(!confirm.request().image().path().exists());
    drop(fixture.workspace);
    let reopened = Workspace::open(&root).await.context(WorkspaceSnafu)?;
    assert_eq!(
        reopened.confirmed_photos().await.context(WorkspaceSnafu)?,
        vec![saved]
    );
    Ok(())
}

#[tokio::test]
async fn corrupted_retake_source_keeps_the_previous_final_and_allows_abandonment() -> Result {
    let fixture = fixture().await?;
    let folder = CaptureSession::prepare(&fixture.workspace, &fixture.hot)
        .await
        .context(HotFolderSnafu)?;
    let old = first_final(&fixture, &folder).await?;
    let group = folder.start_group().await.context(HotFolderSnafu)?;
    let confirm = intent(&folder, &fixture.hot, "retake.png", b"2026:10:31 10:00:03").await?;
    fs::write(
        confirm.request().image().path(),
        b"corrupt candidate after presentation",
    )
    .context(IoSnafu)?;
    assert!(matches!(
        folder.confirm_photo(confirm).await,
        Err(HotFolderError::FinalPhoto {
            source: FinalPhotoError::Integrity
        })
    ));
    assert!(
        folder
            .snapshot()
            .context(HotFolderSnafu)?
            .confirmed
            .is_none()
    );
    assert_eq!(
        fixture
            .workspace
            .confirmed_photos()
            .await
            .context(WorkspaceSnafu)?,
        vec![old.clone()]
    );
    assert!(fixture.workspace.root().join(old.path()).is_file());
    folder.abandon_group(group).await.context(HotFolderSnafu)?;
    folder.shutdown().await.context(HotFolderSnafu)?;
    Ok(())
}

#[tokio::test]
async fn database_failure_keeps_the_old_result_and_retry_reuses_complete_files() -> Result {
    let fixture = fixture().await?;
    let folder = CaptureSession::prepare(&fixture.workspace, &fixture.hot)
        .await
        .context(HotFolderSnafu)?;
    let old = first_final(&fixture, &folder).await?;
    let group = folder.start_group().await.context(HotFolderSnafu)?;
    let confirm = intent(&folder, &fixture.hot, "retake.png", b"2026:10:31 10:00:04").await?;
    let path = fixture.workspace.root().join("workspace.sqlite3");
    let mut database = diesel::SqliteConnection::establish(path.to_str().context(FixtureSnafu)?)
        .context(DatabaseSnafu)?;
    // SQL fault injection is deliberately distinct from ordinary fixture CRUD.
    database.batch_execute("CREATE TRIGGER fail_final BEFORE UPDATE ON photo_team_results BEGIN SELECT RAISE(ABORT, 'injected final failure'); END;").context(QuerySnafu)?;
    assert!(matches!(
        folder.confirm_photo(confirm.clone()).await,
        Err(HotFolderError::FinalPhoto {
            source: FinalPhotoError::Persistence { .. }
        })
    ));
    assert_eq!(
        fixture
            .workspace
            .confirmed_photos()
            .await
            .context(WorkspaceSnafu)?,
        vec![old.clone()]
    );
    assert!(
        folder
            .snapshot()
            .context(HotFolderSnafu)?
            .confirmed
            .is_none()
    );
    database
        .batch_execute("DROP TRIGGER fail_final;")
        .context(QuerySnafu)?;
    let saved = folder
        .confirm_photo(confirm)
        .await
        .context(HotFolderSnafu)?;
    assert_eq!(saved.revision(), old.revision() + 1);
    assert_ne!(saved.result_id(), old.result_id());
    assert_ne!(saved.submission_id(), old.submission_id());
    assert_eq!(
        fixture
            .workspace
            .confirmed_photos()
            .await
            .context(WorkspaceSnafu)?,
        vec![saved]
    );
    folder.finish_group(group).await.context(HotFolderSnafu)?;
    folder.shutdown().await.context(HotFolderSnafu)?;
    Ok(())
}

#[tokio::test]
async fn a_cancelled_confirmation_waiter_does_not_cancel_an_accepted_save() -> Result {
    let fixture = fixture().await?;
    let folder = CaptureSession::prepare(&fixture.workspace, &fixture.hot)
        .await
        .context(HotFolderSnafu)?;
    folder.select_team(TEAM_A).await.context(HotFolderSnafu)?;
    let group = folder.start_group().await.context(HotFolderSnafu)?;
    folder.attach_display().await.context(HotFolderSnafu)?;
    let confirm = intent(&folder, &fixture.hot, "one.png", b"2026:10:31 10:00:00").await?;
    let (reply, answer) = tokio::sync::oneshot::channel();
    drop(answer);
    assert!(
        folder
            .commands
            .send(crate::capture_session::Command {
                action: crate::capture_session::Action::Confirm(confirm),
                reply
            })
            .await
            .is_ok()
    );
    let state = wait(&folder, |s| s.confirmed.is_some()).await?;
    let saved = state.confirmed.context(FixtureSnafu)?;
    assert_eq!(
        fixture
            .workspace
            .confirmed_photos()
            .await
            .context(WorkspaceSnafu)?,
        vec![saved]
    );
    folder.finish_group(group).await.context(HotFolderSnafu)?;
    folder.shutdown().await.context(HotFolderSnafu)?;
    Ok(())
}

#[tokio::test]
async fn a_corrupt_confirmed_file_is_rejected_on_readonly_and_writable_reopen() -> Result {
    let fixture = fixture().await?;
    let root = fixture.workspace.root().to_owned();
    let folder = CaptureSession::prepare(&fixture.workspace, &fixture.hot)
        .await
        .context(HotFolderSnafu)?;
    let saved = first_final(&fixture, &folder).await?;
    folder.shutdown().await.context(HotFolderSnafu)?;
    drop(fixture.workspace);
    fs::write(root.join(saved.path()), b"corrupt final").context(IoSnafu)?;
    assert!(matches!(
        Workspace::open(&root).await,
        Err(crate::workspace::Error::FinalPhoto {
            source: FinalPhotoError::Integrity
        })
    ));
    assert!(matches!(
        crate::workspace::WorkspaceReader::inspect(&root).await,
        Err(crate::workspace::Error::FinalPhoto {
            source: FinalPhotoError::Integrity
        })
    ));
    Ok(())
}

#[tokio::test]
async fn failed_final_file_publication_preserves_the_old_result_and_can_be_retried() -> Result {
    let fixture = fixture().await?;
    let folder = CaptureSession::prepare(&fixture.workspace, &fixture.hot)
        .await
        .context(HotFolderSnafu)?;
    let old = first_final(&fixture, &folder).await?;
    let group = folder.start_group().await.context(HotFolderSnafu)?;
    image::RgbImage::from_pixel(8, 6, image::Rgb([100, 120, 140]))
        .save(fixture.hot.join("different.png"))
        .context(ImageSnafu)?;
    let state = wait(&folder, |s| {
        s.candidates().count() == 1 && s.pending_tasks == 0
    })
    .await?;
    let request = frame(&state)?;
    assert_ne!(request.image().sha256(), old.sha256());
    folder
        .acknowledge_presentation(receipt(&request))
        .await
        .context(HotFolderSnafu)?;
    let confirm = folder
        .prepare_confirmation(request.clone())
        .await
        .context(HotFolderSnafu)?;
    let blocked = fixture
        .workspace
        .root()
        .join("finals")
        .join(format!("{}.png", request.image().sha256()));
    fs::create_dir(&blocked).context(IoSnafu)?;
    assert!(matches!(
        folder.confirm_photo(confirm.clone()).await,
        Err(HotFolderError::FinalPhoto { .. })
    ));
    assert_eq!(
        fixture
            .workspace
            .confirmed_photos()
            .await
            .context(WorkspaceSnafu)?,
        vec![old.clone()]
    );
    assert!(blocked.is_dir());
    fs::remove_dir(&blocked).context(IoSnafu)?;
    let saved = folder
        .confirm_photo(confirm)
        .await
        .context(HotFolderSnafu)?;
    assert_eq!(saved.revision(), old.revision() + 1);
    assert_eq!(
        fixture
            .workspace
            .confirmed_photos()
            .await
            .context(WorkspaceSnafu)?,
        vec![saved]
    );
    folder.finish_group(group).await.context(HotFolderSnafu)?;
    folder.shutdown().await.context(HotFolderSnafu)?;
    Ok(())
}

#[tokio::test]
async fn a_changed_selection_rejects_a_previously_prepared_intent_without_writing_a_result()
-> Result {
    let fixture = fixture().await?;
    let folder = CaptureSession::prepare(&fixture.workspace, &fixture.hot)
        .await
        .context(HotFolderSnafu)?;
    folder.select_team(TEAM_A).await.context(HotFolderSnafu)?;
    let group = folder.start_group().await.context(HotFolderSnafu)?;
    folder.attach_display().await.context(HotFolderSnafu)?;
    let old = intent(&folder, &fixture.hot, "one.png", b"2026:10:31 10:00:01").await?;
    write_timed_png(&fixture.hot.join("two.png"), b"2026:10:31 10:00:02")?;
    wait(&folder, |s| {
        s.candidates().count() == 2 && s.pending_tasks == 0
    })
    .await?;
    assert!(matches!(
        folder.confirm_photo(old).await,
        Err(HotFolderError::Presentation {
            source: PresentationError::NotPresented
        })
    ));
    assert!(
        fixture
            .workspace
            .confirmed_photos()
            .await
            .context(WorkspaceSnafu)?
            .is_empty()
    );
    folder.abandon_group(group).await.context(HotFolderSnafu)?;
    folder.shutdown().await.context(HotFolderSnafu)?;
    Ok(())
}

#[tokio::test]
async fn shared_confirmed_images_are_deduplicated_and_inconsistent_references_are_rejected()
-> Result {
    use crate::diesel_schema::photo_team_results;
    let fixture = fixture().await?;
    let folder = CaptureSession::prepare(&fixture.workspace, &fixture.hot)
        .await
        .context(HotFolderSnafu)?;
    let a = first_final(&fixture, &folder).await?;
    folder.select_team(TEAM_B).await.context(HotFolderSnafu)?;
    let group = folder.start_group().await.context(HotFolderSnafu)?;
    let confirm = intent(&folder, &fixture.hot, "team-b.png", b"2026:10:31 10:00:02").await?;
    let b = folder
        .confirm_photo(confirm)
        .await
        .context(HotFolderSnafu)?;
    assert_eq!(a.path(), b.path());
    assert_ne!(a.result_id(), b.result_id());
    assert_eq!(
        fixture
            .workspace
            .confirmed_photos()
            .await
            .context(WorkspaceSnafu)?
            .len(),
        2
    );
    assert_eq!(
        fs::read_dir(fixture.workspace.root().join("finals"))
            .context(IoSnafu)?
            .count(),
        1
    );
    let database = fixture.workspace.root().join("workspace.sqlite3");
    let mut database =
        diesel::SqliteConnection::establish(database.to_str().context(FixtureSnafu)?)
            .context(DatabaseSnafu)?;
    diesel::update(photo_team_results::table.find(TEAM_B))
        .set(photo_team_results::width.eq(9))
        .execute(&mut database)
        .context(QuerySnafu)?;
    assert!(matches!(
        fixture.workspace.confirmed_photos().await,
        Err(crate::workspace::Error::FinalPhoto {
            source: FinalPhotoError::Integrity
        })
    ));
    diesel::update(photo_team_results::table.find(TEAM_B))
        .set((
            photo_team_results::width.eq(8),
            photo_team_results::account_id.eq("unknown-team"),
        ))
        .execute(&mut database)
        .context(QuerySnafu)?;
    assert!(matches!(
        fixture.workspace.confirmed_photos().await,
        Err(crate::workspace::Error::FinalPhoto {
            source: FinalPhotoError::Integrity
        })
    ));
    folder.finish_group(group).await.context(HotFolderSnafu)?;
    folder.shutdown().await.context(HotFolderSnafu)?;
    Ok(())
}
