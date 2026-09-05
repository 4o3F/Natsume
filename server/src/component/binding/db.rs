use diesel::{
    BoolExpressionMethods, ExpressionMethods, JoinOnDsl, NullableExpressionMethods,
    OptionalExtension, QueryDsl, RunQueryDsl, dsl::exists,
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
type PersistedBoundContext = (
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<i64>,
);

pub(in crate::component::binding) struct PersistedNegotiationRow {
    pub(in crate::component::binding) negotiation_id: Uuid,
    pub(in crate::component::binding) rejected_submission: Option<PersistedRejectedSubmissionRow>,
}

pub(in crate::component::binding) struct PersistedRejectedSubmissionRow {
    pub(in crate::component::binding) submission_epoch: i64,
    pub(in crate::component::binding) seat_code: String,
    pub(in crate::component::binding) evaluation_error_code: String,
}

pub(in crate::component::binding) struct PersistedSubmissionSeatRow {
    pub(in crate::component::binding) seat_id: Uuid,
    pub(in crate::component::binding) mapped: bool,
    pub(in crate::component::binding) occupied: bool,
}

pub(in crate::component::binding) struct PersistedBoundTargetRow {
    pub(in crate::component::binding) context: PersistedBoundContextRow,
    pub(in crate::component::binding) nonce: Vec<u8>,
    pub(in crate::component::binding) ciphertext: Vec<u8>,
}

pub(in crate::component::binding) struct PersistedBoundContextRow {
    pub(in crate::component::binding) binding_id: Uuid,
    pub(in crate::component::binding) account_id: Uuid,
    pub(in crate::component::binding) seat_code: String,
    pub(in crate::component::binding) domjudge_username: String,
    pub(in crate::component::binding) credential_revision: i64,
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

pub(in crate::component::binding) fn list_negotiations(
    transaction: &mut Transaction<'_>,
) -> Result<Vec<(String, PersistedNegotiationRow)>, PersistenceError> {
    binding_negotiations::table
        .select((
            binding_negotiations::device_id,
            binding_negotiations::negotiation_id,
            binding_negotiations::submission_epoch,
            binding_negotiations::seat_code,
            binding_negotiations::evaluation_error_code,
        ))
        .load::<(String, String, Option<i64>, Option<String>, Option<String>)>(
            transaction.connection(),
        )
        .map_err(|_| PersistenceError::OperationFailed)?
        .into_iter()
        .map(
            |(device_id, negotiation_id, submission_epoch, seat_code, evaluation_error_code)| {
                parse_negotiation((
                    negotiation_id,
                    submission_epoch,
                    seat_code,
                    evaluation_error_code,
                ))
                .map(|row| (device_id, row))
            },
        )
        .collect()
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

pub(in crate::component::binding) fn list_bound_contexts(
    transaction: &mut Transaction<'_>,
) -> Result<Vec<(String, PersistedBoundContextRow)>, PersistenceError> {
    device_bindings::table
        .left_join(seats::table.on(seats::seat_id.eq(device_bindings::seat_id)))
        .left_join(account_mappings::table.on(account_mappings::seat_id.eq(seats::seat_id)))
        .left_join(accounts::table.on(accounts::account_id.eq(account_mappings::account_id)))
        .select((
            device_bindings::device_id,
            device_bindings::binding_id,
            device_bindings::seat_id,
            seats::seat_code.nullable(),
            account_mappings::account_id.nullable(),
            accounts::domjudge_username.nullable(),
            accounts::credential_revision.nullable(),
        ))
        .load::<PersistedBoundContext>(transaction.connection())
        .map_err(|_| PersistenceError::OperationFailed)?
        .into_iter()
        .map(parse_bound_context)
        .collect()
}

fn parse_bound_context(
    (
        device_id,
        binding_id,
        seat_id,
        seat_code,
        account_id,
        domjudge_username,
        credential_revision,
    ): PersistedBoundContext,
) -> Result<(String, PersistedBoundContextRow), PersistenceError> {
    let binding_id = canonical_uuid_v7(&binding_id)?;
    let _seat_id = canonical_uuid_v7(&seat_id)?;
    let seat_code = seat_code.ok_or(PersistenceError::InvalidPersistedData)?;
    let account_id = canonical_uuid_v7(
        account_id
            .as_deref()
            .ok_or(PersistenceError::InvalidPersistedData)?,
    )?;
    let domjudge_username = domjudge_username.ok_or(PersistenceError::InvalidPersistedData)?;
    let credential_revision = credential_revision.ok_or(PersistenceError::InvalidPersistedData)?;
    if credential_revision < 1 {
        return Err(PersistenceError::InvalidPersistedData);
    }
    Ok((
        device_id,
        PersistedBoundContextRow {
            binding_id,
            account_id,
            seat_code,
            domjudge_username,
            credential_revision,
        },
    ))
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
