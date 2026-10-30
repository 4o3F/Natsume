//! Typed persistence operations; authorization and transitions belong to Photo.
use diesel::{ExpressionMethods as _, OptionalExtension as _, QueryDsl as _, RunQueryDsl as _};

use super::SourceState;

use crate::{
    db::{PersistenceError as SharedPersistenceError, Transaction},
    diesel_schema::{
        photo_initializations as initializations, photo_server_metadata as metadata,
        photo_sources as sources,
    },
};

use natsume_photo_protocol::{
    DatasetId, EnrollmentId, InitializationId, InitializationRequestId, Sha256Digest,
    SigningPublicKey, SourceId, ValueError,
};
use snafu::{IntoError as _, ResultExt as _, Snafu};

#[derive(Debug, Snafu)]
pub(crate) enum PersistenceError {
    #[snafu(display("Photo database operation failed"))]
    Database { source: SharedPersistenceError },
    #[snafu(display("Photo persisted value is invalid ({field})"))]
    Value {
        field: &'static str,
        source: ValueError,
    },
}
impl From<SharedPersistenceError> for PersistenceError {
    fn from(source: SharedPersistenceError) -> Self {
        DatabaseSnafu.into_error(source)
    }
}
impl From<diesel::result::Error> for PersistenceError {
    fn from(source: diesel::result::Error) -> Self {
        SharedPersistenceError::from(source).into()
    }
}
impl PersistenceError {
    fn invalid(field: &'static str) -> Self {
        SharedPersistenceError::invalid(field).into()
    }
    pub(crate) fn is_transient(&self) -> bool {
        matches!(self, Self::Database { source } if source.is_transient())
    }
}

pub(super) struct SourceRecord {
    pub id: SourceId,
    pub name: String,
    pub state: SourceState,
    pub enrollment_id: Option<EnrollmentId>,
    pub public_key: Option<SigningPublicKey>,
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
    let state = super::enrollment::source_state_from_persisted(&state)?;
    let confirmed = match confirmed {
        0 => false,
        1 => true,
        _ => return Err(PersistenceError::invalid("photo confirmed flag")),
    };
    Ok(SourceRecord {
        id: id.parse().context(ValueSnafu {
            field: "photo source id",
        })?,
        name,
        state,
        enrollment_id: enrollment_id
            .map(|value| value.parse())
            .transpose()
            .context(ValueSnafu {
                field: "photo enrollment id",
            })?,
        public_key: public_key
            .map(|value| value.parse())
            .transpose()
            .context(ValueSnafu {
                field: "photo signing public key",
            })?,
        confirmed,
        code_hash,
        code_expires,
        token_hash,
        token_expires,
    })
}

