use std::collections::HashMap;

use tokio::sync::Mutex;
use uuid::Uuid;

use super::types::{
    ActivationError, ControlAuthority, ControlPublicKey, DeviceError, EvidenceQuality,
    MachineHardwareId,
};

/// Opaque handle for one connection-local Enrollment review.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct EnrollmentReviewId(Uuid);

impl EnrollmentReviewId {
    fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

/// Validated, non-secret facts shown during manual Enrollment review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ValidatedEnrollmentEvidence {
    machine_hardware_id: MachineHardwareId,
    candidate_public_key: ControlPublicKey,
    evidence_quality: EvidenceQuality,
    daemon_version: String,
    agent_version: String,
}

impl ValidatedEnrollmentEvidence {
    pub(crate) fn new(
        machine_hardware_id: MachineHardwareId,
        candidate_public_key: ControlPublicKey,
        evidence_quality: EvidenceQuality,
        daemon_version: String,
        agent_version: String,
    ) -> Self {
        Self {
            machine_hardware_id,
            candidate_public_key,
            evidence_quality,
            daemon_version,
            agent_version,
        }
    }

    pub(crate) const fn machine_hardware_id(&self) -> MachineHardwareId {
        self.machine_hardware_id
    }

    pub(crate) const fn candidate_public_key(&self) -> ControlPublicKey {
        self.candidate_public_key
    }

    pub(crate) const fn evidence_quality(&self) -> EvidenceQuality {
        self.evidence_quality
    }

    pub(crate) fn daemon_version(&self) -> &str {
        &self.daemon_version
    }

    pub(crate) fn agent_version(&self) -> &str {
        &self.agent_version
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingEnrollmentReview {
    review_id: EnrollmentReviewId,
    evidence: ValidatedEnrollmentEvidence,
}

impl PendingEnrollmentReview {
    pub(crate) const fn review_id(&self) -> EnrollmentReviewId {
        self.review_id
    }

    pub(crate) const fn evidence(&self) -> &ValidatedEnrollmentEvidence {
        &self.evidence
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EnrollmentStartOutcome {
    Replay(ControlAuthority),
    Pending(PendingEnrollmentReview),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EnrollmentStartError {
    ProvisioningClosed,
    Authority(DeviceError),
}

impl From<DeviceError> for EnrollmentStartError {
    fn from(error: DeviceError) -> Self {
        Self::Authority(error)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EnrollmentApprovalError {
    ProvisioningClosed,
    ReviewNotFound,
    Activation(ActivationError),
}

impl From<ActivationError> for EnrollmentApprovalError {
    fn from(error: ActivationError) -> Self {
        Self::Activation(error)
    }
}

/// Process-local pending Enrollment reviews. A new process starts empty.
pub(in crate::component::device) struct EnrollmentReviewRegistry {
    reviews: Mutex<HashMap<EnrollmentReviewId, ValidatedEnrollmentEvidence>>,
}

impl EnrollmentReviewRegistry {
    pub(in crate::component::device) fn new() -> Self {
        Self {
            reviews: Mutex::new(HashMap::new()),
        }
    }

    pub(in crate::component::device) async fn create(
        &self,
        evidence: ValidatedEnrollmentEvidence,
    ) -> PendingEnrollmentReview {
        let review_id = EnrollmentReviewId::new();
        self.reviews
            .lock()
            .await
            .insert(review_id, evidence.clone());
        PendingEnrollmentReview {
            review_id,
            evidence,
        }
    }

    pub(in crate::component::device) async fn list(&self) -> Vec<PendingEnrollmentReview> {
        self.reviews
            .lock()
            .await
            .iter()
            .map(|(&review_id, evidence)| PendingEnrollmentReview {
                review_id,
                evidence: evidence.clone(),
            })
            .collect()
    }

    pub(in crate::component::device) async fn take(
        &self,
        review_id: EnrollmentReviewId,
    ) -> Option<ValidatedEnrollmentEvidence> {
        self.reviews.lock().await.remove(&review_id)
    }

    pub(in crate::component::device) async fn remove(&self, review_id: EnrollmentReviewId) -> bool {
        self.take(review_id).await.is_some()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use uuid::Version;

    use super::{EnrollmentReviewRegistry, ValidatedEnrollmentEvidence};
    use crate::component::device::types::{ControlPublicKey, EvidenceQuality, MachineHardwareId};

    fn evidence(seed: u8) -> ValidatedEnrollmentEvidence {
        ValidatedEnrollmentEvidence::new(
            MachineHardwareId::parse("a9aa9d04-3ece-5567-8260-910930ff5e03")
                .unwrap_or_else(|| panic!("the fixture Machine Hardware ID is valid")),
            ControlPublicKey::parse(&[seed; 32])
                .unwrap_or_else(|| panic!("the fixture control key is valid")),
            EvidenceQuality::Strong,
            "2.0.0".to_owned(),
            "2.0.1".to_owned(),
        )
    }

    #[tokio::test]
    async fn create_lists_the_review_and_take_is_terminal() {
        let registry = EnrollmentReviewRegistry::new();
        let expected = evidence(7);
        let created = registry.create(expected.clone()).await;

        assert_eq!(created.review_id.0.get_version(), Some(Version::SortRand));
        assert_eq!(created.evidence(), &expected);
        assert_eq!(registry.list().await, vec![created.clone()]);
        assert_eq!(registry.take(created.review_id()).await, Some(expected));
        assert_eq!(registry.take(created.review_id()).await, None);
        assert!(registry.list().await.is_empty());
    }

    #[tokio::test]
    async fn remove_deletes_only_the_selected_review() {
        let registry = EnrollmentReviewRegistry::new();
        let first = registry.create(evidence(1)).await;
        let second = registry.create(evidence(2)).await;

        assert!(registry.remove(first.review_id()).await);
        assert!(!registry.remove(first.review_id()).await);
        assert_eq!(registry.list().await, vec![second]);
    }

    #[tokio::test]
    async fn concurrent_terminal_actions_have_one_winner() {
        let registry = Arc::new(EnrollmentReviewRegistry::new());
        let review_id = registry.create(evidence(9)).await.review_id();

        let first_registry = Arc::clone(&registry);
        let first = tokio::spawn(async move { first_registry.take(review_id).await.is_some() });
        let second_registry = Arc::clone(&registry);
        let second = tokio::spawn(async move { second_registry.remove(review_id).await });

        let outcomes = [
            first
                .await
                .unwrap_or_else(|error| panic!("first action failed: {error}")),
            second
                .await
                .unwrap_or_else(|error| panic!("second action failed: {error}")),
        ];
        assert_eq!(outcomes.into_iter().filter(|won| *won).count(), 1);
        assert!(registry.list().await.is_empty());
    }

    #[tokio::test]
    async fn a_new_registry_has_no_old_process_reviews() {
        let old_registry = EnrollmentReviewRegistry::new();
        old_registry.create(evidence(4)).await;
        assert_eq!(old_registry.list().await.len(), 1);

        assert!(EnrollmentReviewRegistry::new().list().await.is_empty());
    }
}
