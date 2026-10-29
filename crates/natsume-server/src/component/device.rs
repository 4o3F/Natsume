mod authority;
mod db;
mod enrollment;
mod lifecycle;
mod types;

use std::net::IpAddr;

use crate::{
    db::{Database, PersistenceError, Transaction},
    journal::{
        Journal, JournalContext,
        event::{
            Event,
            payload::{
                EnrollmentApproved, EnrollmentDenied, LifecycleChanged, ReviewCancelled,
                ReviewCreated,
            },
            types::{LifecycleState, ReviewCancelCause, ReviewId},
        },
    },
};

use self::enrollment::EnrollmentReviewRegistry;
pub(crate) use self::enrollment::{
    EnrollmentApproval, EnrollmentApprovalError, EnrollmentReviewDecision, EnrollmentReviewId,
    EnrollmentStartError, EnrollmentStartOutcome, PendingEnrollmentReview,
    ValidatedEnrollmentEvidence,
};
pub(crate) use self::types::{
    ActivationError, ControlAuthority, ControlPublicKey, DeviceError, DeviceId, DeviceNetworkInfo,
    DeviceProjection, DeviceState, EvidenceQuality, LifecycleOutcome, MachineHardwareId,
};

/// Owns Device authority, lifecycle, and process-local Enrollment review invariants.
///
/// Persistent authority and lifecycle mutations stay in this component. Pending
/// reviews are deliberately process-local, with their result sender assigned to the
/// exact connection that created them.
pub(crate) struct DeviceComponent {
    database: Database,
    reviews: EnrollmentReviewRegistry,
    /// Emit-only handle for committed authority and process-local review transitions.
    journal: Journal,
}

/// Lifecycle selection for the Operator device list; it does not change authority.
#[derive(Debug, Clone)]
pub(crate) enum DeviceListFilter {
    All,
    States(Vec<DeviceState>),
}

/// Origin of the approval that commits a proved enrollment candidate.
#[derive(Clone, Copy)]
pub(crate) enum EnrollmentApprovalMode {
    Manual,
    Automatic,
}

impl EnrollmentApprovalMode {
    pub(crate) fn is_automatic(self) -> bool {
        matches!(self, Self::Automatic)
    }
}

impl DeviceComponent {
    /// Records one authenticated connection's complete address pair. The actor
    /// serializes leases; this transaction also fences lifecycle/key replacement.
    pub(crate) async fn record_network_addresses(
        &self,
        authority: ControlAuthority,
        server_ip: IpAddr,
        client_ip: Option<IpAddr>,
    ) -> Result<(), DeviceError> {
        self.database
            .write(move |transaction| -> Result<(), PersistenceError> {
                let device_id = authority.device_id();
                let Some(device) = db::find_by_id(transaction, &device_id)? else {
                    return Ok(());
                };
                if device.state() != DeviceState::Enabled
                    || db::find_current_for_device(transaction, &device_id)?
                        != Some(authority.control_public_key())
                {
                    return Ok(());
                }
                let now = db::current_unix_ms(transaction)?;
                db::update_network_addresses(transaction, &device_id, server_ip, client_ip, now)?;
                Ok(())
            })
            .await
            .map_err(crate::db::TransactionError::into_error)
            .map_err(DeviceError::from)
    }

    pub(in crate::component) fn enabled_target_devices(
        transaction: &mut Transaction<'_>,
    ) -> Result<Vec<DeviceId>, PersistenceError> {
        db::enabled_ids(transaction)
    }

    pub(in crate::component) fn target_device_state(
        transaction: &mut Transaction<'_>,
        device: &DeviceId,
    ) -> Result<Option<DeviceState>, PersistenceError> {
        db::find_by_id(transaction, device).map(|record| record.map(|device| device.state()))
    }

    pub(crate) fn new(database: Database, journal: Journal) -> Self {
        Self {
            database,
            reviews: EnrollmentReviewRegistry::new(),
            journal,
        }
    }

