use diesel::{ExpressionMethods, QueryDsl, RunQueryDsl};

use crate::{
    component::{
        contest::{CurrentAccountProjection, NewAccountFacts},
        import::{ImportError, candidate::SealedCommitRow},
    },
    db::Transaction,
    schema::server_vault_records,
};

pub(crate) fn insert_account_credential(
    transaction: &mut Transaction<'_>,
    account: &NewAccountFacts,
    credential: &SealedCommitRow,
) -> Result<usize, ImportError> {
    diesel::insert_into(server_vault_records::table)
        .values((
            server_vault_records::account_id.eq(account.account_id().to_string()),
            server_vault_records::nonce.eq(credential.nonce().as_slice()),
            server_vault_records::ciphertext.eq(credential.ciphertext()),
        ))
        .execute(transaction.connection())
        .map_err(|_| ImportError::PersistenceFailure)
}

pub(crate) fn update_account_credential(
    transaction: &mut Transaction<'_>,
    account: &CurrentAccountProjection,
    credential: &SealedCommitRow,
) -> Result<usize, ImportError> {
    diesel::update(
        server_vault_records::table
            .filter(server_vault_records::account_id.eq(account.account_id())),
    )
    .set((
        server_vault_records::nonce.eq(credential.nonce().as_slice()),
        server_vault_records::ciphertext.eq(credential.ciphertext()),
    ))
    .execute(transaction.connection())
    .map_err(|_| ImportError::PersistenceFailure)
}
