use super::*;
use crate::{
    capture::Session,
    capture_session::{ArchivedPhoto, PhotoDimensions, PhotoFormat, PhotoTime, candidate},
    roster::Roster,
};
use image::{DynamicImage, metadata::Orientation};
use snafu::{OptionExt as _, ResultExt as _, Snafu};

#[derive(Debug, Snafu)]
enum TestError {
    Io { source: std::io::Error },
    Roster { source: crate::roster::Error },
    Capture { source: crate::capture::Error },
    Candidate { source: candidate::CandidateError },
    Presentation { source: PresentationError },
    Fixture,
}
type TestResult<T = ()> = std::result::Result<T, TestError>;

fn target() -> TestResult<(tempfile::TempDir, PresentationTarget)> {
    let directory = tempfile::tempdir().context(IoSnafu)?;
    let archive = std::fs::canonicalize(directory.path()).context(IoSnafu)?;
    let roster = Roster::parse(include_bytes!("../fixtures/roster.json")).context(RosterSnafu)?;
    let mut session = Session::new(roster);
    session
        .select_team("0199a000-0000-7000-8000-000000000001")
        .context(CaptureSnafu)?;
    let group = session.start_group().context(CaptureSnafu)?;
    let photo = ArchivedPhoto {
        id: Uuid::now_v7(),
        source: archive.join("camera.png"),
        path: archive.join("source"),
        sha256: "0".repeat(64),
        bytes: 0,
        modified: std::time::UNIX_EPOCH,
        group_id: Some(group),
    };
    let candidate = candidate::prepare(
        &photo,
        &archive,
        candidate::DecodedPhoto {
            dimensions: PhotoDimensions {
                width: 2,
                height: 3,
                format: PhotoFormat::Png,
            },
            image: DynamicImage::ImageRgb8(image::RgbImage::new(2, 3)),
            orientation: Orientation::NoTransforms,
            icc: None,
            captured_at: PhotoTime::read(None, std::time::UNIX_EPOCH),
        },
    )
    .context(CandidateSnafu)?;
    Ok((
        directory,
        PresentationTarget {
            group,
            account: session
                .group()
                .context(FixtureSnafu)?
                .account_id()
                .to_owned(),
            photo: candidate.photo_id(),
            image: candidate.image().clone(),
        },
    ))
}

fn request(presentation: &Presentation) -> TestResult<PresentationRequest> {
    match presentation.status() {
        PresentationStatus::Pending(request) | PresentationStatus::Presented(request) => {
            Ok(request)
        }
        _ => FixtureSnafu.fail(),
    }
}
// Renderer simulation belongs only in tests. Production adapters acknowledge
// only after the exact file has actually been rendered in the attached window.
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

#[test]
fn matching_receipt_is_required_and_duplicates_do_not_change_the_frame() -> TestResult {
    let (_directory, target) = target()?;
    let mut presentation = Presentation::new();
    presentation
        .refresh(Some(target.clone()))
        .context(PresentationSnafu)?;
    assert_eq!(presentation.status(), PresentationStatus::Unavailable);
    presentation.attach().context(PresentationSnafu)?;
    presentation
        .refresh(Some(target.clone()))
        .context(PresentationSnafu)?;
    let current = request(&presentation)?;
    assert!(matches!(
        presentation.prepare_confirmation(&current),
        Err(PresentationError::NotPresented)
    ));
    presentation
        .acknowledge(&receipt(&current))
        .context(PresentationSnafu)?;
    presentation
        .acknowledge(&receipt(&current))
        .context(PresentationSnafu)?;
    presentation
        .refresh(Some(target))
        .context(PresentationSnafu)?;
    let intent = presentation
        .prepare_confirmation(&current)
        .context(PresentationSnafu)?;
    assert_eq!(intent.request(), &current);
    assert_eq!(
        presentation.status(),
        PresentationStatus::Presented(current)
    );
    Ok(())
}

