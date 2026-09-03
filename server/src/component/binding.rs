mod db;

use std::{fmt, sync::Arc};

use snafu::Snafu;
use uuid::{Uuid, Variant, Version};
use zeroize::Zeroizing;

use crate::{
    component::device::DeviceId,
    db::{Database, PersistenceError, Transaction, TransactionError},
    vault::VaultSession,
};

const PASSWORD_LENGTH_LIMIT: usize = 512;

pub(crate) struct BindingComponent {
    database: Database,
    vault: Arc<VaultSession>,
}

impl BindingComponent {
    pub(crate) const fn new(database: Database, vault: Arc<VaultSession>) -> Self {
        Self { database, vault }
    }

    pub(crate) async fn ingest(
        &self,
        device_id: DeviceId,
        input: Option<BindingInput>,
    ) -> Result<(), BindingError> {
        let initial_negotiation_id = BindingNegotiationId::new();
        let binding_id = BindingId::new();
        self.database
            .write(move |transaction| {
                require_enabled_device(transaction, &device_id)?;
                let has_binding = db::device_has_binding(transaction, &device_id)?;
                let negotiation = db::find_negotiation(transaction, &device_id)?;
                match (has_binding, negotiation) {
                    (true, Some(_)) => Err(BindingError::InvalidPersistedFacts),
                    (true, None) => Ok(()),
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
            .map_err(TransactionError::into_error)
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
                    (true, Some(_)) => Err(BindingError::InvalidPersistedFacts),
                    (true, None) => db::find_bound_target(transaction, &device_id)?
                        .map(BindingFact::Bound)
                        .ok_or(BindingError::InvalidPersistedFacts),
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
                    (true, Some(_)) => Err(BindingError::InvalidPersistedFacts),
                    (true, None) => db::find_bound_context(transaction, &device_id)?
                        .as_ref()
                        .map(BindingContext::from_persisted)
                        .transpose()?
                        .map(BindingProjection::Bound)
                        .ok_or(BindingError::InvalidPersistedFacts)
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

    fn resolve(&self, fact: BindingFact) -> Result<MaterializedBinding, BindingError> {
        match fact {
            BindingFact::Unbound(negotiation) => Ok(MaterializedBinding {
                intent: Some(negotiation.into_intent()),
                target: BindingAccessTarget { bound: None },
            }),
            BindingFact::Bound(row) => {
                let context = BindingContext::from_persisted(row.context())?;
                let plaintext = self
                    .vault
                    .open(row.nonce(), row.ciphertext())
                    .map_err(|_| BindingError::VaultFailure)?;
                let password = BindingPassword::new(plaintext)?;
                Ok(MaterializedBinding {
                    intent: None,
                    target: BindingAccessTarget {
                        bound: Some(BoundTarget { context, password }),
                    },
                })
            }
        }
    }

    /// Removes the current Binding and starts a fresh negotiation if needed.
    pub(crate) async fn unbind(&self, device_id: DeviceId) -> Result<(), BindingError> {
        let replacement_negotiation_id = BindingNegotiationId::new();
        self.database
            .write(move |transaction| {
                require_existing_device(transaction, &device_id)?;
                let has_binding = db::device_has_binding(transaction, &device_id)?;
                let negotiation = db::find_negotiation(transaction, &device_id)?;
                match (has_binding, negotiation) {
                    (true, Some(_)) => Err(BindingError::InvalidPersistedFacts),
                    (true, None) => {
                        require_one(db::delete_binding(transaction, &device_id)?)?;
                        require_one(db::insert_negotiation(
                            transaction,
                            &device_id,
                            replacement_negotiation_id.value(),
                        )?)
                    }
                    (false, Some(_)) => Ok(()),
                    (false, None) => require_one(db::insert_negotiation(
                        transaction,
                        &device_id,
                        replacement_negotiation_id.value(),
                    )?),
                }
            })
            .await
            .map_err(TransactionError::into_error)
    }
}

fn ingest_submission(
    transaction: &mut Transaction<'_>,
    device_id: &DeviceId,
    negotiation: &NegotiationFact,
    input: Option<&BindingInput>,
    binding_id: BindingId,
) -> Result<(), BindingError> {
    let Some(input) = input else {
        return Ok(());
    };
    if input.negotiation_id != negotiation.negotiation_id {
        return Ok(());
    }
    if let Some(rejected) = negotiation.rejected_submission.as_ref() {
        match input
            .submission_epoch
            .value()
            .cmp(&rejected.submission_epoch.value())
        {
            std::cmp::Ordering::Less => return Ok(()),
            std::cmp::Ordering::Equal if input.seat_code == rejected.seat_code => return Ok(()),
            std::cmp::Ordering::Equal => return Err(BindingError::ConflictingSubmission),
            std::cmp::Ordering::Greater => {}
        }
    }

    let seat = db::find_submission_seat(transaction, &input.seat_code)?;
    let evaluation = match seat.as_ref() {
        None => Some(BindingEvaluationCode::NotFound),
        Some(seat) if !seat.is_mapped() => Some(BindingEvaluationCode::Unmapped),
        Some(seat) if seat.is_occupied() => Some(BindingEvaluationCode::Occupied),
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
        )?);
    }

    let seat = seat.ok_or(BindingError::InvalidPersistedFacts)?;
    require_one(db::insert_binding(
        transaction,
        binding_id.value(),
        device_id,
        seat.seat_id(),
    )?)?;
    require_one(db::delete_negotiation(
        transaction,
        device_id,
        negotiation.negotiation_id.value(),
    )?)
}

fn require_enabled_device(
    transaction: &mut Transaction<'_>,
    device_id: &DeviceId,
) -> Result<(), BindingError> {
    match db::find_device_state(transaction, device_id)?.as_deref() {
        None => Err(BindingError::DeviceNotFound),
        Some("enabled") => Ok(()),
        Some("disabled" | "revoked") => Err(BindingError::DeviceNotEligible),
        Some(_) => Err(BindingError::InvalidPersistedFacts),
    }
}

fn require_existing_device(
    transaction: &mut Transaction<'_>,
    device_id: &DeviceId,
) -> Result<(), BindingError> {
    match db::find_device_state(transaction, device_id)?.as_deref() {
        None => Err(BindingError::DeviceNotFound),
        Some("enabled" | "disabled" | "revoked") => Ok(()),
        Some(_) => Err(BindingError::InvalidPersistedFacts),
    }
}

fn require_one(updated: usize) -> Result<(), BindingError> {
    if updated == 1 {
        Ok(())
    } else {
        Err(BindingError::InvalidPersistedFacts)
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
        let rejected_submission = if let Some(submission) = row.rejected_submission() {
            Some(RejectedSubmission {
                submission_epoch: BindingSubmissionEpoch::from_persisted(
                    submission.submission_epoch(),
                )
                .ok_or(BindingError::InvalidPersistedFacts)?,
                seat_code: submission.seat_code().to_owned(),
                evaluation_code: BindingEvaluationCode::from_error_code(
                    submission.evaluation_error_code(),
                )
                .ok_or(BindingError::InvalidPersistedFacts)?,
            })
        } else {
            None
        };
        Ok(Self {
            negotiation_id: BindingNegotiationId::from_uuid(row.negotiation_id()),
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
        let seat_code = row.seat_code();
        let domjudge_username = row.domjudge_username();
        if !valid_public_text(seat_code) || !valid_public_text(domjudge_username) {
            return Err(BindingError::InvalidPersistedFacts);
        }
        Ok(Self {
            binding_id: BindingId::from_uuid(row.binding_id()),
            account_id: row.account_id(),
            seat_code: seat_code.to_owned(),
            domjudge_username: domjudge_username.to_owned(),
            credential_revision: u64::try_from(row.credential_revision())
                .ok()
                .filter(|revision| *revision >= 1)
                .ok_or(BindingError::InvalidPersistedFacts)?,
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
        let text = std::str::from_utf8(value.as_slice())
            .map_err(|_| BindingError::InvalidPersistedFacts)?;
        if text.is_empty()
            || text.len() > PASSWORD_LENGTH_LIMIT
            || text.chars().any(char::is_control)
        {
            return Err(BindingError::InvalidPersistedFacts);
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

pub(crate) struct BoundTarget {
    context: BindingContext,
    password: BindingPassword,
}

impl BoundTarget {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Snafu)]
pub(crate) enum BindingError {
    #[snafu(display("the Device does not exist"))]
    DeviceNotFound,
    #[snafu(display("the Device is not eligible for Binding"))]
    DeviceNotEligible,
    #[snafu(display("the Binding submission conflicts with the persisted submission epoch"))]
    ConflictingSubmission,
    #[snafu(display("persisted Binding facts are invalid"))]
    InvalidPersistedFacts,
    #[snafu(display("Binding persistence failed"))]
    PersistenceFailed,
    #[snafu(display("Binding credential decryption failed"))]
    VaultFailure,
}

impl From<PersistenceError> for BindingError {
    fn from(error: PersistenceError) -> Self {
        match error {
            PersistenceError::InvalidPersistedData => Self::InvalidPersistedFacts,
            PersistenceError::OperationFailed => Self::PersistenceFailed,
        }
    }
}

#[cfg(test)]
mod tests;
