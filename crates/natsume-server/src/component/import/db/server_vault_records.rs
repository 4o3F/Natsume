use diesel::{ExpressionMethods, RunQueryDsl};

use crate::{
    db::{PersistenceError, Transaction},
    diesel_schema::server_vault_records,
};

pub(in crate::component::import) fn save(
    transaction: &mut Transaction<'_>,
    account_id: &str,
    nonce: &[u8; 24],
    ciphertext: &[u8],
) -> Result<usize, PersistenceError> {
    diesel::insert_into(server_vault_records::table)
        .values((
            server_vault_records::account_id.eq(account_id),
            server_vault_records::nonce.eq(nonce.as_slice()),
            server_vault_records::ciphertext.eq(ciphertext),
        ))
        .on_conflict(server_vault_records::account_id)
        .do_update()
        .set((
            server_vault_records::nonce.eq(nonce.as_slice()),
            server_vault_records::ciphertext.eq(ciphertext),
        ))
        .execute(transaction.connection())
        .map_err(|_| PersistenceError::OperationFailed)
}
