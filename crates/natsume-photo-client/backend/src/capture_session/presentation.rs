//! Binds renderer acknowledgements and operator intent to one exact displayed frame.
use super::{CandidateImage, GroupId};
use crate::photo::ConfirmationId;
use snafu::{Snafu, ensure};
use uuid::Uuid;

/// Identifies one attached display window, independently of its changing images.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayId(Uuid);

/// Monotonic frame revision within this capture session; constructed only by the backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresentationRevision(u64);
impl PresentationRevision {
    /// Returns the revision for display diagnostics, without allowing construction.
    #[must_use]
    pub fn value(self) -> u64 {
        self.0
    }
}

/// Immutable renderer request; loading the file alone does not acknowledge its presentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresentationRequest {
    display: DisplayId,
    revision: PresentationRevision,
    target: PresentationTarget,
    confirmation_id: ConfirmationId,
}
impl PresentationRequest {
    /// Returns the display instance authorized to acknowledge this request.
    #[must_use]
    pub fn display_id(&self) -> DisplayId {
        self.display
    }
    /// Returns the exact frame revision.
    #[must_use]
    pub fn revision(&self) -> PresentationRevision {
        self.revision
    }
    /// Returns the locked capture group.
    #[must_use]
    pub fn group_id(&self) -> GroupId {
        self.target.group
    }
    /// Returns the stable team identity, not its external account or seat.
    #[must_use]
    pub fn account_id(&self) -> &str {
        &self.target.account
    }
    /// Returns the selected candidate identity.
    #[must_use]
    pub fn photo_id(&self) -> Uuid {
        self.target.photo
    }
    /// Returns the exact normalized file, hash and dimensions to render.
    #[must_use]
    pub fn image(&self) -> &CandidateImage {
        &self.target.image
    }
}

/// Renderer claim made only after rendering the verified file in the attached window.
/// Backend matching validates identity, not the physical screen or contestant satisfaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresentationReceipt {
    pub display_id: DisplayId,
    pub revision: PresentationRevision,
    pub group_id: GroupId,
    pub account_id: String,
    pub photo_id: Uuid,
    pub image_sha256: String,
}

/// Display lifecycle and frame acknowledgement, independent of camera ingestion state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresentationStatus {
    Unavailable,
    Blank { display_id: DisplayId },
    Pending(PresentationRequest),
    Presented(PresentationRequest),
}
impl PresentationStatus {
    fn display_id(&self) -> Option<DisplayId> {
        match self {
            Self::Unavailable => None,
            Self::Blank { display_id } => Some(*display_id),
            Self::Pending(request) | Self::Presented(request) => Some(request.display),
        }
    }
}

/// Operator intent checked against an acknowledged frame; not a saved or reserved final.
/// Persistence must revalidate this request when consuming it.
#[derive(Debug, Clone)]
pub struct ConfirmationIntent {
    request: PresentationRequest,
}
impl ConfirmationIntent {
    /// Returns the exact team/candidate/version the operator chose to confirm.
    #[must_use]
    pub fn request(&self) -> &PresentationRequest {
        &self.request
    }
    pub(crate) fn confirmation_id(&self) -> &ConfirmationId {
        &self.request.confirmation_id
    }
}

/// Expected lifecycle/frame rejection; revision exhaustion fails closed.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub(super)))]
pub enum PresentationError {
    #[snafu(display("a display is already attached; detach it before attaching another"))]
    DisplayAttached,
    #[snafu(display("display instance is no longer attached"))]
    StaleDisplay,
    #[snafu(display("presentation receipt does not match the current frame"))]
    StaleReceipt,
    #[snafu(display("the expected frame has not been acknowledged or is no longer current"))]
    NotPresented,
    #[snafu(display("presentation revision space is exhausted; restart the capture session"))]
    RevisionExhausted,
}
type Result<T> = std::result::Result<T, PresentationError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PresentationTarget {
    pub group: GroupId,
    pub account: String,
    pub photo: Uuid,
    pub image: CandidateImage,
}

pub(super) struct Presentation {
    revision: u64,
    status: PresentationStatus,
}
impl Presentation {
    pub(super) fn new() -> Self {
        Self {
            revision: 0,
            status: PresentationStatus::Unavailable,
        }
    }
    pub(super) fn status(&self) -> PresentationStatus {
        self.status.clone()
    }
    pub(super) fn attach(&mut self) -> Result<DisplayId> {
        ensure!(self.status.display_id().is_none(), DisplayAttachedSnafu);
        let display_id = DisplayId(Uuid::now_v7());
        self.status = PresentationStatus::Blank { display_id };
        Ok(display_id)
    }
    pub(super) fn detach(&mut self, display: DisplayId) -> Result<()> {
        ensure!(self.status.display_id() == Some(display), StaleDisplaySnafu);
        self.stop();
        Ok(())
    }
    pub(super) fn stop(&mut self) {
        self.status = PresentationStatus::Unavailable;
    }
    pub(super) fn refresh(&mut self, target: Option<PresentationTarget>) -> Result<()> {
        let Some(display) = self.status.display_id() else {
            return Ok(());
        };
        let Some(target) = target else {
            self.status = PresentationStatus::Blank {
                display_id: display,
            };
            return Ok(());
        };
        if let PresentationStatus::Pending(current) | PresentationStatus::Presented(current) =
            &self.status
            && current.target == target
        {
            return Ok(());
        }
        let Some(revision) = self.revision.checked_add(1) else {
            self.stop();
            return RevisionExhaustedSnafu.fail();
        };
        self.revision = revision;
        self.status = PresentationStatus::Pending(PresentationRequest {
            display,
            revision: PresentationRevision(revision),
            target,
            confirmation_id: ConfirmationId::generate(),
        });
        Ok(())
    }
    pub(super) fn acknowledge(&mut self, receipt: &PresentationReceipt) -> Result<()> {
        let current = match &self.status {
            PresentationStatus::Pending(request) | PresentationStatus::Presented(request) => {
                request
            }
            PresentationStatus::Unavailable | PresentationStatus::Blank { .. } => {
                return StaleReceiptSnafu.fail();
            }
        };
        ensure!(receipt.display_id == current.display, StaleReceiptSnafu);
        if receipt.revision != current.revision
            || receipt.group_id != current.target.group
            || receipt.account_id != current.target.account
            || receipt.photo_id != current.target.photo
            || receipt.image_sha256 != current.target.image.sha256()
        {
            // A stale callback and a stale load painted over the current image
            // are indistinguishable. Require a fresh revision, not an old queued
            // acknowledgement of the previously accepted frame.
            self.status = PresentationStatus::Blank {
                display_id: current.display,
            };
            return StaleReceiptSnafu.fail();
        }
        self.status = PresentationStatus::Presented(current.clone());
        Ok(())
    }
    pub(super) fn prepare_confirmation(
        &self,
        expected: &PresentationRequest,
    ) -> Result<ConfirmationIntent> {
        ensure!(
            matches!(&self.status, PresentationStatus::Presented(current) if current == expected),
            NotPresentedSnafu
        );
        Ok(ConfirmationIntent {
            request: expected.clone(),
        })
    }
}

#[cfg(test)]
#[path = "../../tests/unit/presentation.rs"]
mod tests;
