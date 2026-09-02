mod authority;
mod db;
mod enrollment;
mod lifecycle;
mod types;

use crate::{component::provisioning::ProvisioningComponent, db::Database};

use self::enrollment::EnrollmentReviewRegistry;
pub(crate) use self::enrollment::{
    EnrollmentApprovalError, EnrollmentReviewId, EnrollmentStartError, EnrollmentStartOutcome,
    PendingEnrollmentReview, ValidatedEnrollmentEvidence,
};
pub(crate) use self::types::{
    ActivationError, ControlAuthority, ControlPublicKey, DeviceError, DeviceId, DeviceState,
    EvidenceQuality, LifecycleOutcome, MachineHardwareId,
};

pub(crate) struct DeviceComponent {
    database: Database,
    reviews: EnrollmentReviewRegistry,
}

impl DeviceComponent {
    pub(crate) fn new(database: Database) -> Self {
        Self {
            database,
            reviews: EnrollmentReviewRegistry::new(),
        }
    }

    pub(crate) async fn find_current_authority(
        &self,
        machine_hardware_id: MachineHardwareId,
    ) -> Result<Option<ControlAuthority>, DeviceError> {
        authority::find_current_authority(&self.database, machine_hardware_id).await
    }

    /// Classifies an exact committed authority replay before consulting the
    /// process-local provisioning gate; only a new or replacement candidate
    /// becomes a pending manual review.
    pub(crate) async fn start_enrollment(
        &self,
        provisioning: &ProvisioningComponent,
        evidence: ValidatedEnrollmentEvidence,
    ) -> Result<EnrollmentStartOutcome, EnrollmentStartError> {
        if let Some(authority) = self
            .find_current_authority(evidence.machine_hardware_id())
            .await?
            .filter(|authority| authority.control_public_key() == evidence.candidate_public_key())
        {
            return Ok(EnrollmentStartOutcome::Replay(authority));
        }
        if !provisioning.read_window().await.is_open() {
            return Err(EnrollmentStartError::ProvisioningClosed);
        }
        Ok(EnrollmentStartOutcome::Pending(
            self.reviews.create(evidence).await,
        ))
    }

    /// Rechecks the gate, atomically claims the exact attached review, then
    /// commits Device/control-key activation without holding the review lock.
    pub(crate) async fn approve_enrollment(
        &self,
        provisioning: &ProvisioningComponent,
        review_id: EnrollmentReviewId,
    ) -> Result<ControlAuthority, EnrollmentApprovalError> {
        if !provisioning.read_window().await.is_open() {
            return Err(EnrollmentApprovalError::ProvisioningClosed);
        }
        let evidence = self
            .reviews
            .take(review_id)
            .await
            .ok_or(EnrollmentApprovalError::ReviewNotFound)?;
        authority::activate(
            &self.database,
            evidence.machine_hardware_id(),
            evidence.candidate_public_key(),
            evidence.evidence_quality(),
        )
        .await
        .map_err(EnrollmentApprovalError::from)
    }

    pub(crate) async fn remove_enrollment_review(&self, review_id: EnrollmentReviewId) -> bool {
        self.reviews.remove(review_id).await
    }

    pub(crate) async fn pending_enrollment_reviews(&self) -> Vec<PendingEnrollmentReview> {
        self.reviews.list().await
    }

    pub(crate) async fn enable(
        &self,
        device_id: DeviceId,
    ) -> Result<LifecycleOutcome, DeviceError> {
        lifecycle::enable(&self.database, device_id).await
    }

    pub(crate) async fn disable(
        &self,
        device_id: DeviceId,
    ) -> Result<LifecycleOutcome, DeviceError> {
        lifecycle::disable(&self.database, device_id).await
    }

    pub(crate) async fn revoke(
        &self,
        device_id: DeviceId,
    ) -> Result<LifecycleOutcome, DeviceError> {
        lifecycle::revoke(&self.database, device_id).await
    }
}

#[cfg(test)]
mod tests;
