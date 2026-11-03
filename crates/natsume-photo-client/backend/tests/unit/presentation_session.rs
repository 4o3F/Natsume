use super::*;
use crate::capture_session::{
    PresentationError, PresentationReceipt, PresentationRequest, PresentationStatus,
};

#[path = "confirmed_session.rs"]
mod confirmed_tests;

fn frame(snapshot: &HotFolderSnapshot) -> Result<PresentationRequest> {
    match &snapshot.presentation {
        PresentationStatus::Pending(request) | PresentationStatus::Presented(request) => {
            Ok(request.clone())
        }
        _ => FixtureSnafu.fail(),
    }
}

// Simulate the future renderer callback only in tests; no production shortcut
// claims an image was actually displayed by the current headless client.
fn receipt(request: &PresentationRequest) -> PresentationReceipt {
    PresentationReceipt {
        display_id: request.display_id(),
        revision: request.revision(),
        group_id: request.group_id(),
        account_id: request.account_id().to_owned(),
        photo_id: request.photo_id(),
        image_sha256: request.image().sha256().to_owned(),
    }
}

#[tokio::test]
async fn receipt_is_required_and_confirmation_preparation_does_not_save_or_confirm() -> Result {
    let fixture = fixture().await?;
    let database = fixture.workspace.root().join("workspace.sqlite3");
    let before = fs::read(&database).context(IoSnafu)?;
    let folder = CaptureSession::prepare(&fixture.workspace, &fixture.hot)
        .await
        .context(HotFolderSnafu)?;
    assert_eq!(
        folder.snapshot().context(HotFolderSnafu)?.presentation,
        PresentationStatus::Unavailable
    );
    let display = folder.attach_display().await.context(HotFolderSnafu)?;
    assert_eq!(
        folder.snapshot().context(HotFolderSnafu)?.presentation,
        PresentationStatus::Blank {
            display_id: display
        }
    );
    folder.select_team(TEAM_A).await.context(HotFolderSnafu)?;
    let group = folder.start_group().await.context(HotFolderSnafu)?;
    write_timed_png(&fixture.hot.join("one.png"), b"2026:10:31 10:00:00")?;
    let state = wait(&folder, |s| {
        s.candidates().count() == 1 && s.pending_tasks == 0
    })
    .await?;
    let request = frame(&state)?;
    assert_eq!(request.group_id(), group);
    assert_eq!(request.account_id(), TEAM_A);
    assert_eq!(
        request.image().sha256(),
        state
            .candidates()
            .next()
            .context(FixtureSnafu)?
            .image()
            .sha256()
    );
    assert!(matches!(
        folder.prepare_confirmation(request.clone()).await,
        Err(HotFolderError::Presentation {
            source: PresentationError::NotPresented
        })
    ));
    folder
        .acknowledge_presentation(receipt(&request))
        .await
        .context(HotFolderSnafu)?;
    folder
        .acknowledge_presentation(receipt(&request))
        .await
        .context(HotFolderSnafu)?;
    assert_eq!(
        folder.snapshot().context(HotFolderSnafu)?.presentation,
        PresentationStatus::Presented(request.clone())
    );
    let intent = folder
        .prepare_confirmation(request.clone())
        .await
        .context(HotFolderSnafu)?;
    assert_eq!(intent.request(), &request);
    assert_eq!(fs::read(&database).context(IoSnafu)?, before);
    // This is still an unconfirmed group. An intent neither locks it nor marks it saved.
    folder.abandon_group(group).await.context(HotFolderSnafu)?;
    assert_eq!(
        folder.snapshot().context(HotFolderSnafu)?.presentation,
        PresentationStatus::Blank {
            display_id: display
        }
    );
    assert_eq!(fs::read(&database).context(IoSnafu)?, before);
    folder.shutdown().await.context(HotFolderSnafu)?;
    Ok(())
}