#[test]
fn wrong_display_revision_group_team_photo_or_hash_cannot_acknowledge() -> TestResult {
    let (_directory, target) = target()?;
    let (_other, other) = self::target()?;
    for field in 0..6 {
        let mut presentation = Presentation::new();
        presentation.attach().context(PresentationSnafu)?;
        presentation
            .refresh(Some(target.clone()))
            .context(PresentationSnafu)?;
        let current = request(&presentation)?;
        let mut wrong = receipt(&current);
        match field {
            0 => wrong.display_id = DisplayId(Uuid::now_v7()),
            1 => wrong.revision = PresentationRevision(current.revision().value() + 1),
            2 => wrong.group_id = other.group,
            3 => wrong.account_id = "other team".to_owned(),
            4 => wrong.photo_id = Uuid::now_v7(),
            _ => wrong.image_sha256 = "0".repeat(64),
        }
        assert!(matches!(
            presentation.acknowledge(&wrong),
            Err(PresentationError::StaleReceipt)
        ));
        assert!(matches!(
            presentation.prepare_confirmation(&current),
            Err(PresentationError::NotPresented)
        ));
        presentation
            .refresh(Some(target.clone()))
            .context(PresentationSnafu)?;
        let fresh = request(&presentation)?;
        if field != 0 {
            assert!(fresh.revision().value() > current.revision().value());
        }
        presentation
            .acknowledge(&receipt(&fresh))
            .context(PresentationSnafu)?;
        if field == 0 {
            assert!(matches!(
                presentation.acknowledge(&wrong),
                Err(PresentationError::StaleReceipt)
            ));
            assert!(presentation.prepare_confirmation(&fresh).is_ok());
        }
    }
    Ok(())
}

#[test]
fn switching_away_and_back_does_not_reuse_receipts_or_confirmation_intents() -> TestResult {
    let (_directory, target) = target()?;
    let mut presentation = Presentation::new();
    presentation.attach().context(PresentationSnafu)?;
    presentation
        .refresh(Some(target.clone()))
        .context(PresentationSnafu)?;
    let first = request(&presentation)?;
    presentation
        .acknowledge(&receipt(&first))
        .context(PresentationSnafu)?;
    let intent = presentation
        .prepare_confirmation(&first)
        .context(PresentationSnafu)?;
    let mut second = target.clone();
    second.photo = Uuid::now_v7();
    presentation
        .refresh(Some(second))
        .context(PresentationSnafu)?;
    presentation
        .refresh(Some(target.clone()))
        .context(PresentationSnafu)?;
    let latest = request(&presentation)?;
    assert!(latest.revision().value() > first.revision().value());
    assert!(matches!(
        presentation.acknowledge(&receipt(&first)),
        Err(PresentationError::StaleReceipt)
    ));
    presentation
        .refresh(Some(target))
        .context(PresentationSnafu)?;
    let latest = request(&presentation)?;
    presentation
        .acknowledge(&receipt(&latest))
        .context(PresentationSnafu)?;
    assert!(matches!(
        presentation.prepare_confirmation(intent.request()),
        Err(PresentationError::NotPresented)
    ));
    assert!(presentation.prepare_confirmation(&latest).is_ok());
    Ok(())
}

#[test]
fn stale_window_callbacks_cannot_detach_or_acknowledge_a_new_display() -> TestResult {
    let (_directory, target) = target()?;
    let mut presentation = Presentation::new();
    let first_display = presentation.attach().context(PresentationSnafu)?;
    presentation
        .refresh(Some(target.clone()))
        .context(PresentationSnafu)?;
    let old = request(&presentation)?;
    presentation
        .acknowledge(&receipt(&old))
        .context(PresentationSnafu)?;
    assert!(matches!(
        presentation.attach(),
        Err(PresentationError::DisplayAttached)
    ));
    presentation
        .detach(first_display)
        .context(PresentationSnafu)?;
    let second_display = presentation.attach().context(PresentationSnafu)?;
    presentation
        .refresh(Some(target))
        .context(PresentationSnafu)?;
    let current = request(&presentation)?;
    assert_ne!(first_display, second_display);
    assert!(matches!(
        presentation.detach(first_display),
        Err(PresentationError::StaleDisplay)
    ));
    assert!(matches!(
        presentation.acknowledge(&receipt(&old)),
        Err(PresentationError::StaleReceipt)
    ));
    assert_eq!(presentation.status(), PresentationStatus::Pending(current));
    Ok(())
}

