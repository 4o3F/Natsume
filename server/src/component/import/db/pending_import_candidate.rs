use diesel::{
    ExpressionMethods, OptionalExtension, QueryDsl, QueryableByName, RunQueryDsl, sql_types::BigInt,
};

use crate::{
    component::import::{
        ImportError, RedactedImportPreview,
        candidate::{CandidateExpiry, CandidateRecord},
    },
    db::Transaction,
    diesel_schema::pending_import_candidate,
};

use super::canonical_uuid_v7;

#[derive(QueryableByName)]
struct CurrentTime {
    #[diesel(sql_type = BigInt)]
    value: i64,
}

pub(in crate::component::import) fn current_time_unix_ms(
    transaction: &mut Transaction<'_>,
) -> Result<i64, ImportError> {
    diesel::sql_query("SELECT CAST(unixepoch('subsec') * 1000 AS INTEGER) AS value")
        .get_result::<CurrentTime>(transaction.connection())
        .map(|row| row.value)
        .map_err(|_| ImportError::PersistenceFailure)
}

pub(in crate::component::import) fn find(
    transaction: &mut Transaction<'_>,
) -> Result<Option<CandidateRecord>, ImportError> {
    let row = pending_import_candidate::table
        .select((
            pending_import_candidate::candidate_id,
            pending_import_candidate::expires_at_unix_ms,
            pending_import_candidate::preview_token_hash,
            pending_import_candidate::fingerprint_version,
            pending_import_candidate::candidate_fingerprint_sha256,
            pending_import_candidate::baseline_fingerprint_sha256,
            pending_import_candidate::redacted_preview_json,
        ))
        .filter(pending_import_candidate::singleton.eq(1_i32))
        .first::<(String, i64, Vec<u8>, i32, Vec<u8>, Vec<u8>, String)>(transaction.connection())
        .optional()
        .map_err(|_| ImportError::PersistenceFailure)?;
    let Some((candidate_id, expires_at, token_hash, version, candidate_hash, baseline_hash, json)) =
        row
    else {
        return Ok(None);
    };
    let now = current_time_unix_ms(transaction)?;
    let token_hash = exact_hash(&token_hash)?;
    let candidate_hash = exact_hash(&candidate_hash)?;
    let baseline_hash = exact_hash(&baseline_hash)?;
    let diff = serde_json::from_str::<RedactedImportPreview>(&json)
        .map_err(|_| ImportError::PersistenceFailure)?;
    let expiry = if expires_at <= now {
        CandidateExpiry::Expired
    } else {
        CandidateExpiry::Valid
    };
    Ok(Some(CandidateRecord::new(
        canonical_uuid_v7(&candidate_id)?,
        expires_at,
        token_hash,
        version,
        candidate_hash,
        baseline_hash,
        diff,
        expiry,
    )))
}

pub(in crate::component::import) fn insert(
    transaction: &mut Transaction<'_>,
    candidate: &CandidateRecord,
) -> Result<usize, ImportError> {
    let diff =
        serde_json::to_string(candidate.diff()).map_err(|_| ImportError::PersistenceFailure)?;
    diesel::insert_into(pending_import_candidate::table)
        .values((
            pending_import_candidate::singleton.eq(1_i32),
            pending_import_candidate::candidate_id.eq(candidate.candidate_id().to_string()),
            pending_import_candidate::expires_at_unix_ms.eq(candidate.expires_at_unix_ms()),
            pending_import_candidate::preview_token_hash
                .eq(candidate.preview_token_hash().as_slice()),
            pending_import_candidate::fingerprint_version.eq(candidate.fingerprint_version()),
            pending_import_candidate::candidate_fingerprint_sha256
                .eq(candidate.candidate_fingerprint_sha256().as_slice()),
            pending_import_candidate::baseline_fingerprint_sha256
                .eq(candidate.baseline_fingerprint_sha256().as_slice()),
            pending_import_candidate::redacted_preview_json.eq(diff),
        ))
        .execute(transaction.connection())
        .map_err(|_| ImportError::PersistenceFailure)
}

pub(in crate::component::import) fn delete_exact(
    transaction: &mut Transaction<'_>,
    candidate: &CandidateRecord,
) -> Result<usize, ImportError> {
    diesel::delete(
        pending_import_candidate::table
            .filter(pending_import_candidate::singleton.eq(1_i32))
            .filter(pending_import_candidate::candidate_id.eq(candidate.candidate_id().to_string()))
            .filter(
                pending_import_candidate::preview_token_hash
                    .eq(candidate.preview_token_hash().as_slice()),
            ),
    )
    .execute(transaction.connection())
    .map_err(|_| ImportError::PersistenceFailure)
}

fn exact_hash(value: &[u8]) -> Result<[u8; 32], ImportError> {
    value
        .try_into()
        .map_err(|_| ImportError::PersistenceFailure)
}
