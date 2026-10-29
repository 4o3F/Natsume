mod db;

use std::{collections::HashMap, fmt, sync::Arc};

use natsume_workstation_protocol::is_valid_domjudge_username;
use snafu::Snafu;
use uuid::{Uuid, Variant, Version};
use zeroize::Zeroizing;

use crate::{
    component::device::DeviceId,
    db::{Database, PersistenceError, Transaction, TransactionError},
    device_control::TargetChange,
    journal::{Journal, JournalContext, event::Event},
    vault::VaultSession,
};

const PASSWORD_LENGTH_LIMIT: usize = 512;

#[derive(Debug, PartialEq, Eq)]
enum IngestOutcome {
    Unchanged,
    Bound {
        /// Binding identity inserted by the committed ingest transaction.
        binding_id: Uuid,
        /// Seat code selected from the committed binding's seat row.
        seat_code: String,
    },
    Rejected {
        /// Evaluation newly committed for this submission epoch.
        code: BindingEvaluationCode,
    },
}

pub(crate) struct BindingComponent {
    database: Database,
    vault: Arc<VaultSession>,
    /// Emit-only handle for facts committed by this component.
    journal: Journal,
}

impl BindingComponent {
    pub(crate) const fn new(
        database: Database,
        vault: Arc<VaultSession>,
        journal: Journal,
    ) -> Self {
        Self {
            database,
            vault,
            journal,
        }
    }

    pub(crate) async fn ingest(
        &self,
        device_id: DeviceId,
        input: Option<BindingInput>,
        context: JournalContext,
    ) -> Result<(), BindingError> {
        let initial_negotiation_id = BindingNegotiationId::new();
        let binding_id = BindingId::new();
        let outcome = self
            .database
            .write(move |transaction| {
                require_enabled_device(transaction, &device_id)?;
                let has_binding = db::device_has_binding(transaction, &device_id)?;
                let negotiation = db::find_negotiation(transaction, &device_id)?;
                match (has_binding, negotiation) {
                    (true, Some(_)) => Err(BindingError::InvalidPersistedFacts {
                        source: crate::db::PersistenceError::invalid("persisted facts"),
                    }),
                    (true, None) => Ok(IngestOutcome::Unchanged),
                    (false, negotiation) => {
                        let negotiation = if let Some(row) = negotiation {
                            NegotiationFact::from_persisted(&row)?
                        } else {
                            require_one(db::insert_negotiation(
                                transaction,
                                &device_id,
                                initial_negotiation_id.value(),
                            )?)?;
                            NegotiationFact::new(initial_negotiation_id)
                        };
                        ingest_submission(
                            transaction,
                            &device_id,
                            &negotiation,
                            input.as_ref(),
                            binding_id,
                        )
                    }
                }
            })
            .await
            .map_err(TransactionError::into_error)?;
        self.record_ingest(device_id, context, outcome);
        Ok(())
    }

    fn record_ingest(&self, device_id: DeviceId, context: JournalContext, outcome: IngestOutcome) {
        use crate::journal::event::{
            payload::{BindingBound, BindingRejected},
            types::BindingCode,
        };
        let context = context.with_device(device_id);
        match outcome {
            IngestOutcome::Unchanged => {}
            IngestOutcome::Bound {
                binding_id,
                seat_code,
            } => {
                if let Some(seat_code) = committed_seat_code(&seat_code) {
                    self.journal.emit(
                        Event::BindingBound(BindingBound {
                            binding_id,
                            seat_code: seat_code.clone(),
                        }),
                        context.with_seat(seat_code),
                    );
                }
            }
            IngestOutcome::Rejected { code } => {
                let code = match code {
                    BindingEvaluationCode::NotFound => BindingCode::NotFound,
                    BindingEvaluationCode::Unmapped => BindingCode::Unmapped,
                    BindingEvaluationCode::Occupied => BindingCode::Occupied,
                };
                self.journal
                    .emit(Event::BindingRejected(BindingRejected { code }), context);
            }
        }
    }

