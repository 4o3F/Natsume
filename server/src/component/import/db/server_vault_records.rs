use diesel::{ExpressionMethods, QueryDsl, RunQueryDsl};
use uuid::Uuid;

use crate::{
    component::import::candidate::SealedCommitRow,
    db::{PersistenceError, Transaction},
    diesel_schema::server_vault_records,
};

use super::super::baseline::BaselineAccount;

pub(in crate::component::import) fn insert_account_credential(
    transaction: &mut Transaction<'_>,
    account_id: Uuid,
    credential: &SealedCommitRow,
) -> Result<usize, PersistenceError> {
    diesel::insert_into(server_vault_records::table)
        .values((
            server_vault_records::account_id.eq(account_id.to_string()),
            server_vault_records::nonce.eq(credential.nonce().as_slice()),
            server_vault_records::ciphertext.eq(credential.ciphertext()),
        ))
        .execute(transaction.connection())
        .map_err(|_| PersistenceError::OperationFailed)
}

pub(in crate::component::import) fn update_account_credential(
    transaction: &mut Transaction<'_>,
    account: &BaselineAccount,
    credential: &SealedCommitRow,
) -> Result<usize, PersistenceError> {
    diesel::update(
        server_vault_records::table
            .filter(server_vault_records::account_id.eq(account.account_id())),
    )
    .set((
        server_vault_records::nonce.eq(credential.nonce().as_slice()),
        server_vault_records::ciphertext.eq(credential.ciphertext()),
    ))
    .execute(transaction.connection())
    .map_err(|_| PersistenceError::OperationFailed)
}
