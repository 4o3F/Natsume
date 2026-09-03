use diesel::{
    BoolExpressionMethods, ExpressionMethods, OptionalExtension, QueryDsl, RunQueryDsl, dsl::exists,
};
use uuid::{Uuid, Variant, Version};

use crate::{
    component::device::DeviceId,
    db::{PersistenceError, Transaction},
    diesel_schema::{
        account_mappings, accounts, binding_negotiations, device_bindings, devices, seats,
        server_vault_records,
    },
};

type PersistedNegotiation = (String, Option<i64>, Option<String>, Option<String>);

pub(in crate::component::binding) struct PersistedNegotiationRow {
    negotiation_id: Uuid,
    rejected_submission: Option<PersistedRejectedSubmissionRow>,
}

impl PersistedNegotiationRow {
    pub(in crate::component::binding) const fn negotiation_id(&self) -> Uuid {
        self.negotiation_id
    }

    pub(in crate::component::binding) const fn rejected_submission(
        &self,
    ) -> Option<&PersistedRejectedSubmissionRow> {
        self.rejected_submission.as_ref()
    }
}

pub(in crate::component::binding) struct PersistedRejectedSubmissionRow {
    submission_epoch: i64,
    seat_code: String,
    evaluation_error_code: String,
}

impl PersistedRejectedSubmissionRow {
    pub(in crate::component::binding) const fn submission_epoch(&self) -> i64 {
        self.submission_epoch
    }

    pub(in crate::component::binding) fn seat_code(&self) -> &str {
        &self.seat_code
    }

    pub(in crate::component::binding) fn evaluation_error_code(&self) -> &str {
        &self.evaluation_error_code
    }
}

pub(in crate::component::binding) struct PersistedSubmissionSeatRow {
    seat_id: Uuid,
    mapped: bool,
    occupied: bool,
}

impl PersistedSubmissionSeatRow {
    pub(in crate::component::binding) const fn seat_id(&self) -> Uuid {
        self.seat_id
    }

    pub(in crate::component::binding) const fn is_mapped(&self) -> bool {
        self.mapped
    }

    pub(in crate::component::binding) const fn is_occupied(&self) -> bool {
        self.occupied
    }
}

pub(in crate::component::binding) struct PersistedBoundTargetRow {
    context: PersistedBoundContextRow,
    nonce: Vec<u8>,
    ciphertext: Vec<u8>,
}

impl PersistedBoundTargetRow {
    pub(in crate::component::binding) const fn context(&self) -> &PersistedBoundContextRow {
        &self.context
    }

    pub(in crate::component::binding) fn nonce(&self) -> &[u8] {
        &self.nonce
    }

    pub(in crate::component::binding) fn ciphertext(&self) -> &[u8] {
        &self.ciphertext
    }
}

pub(in crate::component::binding) struct PersistedBoundContextRow {
    binding_id: Uuid,
    account_id: Uuid,
    seat_code: String,
    domjudge_username: String,
    credential_revision: i64,
}

impl PersistedBoundContextRow {
    pub(in crate::component::binding) const fn binding_id(&self) -> Uuid {
        self.binding_id
    }

    pub(in crate::component::binding) const fn account_id(&self) -> Uuid {
        self.account_id
    }

    pub(in crate::component::binding) fn seat_code(&self) -> &str {
        &self.seat_code
    }

    pub(in crate::component::binding) fn domjudge_username(&self) -> &str {
        &self.domjudge_username
    }

    pub(in crate::component::binding) const fn credential_revision(&self) -> i64 {
        self.credential_revision
    }
}

pub(in crate::component::binding) fn find_device_state(
    transaction: &mut Transaction<'_>,
    device_id: &DeviceId,
) -> Result<Option<String>, PersistenceError> {
    devices::table
        .select(devices::state)
        .filter(devices::device_id.eq(device_id.as_text()))
        .first::<String>(transaction.connection())
        .optional()
        .map_err(|_| PersistenceError::OperationFailed)
}

pub(in crate::component::binding) fn device_has_binding(
    transaction: &mut Transaction<'_>,
    device_id: &DeviceId,
) -> Result<bool, PersistenceError> {
    diesel::select(exists(
        device_bindings::table.filter(device_bindings::device_id.eq(device_id.as_text())),
    ))
    .get_result::<bool>(transaction.connection())
    .map_err(|_| PersistenceError::OperationFailed)
}

pub(in crate::component::binding) fn find_negotiation(
    transaction: &mut Transaction<'_>,
    device_id: &DeviceId,
) -> Result<Option<PersistedNegotiationRow>, PersistenceError> {
    binding_negotiations::table
        .select((
            binding_negotiations::negotiation_id,
            binding_negotiations::submission_epoch,
            binding_negotiations::seat_code,
            binding_negotiations::evaluation_error_code,
        ))
        .filter(binding_negotiations::device_id.eq(device_id.as_text()))
        .first::<PersistedNegotiation>(transaction.connection())
        .optional()
        .map_err(|_| PersistenceError::OperationFailed)?
        .map(parse_negotiation)
        .transpose()
}