    pub(crate) async fn materialize(
        &self,
        device_id: DeviceId,
    ) -> Result<MaterializedBinding, BindingError> {
        let initial_negotiation_id = BindingNegotiationId::new();
        let fact = self
            .database
            .write(move |transaction| {
                require_enabled_device(transaction, &device_id)?;
                let has_binding = db::device_has_binding(transaction, &device_id)?;
                let negotiation = db::find_negotiation(transaction, &device_id)?;
                match (has_binding, negotiation) {
                    (true, Some(_)) => Err(BindingError::InvalidPersistedFacts {
                        source: crate::db::PersistenceError::invalid("persisted facts"),
                    }),
                    (true, None) => db::find_bound_target(transaction, &device_id)?
                        .map(BindingFact::Bound)
                        .ok_or(BindingError::InvalidPersistedFacts {
                            source: crate::db::PersistenceError::invalid("persisted facts"),
                        }),
                    (false, Some(row)) => {
                        NegotiationFact::from_persisted(&row).map(BindingFact::Unbound)
                    }
                    (false, None) => {
                        require_one(db::insert_negotiation(
                            transaction,
                            &device_id,
                            initial_negotiation_id.value(),
                        )?)?;
                        Ok(BindingFact::Unbound(NegotiationFact::new(
                            initial_negotiation_id,
                        )))
                    }
                }
            })
            .await
            .map_err(TransactionError::into_error)?;

        self.resolve(fact)
    }

    /// Reads the current durable Binding target without creating a negotiation.
    pub(crate) async fn read_current(
        &self,
        device_id: DeviceId,
    ) -> Result<Option<BindingProjection>, BindingError> {
        self.database
            .read(move |transaction| {
                require_existing_device(transaction, &device_id)?;
                let has_binding = db::device_has_binding(transaction, &device_id)?;
                let negotiation = db::find_negotiation(transaction, &device_id)?;
                match (has_binding, negotiation) {
                    (true, Some(_)) => Err(BindingError::InvalidPersistedFacts {
                        source: crate::db::PersistenceError::invalid("persisted facts"),
                    }),
                    (true, None) => db::find_bound_context(transaction, &device_id)?
                        .as_ref()
                        .map(BindingContext::from_persisted)
                        .transpose()?
                        .map(BindingProjection::Bound)
                        .ok_or(BindingError::InvalidPersistedFacts {
                            source: crate::db::PersistenceError::invalid("persisted facts"),
                        })
                        .map(Some),
                    (false, Some(row)) => NegotiationFact::from_persisted(&row)
                        .map(NegotiationFact::into_intent)
                        .map(BindingProjection::Unbound)
                        .map(Some),
                    (false, None) => Ok(None),
                }
            })
            .await
            .map_err(TransactionError::into_error)
    }

    /// Reads and validates every current redacted Binding projection in two queries.
    pub(crate) async fn read_all_current(
        &self,
    ) -> Result<HashMap<DeviceId, BindingProjection>, BindingError> {
        self.database
            .read(|transaction| {
                let negotiations = db::list_negotiations(transaction)?;
                let contexts = db::list_bound_contexts(transaction)?;
                let mut projections = HashMap::with_capacity(negotiations.len() + contexts.len());
                for (device_id, row) in negotiations {
                    let device_id =
                        DeviceId::parse(&device_id).ok_or(BindingError::InvalidPersistedFacts {
                            source: crate::db::PersistenceError::invalid("persisted facts"),
                        })?;
                    let projection = BindingProjection::Unbound(
                        NegotiationFact::from_persisted(&row)?.into_intent(),
                    );
                    projections.insert(device_id, projection);
                }
                for (device_id, row) in contexts {
                    let device_id =
                        DeviceId::parse(&device_id).ok_or(BindingError::InvalidPersistedFacts {
                            source: crate::db::PersistenceError::invalid("persisted facts"),
                        })?;
                    let projection =
                        BindingProjection::Bound(BindingContext::from_persisted(&row)?);
                    if projections.insert(device_id, projection).is_some() {
                        return Err(BindingError::InvalidPersistedFacts {
                            source: crate::db::PersistenceError::invalid("persisted facts"),
                        });
                    }
                }
                Ok(projections)
            })
            .await
            .map_err(TransactionError::into_error)
    }

