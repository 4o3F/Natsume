//! Typed persistence operations; authorization and transitions belong to Photo.
use diesel::{ExpressionMethods as _, OptionalExtension as _, QueryDsl as _, RunQueryDsl as _};

use super::SourceState;

use crate::{
    db::{PersistenceError, Transaction},
    diesel_schema::{
        photo_initializations as initializations, photo_server_metadata as metadata,
        photo_sources as sources,
    },
};

pub(super) struct SourceRecord {
    pub id: String,
    pub name: String,
    pub state: SourceState,
    pub enrollment_id: Option<String>,
    pub public_key: Option<String>,
    pub confirmed: bool,
    pub code_hash: Option<Vec<u8>>,
    pub code_expires: Option<i64>,
    pub token_hash: Option<Vec<u8>>,
    pub token_expires: Option<i64>,
}
type SourceRow = (
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    i32,
    Option<Vec<u8>>,
    Option<i64>,
    Option<Vec<u8>>,
    Option<i64>,
);
fn source_record(row: SourceRow) -> Result<SourceRecord, PersistenceError> {
    let (
        id,
        name,
        state,
        enrollment_id,
        public_key,
        confirmed,
        code_hash,
        code_expires,
        token_hash,
        token_expires,
    ) = row;
    let state = SourceState::from_persisted(&state)?;
    let confirmed = match confirmed {
        0 => false,
        1 => true,
        _ => return Err(PersistenceError::invalid("photo confirmed flag")),
    };
    Ok(SourceRecord {
        id,
        name,
        state,
        enrollment_id,
        public_key,
        confirmed,
        code_hash,
        code_expires,
        token_hash,
        token_expires,
    })
}

pub(super) struct InitializationRecord {
    pub id: String,
    pub json: String,
    pub hash: String,
    pub completion_request: Option<String>,
    pub completed_at: Option<i64>,
}

pub(super) fn now(tx: &mut Transaction<'_>) -> Result<i64, PersistenceError> {
    diesel::select(diesel::dsl::sql::<diesel::sql_types::BigInt>(
        "CAST(unixepoch('subsec') * 1000 AS INTEGER)",
    ))
    .get_result(tx.connection())
    .map_err(PersistenceError::from)
}

pub(super) fn source(
    tx: &mut Transaction<'_>,
    id: &str,
) -> Result<Option<SourceRecord>, PersistenceError> {
    sources::table
        .find(id)
        .select((
            sources::source_id,
            sources::name,
            sources::state,
            sources::enrollment_id,
            sources::public_key,
            sources::confirmed,
            sources::pairing_code_hash,
            sources::pairing_expires_at_unix_ms,
            sources::token_hash,
            sources::token_expires_at_unix_ms,
        ))
        .first::<SourceRow>(tx.connection())
        .optional()
        .map_err(PersistenceError::from)?
        .map(source_record)
        .transpose()
}

pub(super) fn list(tx: &mut Transaction<'_>) -> Result<Vec<SourceRecord>, PersistenceError> {
    sources::table
        .order(sources::source_id)
        .select((
            sources::source_id,
            sources::name,
            sources::state,
            sources::enrollment_id,
            sources::public_key,
            sources::confirmed,
            sources::pairing_code_hash,
            sources::pairing_expires_at_unix_ms,
            sources::token_hash,
            sources::token_expires_at_unix_ms,
        ))
        .load::<SourceRow>(tx.connection())
        .map_err(PersistenceError::from)?
        .into_iter()
        .map(source_record)
        .collect()
}

pub(super) fn insert_source(
    tx: &mut Transaction<'_>,
    id: &str,
    name: &str,
    now: i64,
) -> Result<(), PersistenceError> {
    diesel::insert_into(sources::table)
        .values((
            sources::source_id.eq(id),
            sources::name.eq(name),
            sources::state.eq("enabled"),
            sources::confirmed.eq(0),
            sources::created_at_unix_ms.eq(now),
        ))
        .execute(tx.connection())
        .map_err(PersistenceError::from)?;
    Ok(())
}

pub(super) fn update_state(
    tx: &mut Transaction<'_>,
    id: &str,
    state: &str,
) -> Result<(), PersistenceError> {
    diesel::update(sources::table.find(id))
        .set(sources::state.eq(state))
        .execute(tx.connection())
        .map_err(PersistenceError::from)?;
    Ok(())
}