#[tokio::test]
async fn automatic_and_manual_selection_changes_invalidate_old_receipts() -> Result {
    let fixture = fixture().await?;
    let folder = CaptureSession::prepare(&fixture.workspace, &fixture.hot)
        .await
        .context(HotFolderSnafu)?;
    folder.select_team(TEAM_A).await.context(HotFolderSnafu)?;
    let group = folder.start_group().await.context(HotFolderSnafu)?;
    folder.attach_display().await.context(HotFolderSnafu)?;
    write_timed_png(&fixture.hot.join("one.png"), b"2026:10:31 10:00:01")?;
    let state = wait(&folder, |s| {
        s.candidates().count() == 1 && s.pending_tasks == 0
    })
    .await?;
    let first = frame(&state)?;
    folder
        .acknowledge_presentation(receipt(&first))
        .await
        .context(HotFolderSnafu)?;
    let intent = folder
        .prepare_confirmation(first.clone())
        .await
        .context(HotFolderSnafu)?;
    write_timed_png(&fixture.hot.join("two.png"), b"2026:10:31 10:00:02")?;
    let state = wait(&folder, |s| {
        s.candidates().count() == 2 && s.pending_tasks == 0
    })
    .await?;
    let second = frame(&state)?;
    assert!(second.revision().value() > first.revision().value());
    folder
        .acknowledge_presentation(receipt(&second))
        .await
        .context(HotFolderSnafu)?;
    assert!(matches!(
        folder.acknowledge_presentation(receipt(&first)).await,
        Err(HotFolderError::Presentation {
            source: PresentationError::StaleReceipt
        })
    ));
    assert!(matches!(
        folder.prepare_confirmation(second.clone()).await,
        Err(HotFolderError::Presentation {
            source: PresentationError::NotPresented
        })
    ));
    assert!(matches!(
        folder.prepare_confirmation(intent.request().clone()).await,
        Err(HotFolderError::Presentation {
            source: PresentationError::NotPresented
        })
    ));
    let second = frame(&folder.snapshot().context(HotFolderSnafu)?)?;
    folder
        .acknowledge_presentation(receipt(&second))
        .await
        .context(HotFolderSnafu)?;
    folder
        .select_candidate(group, first.photo_id())
        .await
        .context(HotFolderSnafu)?;
    let restored = frame(&folder.snapshot().context(HotFolderSnafu)?)?;
    assert_eq!(restored.photo_id(), first.photo_id());
    assert!(restored.revision().value() > second.revision().value());
    assert!(matches!(
        folder.acknowledge_presentation(receipt(&first)).await,
        Err(HotFolderError::Presentation {
            source: PresentationError::StaleReceipt
        })
    ));
    let restored = frame(&folder.snapshot().context(HotFolderSnafu)?)?;
    folder
        .acknowledge_presentation(receipt(&restored))
        .await
        .context(HotFolderSnafu)?;
    assert!(folder.prepare_confirmation(restored).await.is_ok());
    folder.abandon_group(group).await.context(HotFolderSnafu)?;
    folder.shutdown().await.context(HotFolderSnafu)?;
    Ok(())
}

#[tokio::test]
async fn display_replacement_rejects_old_callbacks_and_shutdown_clears_presentation() -> Result {
    let fixture = fixture().await?;
    let folder = CaptureSession::prepare(&fixture.workspace, &fixture.hot)
        .await
        .context(HotFolderSnafu)?;
    folder.select_team(TEAM_A).await.context(HotFolderSnafu)?;
    folder.start_group().await.context(HotFolderSnafu)?;
    let old_display = folder.attach_display().await.context(HotFolderSnafu)?;
    write_timed_png(&fixture.hot.join("one.png"), b"2026:10:31 10:00:00")?;
    let state = wait(&folder, |s| {
        s.candidates().count() == 1 && s.pending_tasks == 0
    })
    .await?;
    let old = frame(&state)?;
    folder
        .acknowledge_presentation(receipt(&old))
        .await
        .context(HotFolderSnafu)?;
    folder
        .detach_display(old_display)
        .await
        .context(HotFolderSnafu)?;
    assert_eq!(
        folder.snapshot().context(HotFolderSnafu)?.presentation,
        PresentationStatus::Unavailable
    );
    assert!(matches!(
        folder.prepare_confirmation(old.clone()).await,
        Err(HotFolderError::Presentation {
            source: PresentationError::NotPresented
        })
    ));
    let new_display = folder.attach_display().await.context(HotFolderSnafu)?;
    let current = frame(&folder.snapshot().context(HotFolderSnafu)?)?;
    assert_ne!(new_display, old_display);
    assert!(matches!(
        folder.detach_display(old_display).await,
        Err(HotFolderError::Presentation {
            source: PresentationError::StaleDisplay
        })
    ));
    assert!(matches!(
        folder.acknowledge_presentation(receipt(&old)).await,
        Err(HotFolderError::Presentation {
            source: PresentationError::StaleReceipt
        })
    ));
    assert_eq!(
        folder.snapshot().context(HotFolderSnafu)?.presentation,
        PresentationStatus::Pending(current.clone())
    );
    folder
        .acknowledge_presentation(receipt(&current))
        .await
        .context(HotFolderSnafu)?;
    let observe = folder.snapshots.clone();
    folder.shutdown().await.context(HotFolderSnafu)?;
    assert_eq!(
        observe.borrow().presentation,
        PresentationStatus::Unavailable
    );
    assert_eq!(observe.borrow().state, IngestionState::Stopped);
    Ok(())
}