    fn resolve(&self, fact: BindingFact) -> Result<MaterializedBinding, BindingError> {
        match fact {
            BindingFact::Unbound(negotiation) => Ok(MaterializedBinding {
                intent: Some(negotiation.into_intent()),
                target: BindingAccessTarget { bound: None },
            }),
            BindingFact::Bound(row) => {
                let context = BindingContext::from_persisted(&row.context)?;
                let plaintext = self
                    .vault
                    .open(&row.nonce, &row.ciphertext)
                    .map_err(|source| BindingError::VaultFailure { source })?;
                let password = BindingPassword::new(plaintext)?;
                Ok(MaterializedBinding {
                    intent: None,
                    target: BindingAccessTarget {
                        bound: Some(BoundTarget {
                            context,
                            password,
                            presentation: row.presentation,
                        }),
                    },
                })
            }
        }
    }

    /// Removes the current Binding and starts a fresh negotiation if needed.
    pub(crate) async fn unbind(
        &self,
        device_id: DeviceId,
        context: JournalContext,
    ) -> Result<TargetChange, BindingError> {
        let replacement_negotiation_id = BindingNegotiationId::new();
        let seat_code = self
            .database
            .write(move |transaction| {
                require_existing_device(transaction, &device_id)?;
                let seat_code = db::find_bound_seat_code(transaction, &device_id)?;
                let negotiation = db::find_negotiation(transaction, &device_id)?;
                match (seat_code.as_ref(), negotiation) {
                    (Some(_), Some(_)) => {
                        return Err(BindingError::InvalidPersistedFacts {
                            source: crate::db::PersistenceError::invalid("persisted facts"),
                        });
                    }
                    (Some(_), None) => {
                        require_one(db::delete_binding(transaction, &device_id)?)?;
                        require_one(db::insert_negotiation(
                            transaction,
                            &device_id,
                            replacement_negotiation_id.value(),
                        )?)?;
                    }
                    (None, Some(_)) => {}
                    (None, None) => require_one(db::insert_negotiation(
                        transaction,
                        &device_id,
                        replacement_negotiation_id.value(),
                    )?)?,
                }
                Ok(seat_code)
            })
            .await
            .map_err(TransactionError::into_error)?;
        if let Some(seat_code) = seat_code.as_deref().and_then(committed_seat_code) {
            self.journal.emit(
                Event::BindingUnbound(crate::journal::event::payload::BindingUnbound {
                    seat_code: seat_code.clone(),
                }),
                context.with_device(device_id).with_seat(seat_code),
            );
        }
        Ok(TargetChange::device(device_id))
    }
}

fn committed_seat_code(value: &str) -> Option<crate::journal::event::types::SeatCode> {
    crate::journal::event::types::SeatCode::from_committed_binding(value)
        .inspect_err(|error| tracing::warn!(error = %snafu::Report::from_error(error), "committed binding is outside journal bounds"))
        .ok()
}

fn ingest_submission(
    transaction: &mut Transaction<'_>,
    device_id: &DeviceId,
    negotiation: &NegotiationFact,
    input: Option<&BindingInput>,
    binding_id: BindingId,
) -> Result<IngestOutcome, BindingError> {
    let Some(input) = input else {
        return Ok(IngestOutcome::Unchanged);
    };
    if input.negotiation_id != negotiation.negotiation_id {
        return Ok(IngestOutcome::Unchanged);
    }
    if let Some(rejected) = negotiation.rejected_submission.as_ref() {
        match input
            .submission_epoch
            .value()
            .cmp(&rejected.submission_epoch.value())
        {
            std::cmp::Ordering::Less => return Ok(IngestOutcome::Unchanged),
            std::cmp::Ordering::Equal if input.seat_code == rejected.seat_code => {
                return Ok(IngestOutcome::Unchanged);
            }
            std::cmp::Ordering::Equal => return Err(BindingError::ConflictingSubmission),
            std::cmp::Ordering::Greater => {}
        }
    }

    let seat = db::find_submission_seat(transaction, &input.seat_code)?;
    let evaluation = match seat.as_ref() {
        None => Some(BindingEvaluationCode::NotFound),
        Some(seat) if !seat.mapped => Some(BindingEvaluationCode::Unmapped),
        Some(seat) if seat.occupied => Some(BindingEvaluationCode::Occupied),
        Some(_) => None,
    };
    if let Some(evaluation) = evaluation {
        return require_one(db::store_evaluation(
            transaction,
            device_id,
            negotiation.negotiation_id.value(),
            input.submission_epoch.value(),
            &input.seat_code,
            evaluation.as_persisted(),
        )?)
        .map(|()| IngestOutcome::Rejected { code: evaluation });
    }

    let seat = seat.ok_or(BindingError::InvalidPersistedFacts {
        source: crate::db::PersistenceError::invalid("persisted facts"),
    })?;
    require_one(db::insert_binding(
        transaction,
        binding_id.value(),
        device_id,
        seat.seat_id,
    )?)?;
    require_one(db::delete_negotiation(
        transaction,
        device_id,
        negotiation.negotiation_id.value(),
    )?)?;
    Ok(IngestOutcome::Bound {
        binding_id: binding_id.value(),
        seat_code: seat.seat_code,
    })
}

