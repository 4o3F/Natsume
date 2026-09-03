mod authority;
mod db;
mod enrollment;
mod lifecycle;
mod types;

use crate::{component::provisioning::ProvisioningComponent, db::Database};

use self::enrollment::EnrollmentReviewRegistry;
pub(crate) use self::enrollment::{
    EnrollmentApproval, EnrollmentApprovalError, EnrollmentReviewDecision, EnrollmentReviewId,
    EnrollmentStartError, EnrollmentStartOutcome, PendingEnrollmentReview,
    ValidatedEnrollmentEvidence,
};
pub(crate) use self::types::{
    ActivationError, ControlAuthority, ControlPublicKey, DeviceError, DeviceId, DeviceProjection,
    DeviceState, EvidenceQuality, LifecycleOutcome, MachineHardwareId,
};

/// Owns Device authority, lifecycle, and process-local Enrollment review invariants.
///
/// Persistent authority and lifecycle mutations stay in this component. Pending
/// reviews are deliberately process-local, with their result sender assigned to the
/// exact connection that created them.
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

    /// Lists the durable, non-secret Device fields required by the Operator Panel.
    pub(crate) async fn list_devices(&self) -> Result<Vec<DeviceProjection>, DeviceError> {
        self.database
            .read(db::list)
            .await
            .map_err(crate::db::TransactionError::into_error)
            .map_err(DeviceError::from)
    }

    /// Reads one durable Device projection without changing its lifecycle.
    pub(crate) async fn find_device(
        &self,
        device_id: DeviceId,
    ) -> Result<Option<DeviceProjection>, DeviceError> {
        self.database
            .read(move |transaction| db::find_projection(transaction, &device_id))
            .await
            .map_err(crate::db::TransactionError::into_error)
            .map_err(DeviceError::from)
    }

    /// Selects the database's current control authority for a Machine Hardware ID.
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
        let current_authority = self
            .find_current_authority(evidence.machine_hardware_id())
            .await?;
        if let Some(authority) = current_authority
            && authority.control_public_key() == evidence.candidate_public_key()
        {
            return Ok(EnrollmentStartOutcome::Replay(authority));
        }
        if !provisioning.read_window().await.is_open() {
            return Err(EnrollmentStartError::ProvisioningClosed);
        }
        let (review, activation) = self.reviews.create(evidence).await;
        Ok(EnrollmentStartOutcome::Pending(review, activation))
    }

    /// Returns the immutable Machine Hardware ID attached to a pending review.
    pub(crate) async fn enrollment_review_machine_hardware_id(
        &self,
        review_id: EnrollmentReviewId,
    ) -> Result<MachineHardwareId, EnrollmentApprovalError> {
        self.reviews
            .machine_hardware_id(review_id)
            .await
            .ok_or(EnrollmentApprovalError::ReviewNotFound)
    }

    /// Rechecks the gate, atomically claims the exact attached review, then commits
    /// Device/control-key activation without holding the review lock.
    ///
    /// Activation failure is sent directly to the originating connection. Success
    /// returns an [`EnrollmentApproval`] so application coordination can evict the old
    /// lease before notifying that connection. Dropping the success notification never
    /// rolls back the terminal claim or committed authority.
    pub(crate) async fn approve_enrollment(
        &self,
        provisioning: &ProvisioningComponent,
        review_id: EnrollmentReviewId,
    ) -> Result<EnrollmentApproval, EnrollmentApprovalError> {
        if !provisioning.read_window().await.is_open() {
            return Err(EnrollmentApprovalError::ProvisioningClosed);
        }
        let (evidence, activation) = self
            .reviews
            .take(review_id)
            .await
            .ok_or(EnrollmentApprovalError::ReviewNotFound)?;
        match authority::activate(
            &self.database,
            evidence.machine_hardware_id(),
            evidence.candidate_public_key(),
            evidence.evidence_quality(),
        )
        .await
        {
            Ok(authority) => Ok(EnrollmentApproval {
                authority,
                completion: activation,
            }),
            Err(error) => {
                let error = EnrollmentApprovalError::from(error);
                let _ = activation.send(Err(error));
                Err(error)
            }
        }
    }

    /// Claims one pending review and attempts to notify only its originating connection.
    pub(crate) async fn deny_enrollment_review(&self, review_id: EnrollmentReviewId) -> bool {
        let Some((_, completion)) = self.reviews.take(review_id).await else {
            return false;
        };
        let _ = completion.send(Ok(EnrollmentReviewDecision::Denied));
        true
    }

    /// Cancels one process-local review and wakes its waiting connection by dropping
    /// the attached result sender.
    pub(crate) async fn remove_enrollment_review(&self, review_id: EnrollmentReviewId) -> bool {
        self.reviews.remove(review_id).await
    }

    /// Lists non-secret projections of reviews pending in this process.
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
