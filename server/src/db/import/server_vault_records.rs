use diesel::{ExpressionMethods, OptionalExtension, QueryDsl, RunQueryDsl};

use crate::{
    application::import::{
        CandidateRecord, CurrentAccountProjection, ImportError, ImportPayloadFacts,
        NewAccountFacts, SealedCommitRow,
    },
    db::{Transaction, schema::server_vault_records},
    vault::VaultRecordType,
};

pub(crate) fn insert_import_payload(
    transaction: &mut Transaction<'_>,
    candidate: &CandidateRecord,
    nonce: &[u8; 24],
    ciphertext: &[u8],
) -> Result<usize, ImportError> {
    diesel::insert_into(server_vault_records::table)
        .values((
            server_vault_records::vault_record_id
                .eq(candidate.payload_vault_record_id().to_string()),
            server_vault_records::record_type.eq(VaultRecordType::ImportPayload.as_str()),
            server_vault_records::subject_id.eq(candidate.candidate_id().to_string()),
            server_vault_records::nonce.eq(nonce.as_slice()),
            server_vault_records::ciphertext.eq(ciphertext),
        ))
        .execute(transaction.connection())
        .map_err(|_| ImportError::PersistenceFailure)
}

pub(crate) fn read_import_payload(
    transaction: &mut Transaction<'_>,
    candidate: &CandidateRecord,
) -> Result<Option<ImportPayloadFacts>, ImportError> {
    server_vault_records::table
        .filter(
            server_vault_records::vault_record_id
                .eq(candidate.payload_vault_record_id().to_string()),
        )
        .filter(server_vault_records::record_type.eq(VaultRecordType::ImportPayload.as_str()))
        .filter(server_vault_records::subject_id.eq(candidate.candidate_id().to_string()))
        .select((
            server_vault_records::nonce,
            server_vault_records::ciphertext,
        ))
        .first::<(Vec<u8>, Vec<u8>)>(transaction.connection())
        .optional()
        .map(|row| row.map(|(nonce, ciphertext)| ImportPayloadFacts::new(nonce, ciphertext)))
        .map_err(|_| ImportError::PersistenceFailure)
}

pub(crate) fn delete_import_payload(
    transaction: &mut Transaction<'_>,
    candidate: &CandidateRecord,
) -> Result<usize, ImportError> {
    diesel::delete(
        server_vault_records::table
            .filter(
                server_vault_records::vault_record_id
                    .eq(candidate.payload_vault_record_id().to_string()),
            )
            .filter(server_vault_records::record_type.eq(VaultRecordType::ImportPayload.as_str()))
            .filter(server_vault_records::subject_id.eq(candidate.candidate_id().to_string())),
    )
    .execute(transaction.connection())
    .map_err(|_| ImportError::PersistenceFailure)
}

pub(crate) fn insert_account_credential(
    transaction: &mut Transaction<'_>,
    account: &NewAccountFacts,
    credential: &SealedCommitRow,
) -> Result<usize, ImportError> {
    diesel::insert_into(server_vault_records::table)
        .values((
            server_vault_records::vault_record_id
                .eq(account.credential_vault_record_id().to_string()),
            server_vault_records::record_type.eq(VaultRecordType::AccountCredential.as_str()),
            server_vault_records::subject_id.eq(account.account_id().to_string()),
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
            .filter(server_vault_records::vault_record_id.eq(account.credential_vault_record_id()))
            .filter(
                server_vault_records::record_type.eq(VaultRecordType::AccountCredential.as_str()),
            )
            .filter(server_vault_records::subject_id.eq(account.account_id())),
    )
    .set((
        server_vault_records::nonce.eq(credential.nonce().as_slice()),
        server_vault_records::ciphertext.eq(credential.ciphertext()),
    ))
    .execute(transaction.connection())
    .map_err(|_| ImportError::PersistenceFailure)
}

pub(crate) fn delete_account_credential(
    transaction: &mut Transaction<'_>,
    account: &CurrentAccountProjection,
) -> Result<usize, ImportError> {
    diesel::delete(
        server_vault_records::table
            .filter(server_vault_records::vault_record_id.eq(account.credential_vault_record_id()))
            .filter(
                server_vault_records::record_type.eq(VaultRecordType::AccountCredential.as_str()),
            )
            .filter(server_vault_records::subject_id.eq(account.account_id())),
    )
    .execute(transaction.connection())
    .map_err(|_| ImportError::PersistenceFailure)
}
