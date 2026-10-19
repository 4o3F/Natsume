use super::super::{InvitationSummary, OperatorError, OperatorRole, PasswordResetInspection};
use crate::{
    db::{PersistenceError, Transaction},
    diesel_schema::{operator_invitations as invites, operator_password_resets as resets},
};
use diesel::{ExpressionMethods, OptionalExtension, QueryDsl, RunQueryDsl, sql_types::BigInt};
use uuid::Uuid;

type InvitationRow = (String, String, String, i64, i64);

pub(in crate::component::operator) fn now(
    transaction: &mut Transaction<'_>,
) -> Result<i64, PersistenceError> {
    diesel::select(diesel::dsl::sql::<BigInt>(
        "CAST(unixepoch('subsec') * 1000 AS INTEGER)",
    ))
    .get_result(transaction.connection())
    .map_err(|_| PersistenceError::OperationFailed)
}

fn invitation(row: InvitationRow) -> Result<InvitationSummary, PersistenceError> {
    let (id, role, issuer, created, expires) = row;
    Ok(InvitationSummary {
        invitation_id: Uuid::parse_str(&id).map_err(|_| PersistenceError::InvalidPersistedData)?,
        role: OperatorRole::from_persisted(&role)
            .map_err(|_| PersistenceError::InvalidPersistedData)?,
        issuer_operator_id: Uuid::parse_str(&issuer)
            .map_err(|_| PersistenceError::InvalidPersistedData)?,
        created_at_unix_ms: created,
        expires_at_unix_ms: expires,
    })
}

pub(in crate::component::operator) fn list_invitations(
    transaction: &mut Transaction<'_>,
) -> Result<Vec<InvitationSummary>, PersistenceError> {
    invites::table
        .order((
            invites::created_at_unix_ms.asc(),
            invites::invitation_id.asc(),
        ))
        .select((
            invites::invitation_id,
            invites::role,
            invites::issuer_operator_id,
            invites::created_at_unix_ms,
            invites::expires_at_unix_ms,
        ))
        .load::<InvitationRow>(transaction.connection())
        .map_err(|_| PersistenceError::OperationFailed)?
        .into_iter()
        .map(invitation)
        .collect()
}

pub(in crate::component::operator) fn find_invitation(
    transaction: &mut Transaction<'_>,
    hash: &[u8; 32],
) -> Result<Option<InvitationSummary>, PersistenceError> {
    invites::table
        .filter(invites::token_hash.eq(hash.as_slice()))
        .select((
            invites::invitation_id,
            invites::role,
            invites::issuer_operator_id,
            invites::created_at_unix_ms,
            invites::expires_at_unix_ms,
        ))
        .first::<InvitationRow>(transaction.connection())
        .optional()
        .map_err(|_| PersistenceError::OperationFailed)?
        .map(invitation)
        .transpose()
}

pub(in crate::component::operator) fn invitation_by_id(
    transaction: &mut Transaction<'_>,
    id: Uuid,
) -> Result<InvitationSummary, OperatorError> {
    invites::table
        .filter(invites::invitation_id.eq(id.to_string()))
        .select((
            invites::invitation_id,
            invites::role,
            invites::issuer_operator_id,
            invites::created_at_unix_ms,
            invites::expires_at_unix_ms,
        ))
        .first::<InvitationRow>(transaction.connection())
        .optional()
        .map_err(|_| PersistenceError::OperationFailed)?
        .map(invitation)
        .transpose()?
        .ok_or(OperatorError::LinkUnavailable)
}

pub(in crate::component::operator) fn insert_invitation(
    transaction: &mut Transaction<'_>,
    summary: &InvitationSummary,
    hash: &[u8; 32],
) -> Result<(), PersistenceError> {
    diesel::insert_into(invites::table)
        .values((
            invites::invitation_id.eq(summary.invitation_id.to_string()),
            invites::role.eq(summary.role.as_persisted()),
            invites::issuer_operator_id.eq(summary.issuer_operator_id.to_string()),
            invites::token_hash.eq(hash.as_slice()),
            invites::created_at_unix_ms.eq(summary.created_at_unix_ms),
            invites::expires_at_unix_ms.eq(summary.expires_at_unix_ms),
        ))
        .execute(transaction.connection())
        .map(|_| ())
        .map_err(|_| PersistenceError::OperationFailed)
}