#[tokio::test]
async fn pause_recovery_and_team_changes_require_new_frame_acknowledgements() -> Result {
    let fixture = fixture().await?;
    let folder = CaptureSession::prepare(&fixture.workspace, &fixture.hot)
        .await
        .context(HotFolderSnafu)?;
    folder.select_team(TEAM_A).await.context(HotFolderSnafu)?;
    let group_a = folder.start_group().await.context(HotFolderSnafu)?;
    folder.attach_display().await.context(HotFolderSnafu)?;
    let source = fixture.hot.join("one.png");
    write_timed_png(&source, b"2026:10:31 10:00:00")?;
    let state = wait(&folder, |s| {
        s.candidates().count() == 1 && s.pending_tasks == 0
    })
    .await?;
    let old = frame(&state)?;
    folder
        .acknowledge_presentation(receipt(&old))
        .await
        .context(HotFolderSnafu)?;
    fs::remove_file(&source).context(IoSnafu)?;
    fs::remove_dir(&fixture.hot).context(IoSnafu)?;
    wait(&folder, |s| {
        s.paused && matches!(s.presentation, PresentationStatus::Blank { .. })
    })
    .await?;
    assert!(matches!(
        folder.prepare_confirmation(old.clone()).await,
        Err(HotFolderError::Presentation {
            source: PresentationError::NotPresented
        })
    ));
    fs::create_dir(&fixture.hot).context(IoSnafu)?;
    let restored = wait(&folder, |s| {
        !s.paused && matches!(s.presentation, PresentationStatus::Pending(_))
    })
    .await?;
    let restored = frame(&restored)?;
    assert!(restored.revision().value() > old.revision().value());
    assert!(matches!(
        folder.acknowledge_presentation(receipt(&old)).await,
        Err(HotFolderError::Presentation {
            source: PresentationError::StaleReceipt
        })
    ));
    let restored = frame(&folder.snapshot().context(HotFolderSnafu)?)?;
    folder
        .acknowledge_presentation(receipt(&restored))
        .await
        .context(HotFolderSnafu)?;
    folder
        .abandon_group(group_a)
        .await
        .context(HotFolderSnafu)?;
    folder.select_team(TEAM_B).await.context(HotFolderSnafu)?;
    let group_b = folder.start_group().await.context(HotFolderSnafu)?;
    assert!(matches!(
        folder.acknowledge_presentation(receipt(&restored)).await,
        Err(HotFolderError::Presentation {
            source: PresentationError::StaleReceipt
        })
    ));
    write_timed_png(&fixture.hot.join("team-b.png"), b"2026:10:31 10:00:01")?;
    let state = wait(&folder, |s| {
        s.candidates().count() == 1 && s.pending_tasks == 0
    })
    .await?;
    let next = frame(&state)?;
    assert_eq!(next.account_id(), TEAM_B);
    assert_eq!(next.group_id(), group_b);
    assert!(matches!(
        folder.prepare_confirmation(restored).await,
        Err(HotFolderError::Presentation {
            source: PresentationError::NotPresented
        })
    ));
    folder
        .abandon_group(group_b)
        .await
        .context(HotFolderSnafu)?;
    folder.shutdown().await.context(HotFolderSnafu)?;
    Ok(())
}