    /// Lists the durable, non-secret Device fields required by the Operator Panel.
    pub(crate) async fn list_devices(
        &self,
        filter: DeviceListFilter,
    ) -> Result<Vec<DeviceProjection>, DeviceError> {
        self.database
            .read(move |transaction| db::list(transaction, filter))
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

    /// Classifies an exact committed authority replay; a new or replacement candidate
    /// gets a review that can be approved automatically or by an administrator.
    pub(crate) async fn start_enrollment(
        &self,
        evidence: ValidatedEnrollmentEvidence,
        peer_ip: IpAddr,
        context: JournalContext,
    ) -> Result<EnrollmentStartOutcome, EnrollmentStartError> {
        let current_authority = self
            .find_current_authority(evidence.machine_hardware_id())
            .await?;
        if let Some(authority) = current_authority
            && authority.control_public_key() == evidence.candidate_public_key()
        {
            return Ok(EnrollmentStartOutcome::Replay(authority));
        }
        let (review, activation) = self.reviews.create(evidence).await?;
        self.journal.emit(
            Event::ReviewCreated(ReviewCreated {
                review_id: ReviewId::from_enrollment_review(review.review_id()),
                peer_ip,
            }),
            context,
        );
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

    /// Atomically claims the exact attached review, then commits Device/control-key
    /// activation without holding the review lock.
    ///
    /// Activation failure is sent directly to the originating connection. Success
    /// returns an [`EnrollmentApproval`] so application coordination can evict the old
    /// lease before notifying that connection. Dropping the success notification never
    /// rolls back the terminal claim or committed authority.
    pub(crate) async fn approve_enrollment(
        &self,
        review_id: EnrollmentReviewId,
        mode: EnrollmentApprovalMode,
        context: JournalContext,
    ) -> Result<EnrollmentApproval, EnrollmentApprovalError> {
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
            Ok(authority) => {
                self.journal.emit(
                    Event::EnrollmentApproved(EnrollmentApproved {
                        review_id: ReviewId::from_enrollment_review(review_id),
                        automatic: mode.is_automatic(),
                    }),
                    context.with_device(authority.device_id()),
                );
                Ok(EnrollmentApproval {
                    authority,
                    completion: activation,
                })
            }
            Err(error) => {
                let error = EnrollmentApprovalError::from(error);
                let _ = activation.send(Err(error.clone()));
                Err(error)
            }
        }
    }

    /// Claims one pending review and attempts to notify only its originating connection.
    pub(crate) async fn deny_enrollment_review(
        &self,
        review_id: EnrollmentReviewId,
        context: JournalContext,
    ) -> bool {
        let Some((_, completion)) = self.reviews.take(review_id).await else {
            return false;
        };
        let _ = completion.send(Ok(EnrollmentReviewDecision::Denied));
        self.journal.emit(
            Event::EnrollmentDenied(EnrollmentDenied {
                review_id: ReviewId::from_enrollment_review(review_id),
            }),
            context,
        );
        true
    }

    /// Cancels one process-local review and wakes its waiting connection by dropping
    /// the attached result sender.
    pub(crate) async fn remove_enrollment_review(
        &self,
        review_id: EnrollmentReviewId,
        cause: ReviewCancelCause,
        context: JournalContext,
    ) -> bool {
        let winning = self.reviews.remove(review_id).await;
        if winning {
            self.journal.emit(
                Event::ReviewCancelled(ReviewCancelled {
                    review_id: ReviewId::from_enrollment_review(review_id),
                    cause,
                }),
                context,
            );
        }
        winning
    }

    /// Lists non-secret projections of reviews pending in this process.
    pub(crate) async fn pending_enrollment_reviews(&self) -> Vec<PendingEnrollmentReview> {
        self.reviews.list().await
    }

    pub(crate) async fn enable(
        &self,
        device_id: DeviceId,
        context: JournalContext,
    ) -> Result<LifecycleOutcome, DeviceError> {
        let outcome = lifecycle::enable(&self.database, device_id).await?;
        if outcome == LifecycleOutcome::Changed {
            self.journal.emit(
                Event::LifecycleChanged(LifecycleChanged {
                    to: LifecycleState::Enabled,
                }),
                context.with_device(device_id),
            );
        }
        Ok(outcome)
    }

    pub(crate) async fn disable(
        &self,
        device_id: DeviceId,
        context: JournalContext,
    ) -> Result<LifecycleOutcome, DeviceError> {
        let outcome = lifecycle::disable(&self.database, device_id).await?;
        if outcome == LifecycleOutcome::Changed {
            self.journal.emit(
                Event::LifecycleChanged(LifecycleChanged {
                    to: LifecycleState::Disabled,
                }),
                context.with_device(device_id),
            );
        }
        Ok(outcome)
    }

    pub(crate) async fn revoke(
        &self,
        device_id: DeviceId,
        context: JournalContext,
    ) -> Result<LifecycleOutcome, DeviceError> {
        let outcome = lifecycle::revoke(&self.database, device_id).await?;
        if outcome == LifecycleOutcome::Changed {
            self.journal.emit(
                Event::LifecycleChanged(LifecycleChanged {
                    to: LifecycleState::Revoked,
                }),
                context.with_device(device_id),
            );
        }
        Ok(outcome)
    }
}

#[cfg(test)]
mod tests;
