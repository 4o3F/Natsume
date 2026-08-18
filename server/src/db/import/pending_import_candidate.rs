use diesel::{
    ExpressionMethods, OptionalExtension, QueryDsl, QueryableByName, RunQueryDsl,
    sql_types::{BigInt, Binary, Integer, Text},
};

use crate::{
    application::import::{
        CandidateExpiry, CandidateRecord, ImportError, PendingImportCandidate,
        RedactedImportPreview,
    },
    db::{Transaction, schema::pending_import_candidate},
};

use super::canonical_uuid_v7;

#[derive(QueryableByName)]
struct PersistedCandidateRow {
    #[diesel(sql_type = Text)]
    candidate_id: String,
    #[diesel(sql_type = Text)]
    expires_at: String,
    #[diesel(sql_type = Text)]
    payload_vault_record_id: String,
    #[diesel(sql_type = BigInt)]
    baseline_configuration_revision: i64,
    #[diesel(sql_type = BigInt)]
    baseline_binding_revision: i64,
    #[diesel(sql_type = Binary)]
    preview_token_hash: Vec<u8>,
    #[diesel(sql_type = Text)]
    redacted_preview_json: String,
    #[diesel(sql_type = BigInt)]
    expiry_state: i64,
}

pub(crate) fn find(
    transaction: &mut Transaction<'_>,
) -> Result<Option<CandidateRecord>, ImportError> {
    let row = diesel::sql_query(
        "SELECT candidate_id, expires_at, payload_vault_record_id, \
         baseline_configuration_revision, baseline_binding_revision, preview_token_hash, \
         redacted_preview_json, \
         CASE \
           WHEN strftime('%Y-%m-%dT%H:%M:%fZ', expires_at) IS NULL \
             OR expires_at <> strftime('%Y-%m-%dT%H:%M:%fZ', expires_at) THEN -1 \
           WHEN expires_at <= strftime('%Y-%m-%dT%H:%M:%fZ', 'now') THEN 1 \
           ELSE 0 \
         END AS expiry_state \
         FROM pending_import_candidate WHERE singleton = 1",
    )
    .get_result::<PersistedCandidateRow>(transaction.connection())
    .optional()
    .map_err(|_| ImportError::PersistenceFailure)?;

    row.map(candidate_from_persisted).transpose()
}

pub(crate) fn calculate_expiry(transaction: &mut Transaction<'_>) -> Result<String, ImportError> {
    #[derive(QueryableByName)]
    struct ExpiryRow {
        #[diesel(sql_type = Text)]
        expires_at: String,
    }

    let modifier = format!(
        "+{} seconds",
        crate::application::import::IMPORT_CANDIDATE_TTL_SECONDS
    );
    diesel::sql_query("SELECT strftime('%Y-%m-%dT%H:%M:%fZ', 'now', ?) AS expires_at")
        .bind::<Text, _>(modifier)
        .get_result::<ExpiryRow>(transaction.connection())
        .map(|row| row.expires_at)
        .map_err(|_| ImportError::PersistenceFailure)
}

pub(crate) fn insert(
    transaction: &mut Transaction<'_>,
    candidate: &CandidateRecord,
) -> Result<usize, ImportError> {
    let redacted_preview_json =
        serde_json::to_string(candidate.diff()).map_err(|_| ImportError::PersistenceFailure)?;
    diesel::insert_into(pending_import_candidate::table)
        .values((
            pending_import_candidate::singleton.eq(Some(1_i32)),
            pending_import_candidate::candidate_id.eq(candidate.candidate_id().to_string()),
            pending_import_candidate::expires_at.eq(candidate.expires_at()),
            pending_import_candidate::baseline_configuration_revision
                .eq(diesel::dsl::sql::<Integer>("")
                    .bind::<BigInt, _>(candidate.baseline_configuration_revision())),
            pending_import_candidate::baseline_binding_revision.eq(diesel::dsl::sql::<Integer>("")
                .bind::<BigInt, _>(candidate.baseline_binding_revision())),
            pending_import_candidate::preview_token_hash
                .eq(candidate.preview_token_hash().as_slice()),
            pending_import_candidate::payload_vault_record_id
                .eq(candidate.payload_vault_record_id().to_string()),
            pending_import_candidate::redacted_preview_json.eq(redacted_preview_json),
        ))
        .execute(transaction.connection())
        .map_err(|_| ImportError::PersistenceFailure)
}

pub(crate) fn delete_exact(
    transaction: &mut Transaction<'_>,
    candidate: &CandidateRecord,
) -> Result<usize, ImportError> {
    diesel::delete(
        pending_import_candidate::table
            .filter(pending_import_candidate::singleton.eq(Some(1_i32)))
            .filter(pending_import_candidate::candidate_id.eq(candidate.candidate_id().to_string()))
            .filter(pending_import_candidate::expires_at.eq(candidate.expires_at()))
            .filter(
                pending_import_candidate::baseline_configuration_revision
                    .eq(diesel::dsl::sql::<Integer>("")
                        .bind::<BigInt, _>(candidate.baseline_configuration_revision())),
            )
            .filter(
                pending_import_candidate::baseline_binding_revision
                    .eq(diesel::dsl::sql::<Integer>("")
                        .bind::<BigInt, _>(candidate.baseline_binding_revision())),
            )
            .filter(
                pending_import_candidate::preview_token_hash
                    .eq(candidate.preview_token_hash().as_slice()),
            )
            .filter(
                pending_import_candidate::payload_vault_record_id
                    .eq(candidate.payload_vault_record_id().to_string()),
            ),
    )
    .execute(transaction.connection())
    .map_err(|_| ImportError::PersistenceFailure)
}

fn candidate_from_persisted(row: PersistedCandidateRow) -> Result<CandidateRecord, ImportError> {
    if row.baseline_configuration_revision < 0 || row.baseline_binding_revision < 0 {
        return Err(ImportError::PersistenceFailure);
    }
    let preview_token_hash = row
        .preview_token_hash
        .as_slice()
        .try_into()
        .map_err(|_| ImportError::PersistenceFailure)?;
    let diff = serde_json::from_str::<RedactedImportPreview>(&row.redacted_preview_json)
        .map_err(|_| ImportError::PersistenceFailure)?;
    let expiry = match row.expiry_state {
        0 => CandidateExpiry::Valid,
        1 => CandidateExpiry::Expired,
        _ => return Err(ImportError::PersistenceFailure),
    };
    let pending = PendingImportCandidate::new(
        canonical_uuid_v7(&row.candidate_id)?,
        row.expires_at,
        row.baseline_configuration_revision,
        row.baseline_binding_revision,
        diff,
    );
    Ok(CandidateRecord::new(
        pending,
        preview_token_hash,
        canonical_uuid_v7(&row.payload_vault_record_id)?,
        expiry,
    ))
}