pub(in crate::component::binding) fn insert_negotiation(
    transaction: &mut Transaction<'_>,
    device_id: &DeviceId,
    negotiation_id: Uuid,
) -> Result<usize, PersistenceError> {
    require_uuid_v7(negotiation_id)?;
    diesel::insert_into(binding_negotiations::table)
        .values((
            binding_negotiations::device_id.eq(device_id.as_text()),
            binding_negotiations::negotiation_id.eq(negotiation_id.hyphenated().to_string()),
        ))
        .execute(transaction.connection())
        .map_err(|_| PersistenceError::OperationFailed)
}

pub(in crate::component::binding) fn store_evaluation(
    transaction: &mut Transaction<'_>,
    device_id: &DeviceId,
    negotiation_id: Uuid,
    submission_epoch: i64,
    seat_code: &str,
    evaluation_error_code: &str,
) -> Result<usize, PersistenceError> {
    require_uuid_v7(negotiation_id)?;
    if submission_epoch < 1 {
        return Err(PersistenceError::InvalidPersistedData);
    }
    diesel::update(
        binding_negotiations::table
            .filter(binding_negotiations::device_id.eq(device_id.as_text()))
            .filter(
                binding_negotiations::negotiation_id.eq(negotiation_id.hyphenated().to_string()),
            )
            .filter(
                binding_negotiations::submission_epoch
                    .is_null()
                    .or(binding_negotiations::submission_epoch.lt(submission_epoch)),
            ),
    )
    .set((
        binding_negotiations::submission_epoch.eq(Some(submission_epoch)),
        binding_negotiations::seat_code.eq(Some(seat_code)),
        binding_negotiations::evaluation_error_code.eq(Some(evaluation_error_code)),
    ))
    .execute(transaction.connection())
    .map_err(|_| PersistenceError::OperationFailed)
}

pub(in crate::component::binding) fn find_submission_seat(
    transaction: &mut Transaction<'_>,
    seat_code: &str,
) -> Result<Option<PersistedSubmissionSeatRow>, PersistenceError> {
    let Some(seat_id) = seats::table
        .select(seats::seat_id)
        .filter(seats::seat_code.eq(seat_code))
        .first::<String>(transaction.connection())
        .optional()
        .map_err(|_| PersistenceError::OperationFailed)?
    else {
        return Ok(None);
    };
    let seat_id = canonical_uuid_v7(&seat_id)?;
    let account_id = account_mappings::table
        .select(account_mappings::account_id)
        .filter(account_mappings::seat_id.eq(seat_id.hyphenated().to_string()))
        .first::<String>(transaction.connection())
        .optional()
        .map_err(|_| PersistenceError::OperationFailed)?
        .map(|value| canonical_uuid_v7(&value))
        .transpose()?;
    let occupied = diesel::select(exists(
        device_bindings::table
            .filter(device_bindings::seat_id.eq(seat_id.hyphenated().to_string())),
    ))
    .get_result::<bool>(transaction.connection())
    .map_err(|_| PersistenceError::OperationFailed)?;
    Ok(Some(PersistedSubmissionSeatRow {
        seat_id,
        mapped: account_id.is_some(),
        occupied,
    }))
}

pub(in crate::component::binding) fn delete_negotiation(
    transaction: &mut Transaction<'_>,
    device_id: &DeviceId,
    negotiation_id: Uuid,
) -> Result<usize, PersistenceError> {
    require_uuid_v7(negotiation_id)?;
    diesel::delete(
        binding_negotiations::table
            .filter(binding_negotiations::device_id.eq(device_id.as_text()))
            .filter(
                binding_negotiations::negotiation_id.eq(negotiation_id.hyphenated().to_string()),
            ),
    )
    .execute(transaction.connection())
    .map_err(|_| PersistenceError::OperationFailed)
}

pub(in crate::component::binding) fn insert_binding(
    transaction: &mut Transaction<'_>,
    binding_id: Uuid,
    device_id: &DeviceId,
    seat_id: Uuid,
) -> Result<usize, PersistenceError> {
    require_uuid_v7(binding_id)?;
    require_uuid_v7(seat_id)?;
    diesel::insert_into(device_bindings::table)
        .values((
            device_bindings::binding_id.eq(binding_id.hyphenated().to_string()),
            device_bindings::device_id.eq(device_id.as_text()),
            device_bindings::seat_id.eq(seat_id.hyphenated().to_string()),
        ))
        .execute(transaction.connection())
        .map_err(|_| PersistenceError::OperationFailed)
}