pub(super) fn issue_code(
    tx: &mut Transaction<'_>,
    id: &str,
    hash: &[u8],
    expires: i64,
) -> Result<(), PersistenceError> {
    diesel::update(sources::table.find(id))
        .set((
            sources::pairing_code_hash.eq(Some(hash)),
            sources::pairing_expires_at_unix_ms.eq(Some(expires)),
            sources::enrollment_id.eq(None::<&str>),
            sources::public_key.eq(None::<&str>),
            sources::token_hash.eq(None::<&[u8]>),
            sources::token_expires_at_unix_ms.eq(None::<i64>),
            sources::confirmed.eq(0),
        ))
        .execute(tx.connection())
        .map_err(PersistenceError::from)?;
    Ok(())
}

pub(super) fn code_source(
    tx: &mut Transaction<'_>,
    hash: &[u8],
) -> Result<Option<String>, PersistenceError> {
    sources::table
        .filter(sources::pairing_code_hash.eq(hash))
        .select(sources::source_id)
        .first(tx.connection())
        .optional()
        .map_err(PersistenceError::from)
}

pub(super) fn bind(
    tx: &mut Transaction<'_>,
    id: &str,
    enrollment: &str,
    key: &str,
    hash: &[u8],
    expires: i64,
) -> Result<(), PersistenceError> {
    diesel::update(sources::table.find(id))
        .set((
            sources::enrollment_id.eq(Some(enrollment)),
            sources::public_key.eq(Some(key)),
            sources::token_hash.eq(Some(hash)),
            sources::token_expires_at_unix_ms.eq(Some(expires)),
            sources::pairing_code_hash.eq(None::<&[u8]>),
            sources::pairing_expires_at_unix_ms.eq(None::<i64>),
        ))
        .execute(tx.connection())
        .map_err(PersistenceError::from)?;
    Ok(())
}

pub(super) fn confirm(tx: &mut Transaction<'_>, id: &str) -> Result<(), PersistenceError> {
    diesel::update(sources::table.find(id))
        .set(sources::confirmed.eq(1))
        .execute(tx.connection())
        .map_err(PersistenceError::from)?;
    Ok(())
}

pub(super) fn dataset_id(
    tx: &mut Transaction<'_>,
    candidate: &str,
) -> Result<String, PersistenceError> {
    diesel::insert_into(metadata::table)
        .values((
            metadata::singleton.eq(1),
            metadata::dataset_id.eq(candidate),
        ))
        .on_conflict(metadata::singleton)
        .do_nothing()
        .execute(tx.connection())
        .map_err(PersistenceError::from)?;
    metadata::table
        .find(1)
        .select(metadata::dataset_id)
        .first(tx.connection())
        .map_err(PersistenceError::from)
}

/// Authentication and source lists need only this flag, never the manifest bytes.
pub(super) fn initialization_completed(
    tx: &mut Transaction<'_>,
    source: &str,
) -> Result<bool, PersistenceError> {
    initializations::table
        .filter(initializations::source_id.eq(source))
        .select(initializations::completed_at_unix_ms.is_not_null())
        .first::<bool>(tx.connection())
        .optional()
        .map(|completed| completed.unwrap_or(false))
        .map_err(PersistenceError::from)
}

pub(super) fn initialization(
    tx: &mut Transaction<'_>,
    source: &str,
) -> Result<Option<InitializationRecord>, PersistenceError> {
    type Row = (String, String, String, Option<String>, Option<i64>);
    initializations::table
        .filter(initializations::source_id.eq(source))
        .select((
            initializations::initialization_id,
            initializations::manifest_json,
            initializations::manifest_sha256,
            initializations::completion_request_id,
            initializations::completed_at_unix_ms,
        ))
        .first::<Row>(tx.connection())
        .optional()
        .map(|row| {
            row.map(
                |(id, json, hash, completion_request, completed_at)| InitializationRecord {
                    id,
                    json,
                    hash,
                    completion_request,
                    completed_at,
                },
            )
        })
        .map_err(PersistenceError::from)
}

pub(super) fn insert_initialization(
    tx: &mut Transaction<'_>,
    source: &str,
    id: &str,
    json: &str,
    hash: &str,
) -> Result<(), PersistenceError> {
    diesel::insert_into(initializations::table)
        .values((
            initializations::initialization_id.eq(id),
            initializations::source_id.eq(source),
            initializations::manifest_json.eq(json),
            initializations::manifest_sha256.eq(hash),
        ))
        .execute(tx.connection())
        .map_err(PersistenceError::from)?;
    Ok(())
}

pub(super) fn complete(
    tx: &mut Transaction<'_>,
    id: &str,
    request: &str,
    now: i64,
) -> Result<(), PersistenceError> {
    diesel::update(initializations::table.find(id))
        .set((
            initializations::completion_request_id.eq(Some(request)),
            initializations::completed_at_unix_ms.eq(Some(now)),
        ))
        .execute(tx.connection())
        .map_err(PersistenceError::from)?;
    Ok(())
}