#[test]
fn blanking_or_stopping_clears_acknowledgement() -> TestResult {
    let (_directory, target) = target()?;
    let mut presentation = Presentation::new();
    let display = presentation.attach().context(PresentationSnafu)?;
    presentation
        .refresh(Some(target.clone()))
        .context(PresentationSnafu)?;
    let first = request(&presentation)?;
    presentation
        .acknowledge(&receipt(&first))
        .context(PresentationSnafu)?;
    presentation.refresh(None).context(PresentationSnafu)?;
    assert_eq!(
        presentation.status(),
        PresentationStatus::Blank {
            display_id: display
        }
    );
    assert!(matches!(
        presentation.prepare_confirmation(&first),
        Err(PresentationError::NotPresented)
    ));
    presentation
        .refresh(Some(target))
        .context(PresentationSnafu)?;
    assert!(matches!(
        presentation.acknowledge(&receipt(&first)),
        Err(PresentationError::StaleReceipt)
    ));
    presentation.stop();
    assert!(matches!(
        presentation.acknowledge(&receipt(&first)),
        Err(PresentationError::StaleReceipt)
    ));
    Ok(())
}

#[test]
fn exhausted_revisions_fail_closed_without_reusing_an_old_frame() -> TestResult {
    let (_directory, target) = target()?;
    let mut presentation = Presentation::new();
    presentation.attach().context(PresentationSnafu)?;
    presentation
        .refresh(Some(target.clone()))
        .context(PresentationSnafu)?;
    let first = request(&presentation)?;
    presentation
        .acknowledge(&receipt(&first))
        .context(PresentationSnafu)?;
    presentation.revision = u64::MAX;
    let mut changed = target;
    changed.photo = Uuid::now_v7();
    assert!(matches!(
        presentation.refresh(Some(changed)),
        Err(PresentationError::RevisionExhausted)
    ));
    assert_eq!(presentation.status(), PresentationStatus::Unavailable);
    assert!(matches!(
        presentation.prepare_confirmation(&first),
        Err(PresentationError::NotPresented)
    ));
    Ok(())
}

#[test]
fn a_late_frame_claim_from_the_current_window_revokes_confirmation_readiness() -> TestResult {
    let (_directory, target) = target()?;
    let mut presentation = Presentation::new();
    presentation.attach().context(PresentationSnafu)?;
    presentation
        .refresh(Some(target.clone()))
        .context(PresentationSnafu)?;
    let first = request(&presentation)?;
    let mut newer = target;
    newer.photo = Uuid::now_v7();
    presentation
        .refresh(Some(newer.clone()))
        .context(PresentationSnafu)?;
    let current = request(&presentation)?;
    presentation
        .acknowledge(&receipt(&current))
        .context(PresentationSnafu)?;
    // A renderer which painted an older asynchronous load after this frame is
    // indistinguishable from a delayed callback. Fail closed and request a new frame.
    assert!(matches!(
        presentation.acknowledge(&receipt(&first)),
        Err(PresentationError::StaleReceipt)
    ));
    assert!(matches!(
        presentation.prepare_confirmation(&current),
        Err(PresentationError::NotPresented)
    ));
    presentation
        .refresh(Some(newer))
        .context(PresentationSnafu)?;
    let retry = request(&presentation)?;
    assert!(retry.revision().value() > current.revision().value());
    Ok(())
}