pub(in crate::component::binding) fn delete_binding(
    transaction: &mut Transaction<'_>,
    device_id: &DeviceId,
) -> Result<usize, PersistenceError> {
    diesel::delete(
        device_bindings::table.filter(device_bindings::device_id.eq(device_id.as_text())),
    )
    .execute(transaction.connection())
    .map_err(|_| PersistenceError::OperationFailed)
}

pub(in crate::component::binding) fn find_bound_target(
    transaction: &mut Transaction<'_>,
    device_id: &DeviceId,
) -> Result<Option<PersistedBoundTargetRow>, PersistenceError> {
    let Some(context) = find_bound_context(transaction, device_id)? else {
        return Ok(None);
    };
    let (nonce, ciphertext) = server_vault_records::table
        .select((
            server_vault_records::nonce,
            server_vault_records::ciphertext,
        ))
        .filter(server_vault_records::account_id.eq(context.account_id.hyphenated().to_string()))
        .first::<(Vec<u8>, Vec<u8>)>(transaction.connection())
        .optional()
        .map_err(|_| PersistenceError::OperationFailed)?
        .ok_or(PersistenceError::InvalidPersistedData)?;
    Ok(Some(PersistedBoundTargetRow {
        context,
        nonce,
        ciphertext,
    }))
}

pub(in crate::component::binding) fn find_bound_context(
    transaction: &mut Transaction<'_>,
    device_id: &DeviceId,
) -> Result<Option<PersistedBoundContextRow>, PersistenceError> {
    let Some((binding_id, seat_id, seat_code)) = device_bindings::table
        .inner_join(seats::table)
        .select((
            device_bindings::binding_id,
            seats::seat_id,
            seats::seat_code,
        ))
        .filter(device_bindings::device_id.eq(device_id.as_text()))
        .first::<(String, String, String)>(transaction.connection())
        .optional()
        .map_err(|_| PersistenceError::OperationFailed)?
    else {
        return Ok(None);
    };
    let binding_id = canonical_uuid_v7(&binding_id)?;
    let seat_id = canonical_uuid_v7(&seat_id)?;
    let account_id = account_mappings::table
        .select(account_mappings::account_id)
        .filter(account_mappings::seat_id.eq(seat_id.hyphenated().to_string()))
        .first::<String>(transaction.connection())
        .optional()
        .map_err(|_| PersistenceError::OperationFailed)?
        .ok_or(PersistenceError::InvalidPersistedData)?;
    let account_id = canonical_uuid_v7(&account_id)?;
    let (domjudge_username, credential_revision) = accounts::table
        .select((accounts::domjudge_username, accounts::credential_revision))
        .filter(accounts::account_id.eq(account_id.hyphenated().to_string()))
        .first::<(String, i64)>(transaction.connection())
        .optional()
        .map_err(|_| PersistenceError::OperationFailed)?
        .ok_or(PersistenceError::InvalidPersistedData)?;
    if credential_revision < 1 {
        return Err(PersistenceError::InvalidPersistedData);
    }
    Ok(Some(PersistedBoundContextRow {
        binding_id,
        account_id,
        seat_code,
        domjudge_username,
        credential_revision,
    }))
}

fn parse_negotiation(
    (negotiation_id, submission_epoch, seat_code, evaluation_error_code): PersistedNegotiation,
) -> Result<PersistedNegotiationRow, PersistenceError> {
    let negotiation_id = canonical_uuid_v7(&negotiation_id)?;
    let rejected_submission = match (submission_epoch, seat_code, evaluation_error_code) {
        (None, None, None) => None,
        (Some(submission_epoch), Some(seat_code), Some(evaluation_error_code))
            if submission_epoch >= 1 =>
        {
            Some(PersistedRejectedSubmissionRow {
                submission_epoch,
                seat_code,
                evaluation_error_code,
            })
        }
        _ => return Err(PersistenceError::InvalidPersistedData),
    };
    Ok(PersistedNegotiationRow {
        negotiation_id,
        rejected_submission,
    })
}

fn canonical_uuid_v7(value: &str) -> Result<Uuid, PersistenceError> {
    let parsed = Uuid::parse_str(value).map_err(|_| PersistenceError::InvalidPersistedData)?;
    require_uuid_v7(parsed)?;
    if parsed.hyphenated().to_string() != value {
        return Err(PersistenceError::InvalidPersistedData);
    }
    Ok(parsed)
}

fn require_uuid_v7(value: Uuid) -> Result<(), PersistenceError> {
    if value.get_version() == Some(Version::SortRand) && value.get_variant() == Variant::RFC4122 {
        Ok(())
    } else {
        Err(PersistenceError::InvalidPersistedData)
    }
}