pub(in crate::component::operator) fn delete_invitation(
    transaction: &mut Transaction<'_>,
    id: Uuid,
) -> Result<(), PersistenceError> {
    diesel::delete(invites::table.filter(invites::invitation_id.eq(id.to_string())))
        .execute(transaction.connection())
        .map(|_| ())
        .map_err(|_| PersistenceError::OperationFailed)
}

pub(in crate::component::operator) fn revoke_issued_links(
    transaction: &mut Transaction<'_>,
    issuer: Uuid,
) -> Result<(), PersistenceError> {
    diesel::delete(invites::table.filter(invites::issuer_operator_id.eq(issuer.to_string())))
        .execute(transaction.connection())
        .map_err(|_| PersistenceError::OperationFailed)?;
    diesel::delete(resets::table.filter(resets::issuer_operator_id.eq(issuer.to_string())))
        .execute(transaction.connection())
        .map_err(|_| PersistenceError::OperationFailed)?;
    Ok(())
}

pub(in crate::component::operator) fn delete_target_resets(
    transaction: &mut Transaction<'_>,
    target: Uuid,
) -> Result<(), PersistenceError> {
    diesel::delete(resets::table.filter(resets::operator_id.eq(target.to_string())))
        .execute(transaction.connection())
        .map(|_| ())
        .map_err(|_| PersistenceError::OperationFailed)
}

pub(in crate::component::operator) fn insert_reset(
    transaction: &mut Transaction<'_>,
    reset: &PasswordResetInspection,
    issuer: Uuid,
    hash: &[u8; 32],
    created: i64,
) -> Result<(), PersistenceError> {
    // Replacing the single current authorization is part of the caller's write transaction.
    delete_target_resets(transaction, reset.operator_id)?;
    diesel::insert_into(resets::table)
        .values((
            resets::operator_id.eq(reset.operator_id.to_string()),
            resets::reset_id.eq(reset.reset_id.to_string()),
            resets::issuer_operator_id.eq(issuer.to_string()),
            resets::token_hash.eq(hash.as_slice()),
            resets::created_at_unix_ms.eq(created),
            resets::expires_at_unix_ms.eq(reset.expires_at_unix_ms),
        ))
        .execute(transaction.connection())
        .map(|_| ())
        .map_err(|_| PersistenceError::OperationFailed)
}

pub(in crate::component::operator) fn find_reset(
    transaction: &mut Transaction<'_>,
    hash: &[u8; 32],
) -> Result<Option<(PasswordResetInspection, Uuid)>, OperatorError> {
    let row = resets::table
        .filter(resets::token_hash.eq(hash.as_slice()))
        .select((
            resets::operator_id,
            resets::reset_id,
            resets::issuer_operator_id,
            resets::expires_at_unix_ms,
        ))
        .first::<(String, String, String, i64)>(transaction.connection())
        .optional()
        .map_err(|_| PersistenceError::OperationFailed)?;
    row.map(|(target, id, issuer, expires)| {
        let target =
            Uuid::parse_str(&target).map_err(|_| PersistenceError::InvalidPersistedData)?;
        Ok((
            PasswordResetInspection {
                operator_id: target,
                reset_id: Uuid::parse_str(&id)
                    .map_err(|_| PersistenceError::InvalidPersistedData)?,
                username: super::management::account_name(transaction, target)?,
                expires_at_unix_ms: expires,
            },
            Uuid::parse_str(&issuer).map_err(|_| PersistenceError::InvalidPersistedData)?,
        ))
    })
    .transpose()
}