fn require_enabled_device(
    transaction: &mut Transaction<'_>,
    device_id: &DeviceId,
) -> Result<(), BindingError> {
    match db::find_device_state(transaction, device_id)?.as_deref() {
        None => Err(BindingError::DeviceNotFound),
        Some("enabled") => Ok(()),
        Some("disabled" | "revoked") => Err(BindingError::DeviceNotEligible),
        Some(_) => Err(BindingError::InvalidPersistedFacts {
            source: crate::db::PersistenceError::invalid("persisted facts"),
        }),
    }
}

fn require_existing_device(
    transaction: &mut Transaction<'_>,
    device_id: &DeviceId,
) -> Result<(), BindingError> {
    match db::find_device_state(transaction, device_id)?.as_deref() {
        None => Err(BindingError::DeviceNotFound),
        Some("enabled" | "disabled" | "revoked") => Ok(()),
        Some(_) => Err(BindingError::InvalidPersistedFacts {
            source: crate::db::PersistenceError::invalid("persisted facts"),
        }),
    }
}

fn require_one(updated: usize) -> Result<(), BindingError> {
    if updated == 1 {
        Ok(())
    } else {
        Err(BindingError::InvalidPersistedFacts {
            source: crate::db::PersistenceError::unexpected_row_count(1, updated),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct BindingNegotiationId(Uuid);

impl BindingNegotiationId {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        canonical_uuid_v7(value).map(Self)
    }

    fn new() -> Self {
        Self(Uuid::now_v7())
    }

    const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    const fn value(self) -> Uuid {
        self.0
    }

    pub(crate) fn as_text(self) -> String {
        self.0.hyphenated().to_string()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct BindingId(Uuid);

impl BindingId {
    fn new() -> Self {
        Self(Uuid::now_v7())
    }

    const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    const fn value(self) -> Uuid {
        self.0
    }

    pub(crate) fn as_text(self) -> String {
        self.0.hyphenated().to_string()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct BindingSubmissionEpoch(i64);

impl BindingSubmissionEpoch {
    pub(crate) fn new(value: u64) -> Option<Self> {
        i64::try_from(value)
            .ok()
            .filter(|value| *value >= 1)
            .map(Self)
    }

    const fn from_persisted(value: i64) -> Option<Self> {
        if value >= 1 { Some(Self(value)) } else { None }
    }

    const fn value(self) -> i64 {
        self.0
    }

    pub(crate) const fn as_u64(self) -> u64 {
        self.0.cast_unsigned()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BindingInput {
    negotiation_id: BindingNegotiationId,
    submission_epoch: BindingSubmissionEpoch,
    seat_code: String,
}

impl BindingInput {
    pub(crate) fn new(
        negotiation_id: BindingNegotiationId,
        submission_epoch: BindingSubmissionEpoch,
        seat_code: String,
    ) -> Self {
        Self {
            negotiation_id,
            submission_epoch,
            seat_code,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BindingEvaluationCode {
    NotFound,
    Unmapped,
    Occupied,
}

impl BindingEvaluationCode {
    fn from_error_code(value: &str) -> Option<Self> {
        match value {
            "SEAT_NOT_FOUND" => Some(Self::NotFound),
            "SEAT_UNMAPPED" => Some(Self::Unmapped),
            "SEAT_OCCUPIED" => Some(Self::Occupied),
            _ => None,
        }
    }

    const fn as_persisted(self) -> &'static str {
        match self {
            Self::NotFound => "SEAT_NOT_FOUND",
            Self::Unmapped => "SEAT_UNMAPPED",
            Self::Occupied => "SEAT_OCCUPIED",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RejectedSubmission {
    submission_epoch: BindingSubmissionEpoch,
    seat_code: String,
    evaluation_code: BindingEvaluationCode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NegotiationFact {
    negotiation_id: BindingNegotiationId,
    rejected_submission: Option<RejectedSubmission>,
}

impl NegotiationFact {
    const fn new(negotiation_id: BindingNegotiationId) -> Self {
        Self {
            negotiation_id,
            rejected_submission: None,
        }
    }

    fn from_persisted(row: &db::PersistedNegotiationRow) -> Result<Self, BindingError> {
        let rejected_submission = if let Some(submission) = row.rejected_submission.as_ref() {
            Some(RejectedSubmission {
                submission_epoch: BindingSubmissionEpoch::from_persisted(
                    submission.submission_epoch,
                )
                .ok_or(BindingError::InvalidPersistedFacts {
                    source: crate::db::PersistenceError::invalid("persisted facts"),
                })?,
                seat_code: submission.seat_code.clone(),
                evaluation_code: BindingEvaluationCode::from_error_code(
                    &submission.evaluation_error_code,
                )
                .ok_or(BindingError::InvalidPersistedFacts {
                    source: crate::db::PersistenceError::invalid("persisted facts"),
                })?,
            })
        } else {
            None
        };
        Ok(Self {
            negotiation_id: BindingNegotiationId::from_uuid(row.negotiation_id),
            rejected_submission,
        })
    }

    fn into_intent(self) -> BindingNegotiationIntent {
        BindingNegotiationIntent {
            negotiation_id: self.negotiation_id,
            evaluation: self
                .rejected_submission
                .map(|submission| BindingEvaluation {
                    submission_epoch: submission.submission_epoch,
                    error_code: submission.evaluation_code,
                }),
        }
    }
}

enum BindingFact {
    Unbound(NegotiationFact),
    Bound(db::PersistedBoundTargetRow),
}

/// Redacted durable Binding state shown by the Operator Panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BindingProjection {
    Unbound(BindingNegotiationIntent),
    Bound(BindingContext),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BindingNegotiationIntent {
    negotiation_id: BindingNegotiationId,
    evaluation: Option<BindingEvaluation>,
}

impl BindingNegotiationIntent {
    pub(crate) const fn negotiation_id(&self) -> BindingNegotiationId {
        self.negotiation_id
    }

    pub(crate) const fn evaluation(&self) -> Option<&BindingEvaluation> {
        self.evaluation.as_ref()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BindingEvaluation {
    submission_epoch: BindingSubmissionEpoch,
    error_code: BindingEvaluationCode,
}

impl BindingEvaluation {
    pub(crate) const fn submission_epoch(self) -> BindingSubmissionEpoch {
        self.submission_epoch
    }

    pub(crate) const fn error_code(self) -> BindingEvaluationCode {
        self.error_code
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BindingContext {
    binding_id: BindingId,
    account_id: Uuid,
    seat_code: String,
    domjudge_username: String,
    credential_revision: u64,
}

impl BindingContext {
    fn from_persisted(row: &db::PersistedBoundContextRow) -> Result<Self, BindingError> {
        let seat_code = &row.seat_code;
        let domjudge_username = &row.domjudge_username;
        if !valid_public_text(seat_code) || !is_valid_domjudge_username(domjudge_username) {
            return Err(BindingError::InvalidPersistedFacts {
                source: crate::db::PersistenceError::invalid("persisted facts"),
            });
        }
        Ok(Self {
            binding_id: BindingId::from_uuid(row.binding_id),
            account_id: row.account_id,
            seat_code: seat_code.to_owned(),
            domjudge_username: domjudge_username.to_owned(),
            credential_revision: u64::try_from(row.credential_revision)
                .ok()
                .filter(|revision| *revision >= 1)
                .ok_or(BindingError::InvalidPersistedFacts {
                    source: crate::db::PersistenceError::invalid("persisted facts"),
                })?,
        })
    }

    pub(crate) const fn binding_id(&self) -> BindingId {
        self.binding_id
    }

    pub(crate) const fn account_id(&self) -> Uuid {
        self.account_id
    }

    pub(crate) fn seat_code(&self) -> &str {
        &self.seat_code
    }

    pub(crate) fn domjudge_username(&self) -> &str {
        &self.domjudge_username
    }

    pub(crate) const fn credential_revision(&self) -> u64 {
        self.credential_revision
    }
}

pub(crate) struct BindingPassword(Zeroizing<Vec<u8>>);

impl BindingPassword {
    fn new(value: Zeroizing<Vec<u8>>) -> Result<Self, BindingError> {
        let text = std::str::from_utf8(value.as_slice()).map_err(|source| {
            BindingError::InvalidPersistedFacts {
                source: PersistenceError::Utf8 {
                    field: "binding password",
                    source,
                },
            }
        })?;
        if text.is_empty()
            || text.len() > PASSWORD_LENGTH_LIMIT
            || text.chars().any(char::is_control)
        {
            return Err(BindingError::InvalidPersistedFacts {
                source: crate::db::PersistenceError::invalid("persisted facts"),
            });
        }
        Ok(Self(value))
    }

    pub(crate) fn as_bytes(&self) -> &[u8] {
        self.0.as_slice()
    }
}

impl fmt::Debug for BindingPassword {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("BindingPassword([REDACTED])")
    }
}

/// Display facts read in the same transaction as the binding and vault record.
#[derive(Debug)]
pub(crate) struct BindingPresentation {
    pub(crate) organization_id: i64,
    pub(crate) team_name_zh: String,
    pub(crate) team_name_en: String,
    pub(crate) school_name_zh: String,
    pub(crate) school_name_en: String,
}

pub(crate) struct BoundTarget {
    context: BindingContext,
    password: BindingPassword,
    presentation: BindingPresentation,
}

impl BoundTarget {
    pub(crate) const fn presentation(&self) -> &BindingPresentation {
        &self.presentation
    }

    pub(crate) const fn context(&self) -> &BindingContext {
        &self.context
    }

    pub(crate) const fn password(&self) -> &BindingPassword {
        &self.password
    }
}

impl fmt::Debug for BoundTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BoundTarget")
            .field("context", &self.context)
            .field("password", &self.password)
            .field("presentation", &self.presentation)
            .finish()
    }
}

#[derive(Debug)]
pub(crate) struct BindingAccessTarget {
    bound: Option<BoundTarget>,
}

impl BindingAccessTarget {
    pub(crate) const fn bound(&self) -> Option<&BoundTarget> {
        self.bound.as_ref()
    }
}

#[derive(Debug)]
pub(crate) struct MaterializedBinding {
    intent: Option<BindingNegotiationIntent>,
    target: BindingAccessTarget,
}

impl MaterializedBinding {
    pub(crate) const fn intent(&self) -> Option<&BindingNegotiationIntent> {
        self.intent.as_ref()
    }

    pub(crate) const fn target(&self) -> &BindingAccessTarget {
        &self.target
    }
}

fn canonical_uuid_v7(value: &str) -> Option<Uuid> {
    let parsed = Uuid::parse_str(value).ok()?;
    if parsed.hyphenated().to_string() != value
        || parsed.get_version() != Some(Version::SortRand)
        || parsed.get_variant() != Variant::RFC4122
    {
        return None;
    }
    Some(parsed)
}

fn valid_public_text(value: &str) -> bool {
    !value.is_empty() && !value.chars().any(char::is_control)
}

#[derive(Debug, Snafu)]
pub(crate) enum BindingError {
    #[snafu(display("the Device does not exist"))]
    DeviceNotFound,
    #[snafu(display("the Device is not eligible for Binding"))]
    DeviceNotEligible,
    #[snafu(display("the Binding submission conflicts with the persisted submission epoch"))]
    ConflictingSubmission,
    #[snafu(display("persisted Binding facts are invalid"))]
    InvalidPersistedFacts { source: PersistenceError },
    #[snafu(display("Binding persistence failed"))]
    PersistenceFailed { source: PersistenceError },
    #[snafu(display("Binding credential decryption failed"))]
    VaultFailure { source: crate::vault::VaultError },
}

impl BindingError {
    pub(crate) fn is_transient(&self) -> bool {
        matches!(self, Self::PersistenceFailed { source } if source.is_transient())
    }
}

impl From<PersistenceError> for BindingError {
    fn from(source: PersistenceError) -> Self {
        match source {
            source if source.is_invalid_data() => Self::InvalidPersistedFacts { source },
            source => Self::PersistenceFailed { source },
        }
    }
}

#[cfg(test)]
mod tests;