pub(super) struct InitializationRecord {
    pub id: InitializationId,
    pub json: String,
    pub hash: Sha256Digest,
    pub completion_request: Option<InitializationRequestId>,
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
    id: &SourceId,
) -> Result<Option<SourceRecord>, PersistenceError> {
    sources::table
        .find(id.as_str())
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
    id: &SourceId,
    name: &str,
    now: i64,
) -> Result<(), PersistenceError> {
    diesel::insert_into(sources::table)
        .values((
            sources::source_id.eq(id.as_str()),
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
    id: &SourceId,
    state: &str,
) -> Result<(), PersistenceError> {
    diesel::update(sources::table.find(id.as_str()))
        .set(sources::state.eq(state))
        .execute(tx.connection())
        .map_err(PersistenceError::from)?;
    Ok(())
}

pub(super) fn issue_code(
    tx: &mut Transaction<'_>,
    id: &SourceId,
    hash: &[u8],
    expires: i64,
) -> Result<(), PersistenceError> {
    diesel::update(sources::table.find(id.as_str()))
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

pub(super) fn discard_uncompleted_initialization(
    tx: &mut Transaction<'_>,
    source: &SourceId,
) -> Result<(), PersistenceError> {
    diesel::delete(
        initializations::table
            .filter(initializations::source_id.eq(source.as_str()))
            .filter(initializations::completed_at_unix_ms.is_null()),
    )
    .execute(tx.connection())
    .map_err(PersistenceError::from)?;
    Ok(())
}

pub(super) fn code_source(
    tx: &mut Transaction<'_>,
    hash: &[u8],
) -> Result<Option<SourceId>, PersistenceError> {
    sources::table
        .filter(sources::pairing_code_hash.eq(hash))
        .select(sources::source_id)
        .first::<String>(tx.connection())
        .optional()
        .map_err(PersistenceError::from)?
        .map(|value| value.parse())
        .transpose()
        .context(ValueSnafu {
            field: "photo source id",
        })
}

pub(super) fn bind(
    tx: &mut Transaction<'_>,
    id: &SourceId,
    enrollment: &EnrollmentId,
    key: &SigningPublicKey,
    hash: &[u8],
    expires: i64,
) -> Result<(), PersistenceError> {
    diesel::update(sources::table.find(id.as_str()))
        .set((
            sources::enrollment_id.eq(Some(enrollment.as_str())),
            sources::public_key.eq(Some(key.as_str())),
            sources::token_hash.eq(Some(hash)),
            sources::token_expires_at_unix_ms.eq(Some(expires)),
            sources::pairing_code_hash.eq(None::<&[u8]>),
            sources::pairing_expires_at_unix_ms.eq(None::<i64>),
        ))
        .execute(tx.connection())
        .map_err(PersistenceError::from)?;
    Ok(())
}

pub(super) fn confirm(tx: &mut Transaction<'_>, id: &SourceId) -> Result<(), PersistenceError> {
    diesel::update(sources::table.find(id.as_str()))
        .set(sources::confirmed.eq(1))
        .execute(tx.connection())
        .map_err(PersistenceError::from)?;
    Ok(())
}

pub(super) fn dataset_id(
    tx: &mut Transaction<'_>,
    candidate: &DatasetId,
) -> Result<DatasetId, PersistenceError> {
    diesel::insert_into(metadata::table)
        .values((
            metadata::singleton.eq(1),
            metadata::dataset_id.eq(candidate.as_str()),
        ))
        .on_conflict(metadata::singleton)
        .do_nothing()
        .execute(tx.connection())
        .map_err(PersistenceError::from)?;
    metadata::table
        .find(1)
        .select(metadata::dataset_id)
        .first::<String>(tx.connection())
        .map_err(PersistenceError::from)?
        .parse()
        .context(ValueSnafu {
            field: "photo dataset id",
        })
}

/// Authentication and source lists need only this flag, never the manifest bytes.
pub(super) fn initialization_completed(
    tx: &mut Transaction<'_>,
    source: &SourceId,
) -> Result<bool, PersistenceError> {
    initializations::table
        .filter(initializations::source_id.eq(source.as_str()))
        .select(initializations::completed_at_unix_ms.is_not_null())
        .first::<bool>(tx.connection())
        .optional()
        .map(|completed| completed.unwrap_or(false))
        .map_err(PersistenceError::from)
}

pub(super) fn initialization(
    tx: &mut Transaction<'_>,
    source: &SourceId,
) -> Result<Option<InitializationRecord>, PersistenceError> {
    type Row = (String, String, String, Option<String>, Option<i64>);
    initializations::table
        .filter(initializations::source_id.eq(source.as_str()))
        .select((
            initializations::initialization_id,
            initializations::manifest_json,
            initializations::manifest_sha256,
            initializations::completion_request_id,
            initializations::completed_at_unix_ms,
        ))
        .first::<Row>(tx.connection())
        .optional()
        .map_err(PersistenceError::from)?
        .map(
            |(id, json, hash, completion_request, completed_at)| -> Result<_, PersistenceError> {
                Ok(InitializationRecord {
                    id: id.parse().context(ValueSnafu {
                        field: "photo initialization id",
                    })?,
                    json,
                    hash: hash.parse().context(ValueSnafu {
                        field: "photo manifest digest",
                    })?,
                    completion_request: completion_request
                        .map(|value| value.parse())
                        .transpose()
                        .context(ValueSnafu {
                        field: "photo completion request id",
                    })?,
                    completed_at,
                })
            },
        )
        .transpose()
}

pub(super) fn insert_initialization(
    tx: &mut Transaction<'_>,
    source: &SourceId,
    id: &InitializationId,
    json: &str,
    hash: &Sha256Digest,
) -> Result<(), PersistenceError> {
    diesel::insert_into(initializations::table)
        .values((
            initializations::initialization_id.eq(id.as_str()),
            initializations::source_id.eq(source.as_str()),
            initializations::manifest_json.eq(json),
            initializations::manifest_sha256.eq(hash.as_str()),
        ))
        .execute(tx.connection())
        .map_err(PersistenceError::from)?;
    Ok(())
}

pub(super) fn complete(
    tx: &mut Transaction<'_>,
    id: &InitializationId,
    request: &InitializationRequestId,
    now: i64,
) -> Result<(), PersistenceError> {
    diesel::update(initializations::table.find(id.as_str()))
        .set((
            initializations::completion_request_id.eq(Some(request.as_str())),
            initializations::completed_at_unix_ms.eq(Some(now)),
        ))
        .execute(tx.connection())
        .map_err(PersistenceError::from)?;
    Ok(())
}
