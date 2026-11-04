//! The result-import job lifecycle; all transitions are persisted here.
use super::{PersistenceError, UuidSnafu, now};
use crate::{
    component::photo::imports::{ResultImportFailure, ResultImportJob, ResultImportState},
    db::Transaction,
    diesel_schema::photo_import_jobs as jobs,
};
use diesel::{ExpressionMethods as _, OptionalExtension as _, QueryDsl as _, RunQueryDsl as _};
use natsume_photo_protocol::{Sha256Digest, results::SignedResults};
use snafu::ResultExt as _;
use uuid::Uuid;

pub(in crate::component::photo) fn active_count(
    tx: &mut Transaction<'_>,
) -> Result<i64, PersistenceError> {
    jobs::table
        .filter(jobs::state.eq_any([
            ResultImportState::Uploading.as_str(),
            ResultImportState::Processing.as_str(),
        ]))
        .count()
        .get_result(tx.connection())
        .map_err(PersistenceError::from)
}

pub(in crate::component::photo) fn begin_upload(
    tx: &mut Transaction<'_>,
    id: Uuid,
) -> Result<(), PersistenceError> {
    let now = now(tx)?;
    diesel::insert_into(jobs::table)
        .values((
            jobs::import_id.eq(id.to_string()),
            jobs::state.eq(ResultImportState::Uploading.as_str()),
            jobs::created_at_unix_ms.eq(now),
            jobs::updated_at_unix_ms.eq(now),
        ))
        .execute(tx.connection())?;
    Ok(())
}

pub(in crate::component::photo) fn finish_upload(
    tx: &mut Transaction<'_>,
    id: Uuid,
    bytes: u64,
) -> Result<(), PersistenceError> {
    let bytes = i64::try_from(bytes).map_err(|_| PersistenceError::invalid("upload byte count"))?;
    let now = now(tx)?;
    let changed = diesel::update(
        jobs::table
            .find(id.to_string())
            .filter(jobs::state.eq(ResultImportState::Uploading.as_str())),
    )
    .set((
        jobs::state.eq(ResultImportState::Processing.as_str()),
        jobs::uploaded_bytes.eq(bytes),
        jobs::updated_at_unix_ms.eq(now),
    ))
    .execute(tx.connection())?;
    changed_once(changed, "upload completion state")
}

pub(in crate::component::photo) fn mark_failed(
    tx: &mut Transaction<'_>,
    id: Uuid,
    failure: ResultImportFailure,
    bytes: Option<u64>,
) -> Result<(), PersistenceError> {
    let bytes = bytes
        .map(i64::try_from)
        .transpose()
        .map_err(|_| PersistenceError::invalid("upload byte count"))?;
    let now = now(tx)?;
    let changed = diesel::update(jobs::table.find(id.to_string()).filter(jobs::state.eq_any([
        ResultImportState::Uploading.as_str(),
        ResultImportState::Processing.as_str(),
    ])))
    .set((
        jobs::state.eq(ResultImportState::Failed.as_str()),
        jobs::failure_code.eq(failure.as_str()),
        jobs::updated_at_unix_ms.eq(now),
        bytes.map(|bytes| jobs::uploaded_bytes.eq(bytes)),
    ))
    .execute(tx.connection())?;
    changed_once(changed, "Photo import failure state")
}

pub(in crate::component::photo) fn read(
    tx: &mut Transaction<'_>,
    id: Uuid,
) -> Result<Option<ResultImportJob>, PersistenceError> {
    type Row = (String, i64, i64, Option<String>);
    jobs::table
        .find(id.to_string())
        .select((
            jobs::state,
            jobs::uploaded_bytes,
            jobs::verified_images,
            jobs::failure_code,
        ))
        .first::<Row>(tx.connection())
        .optional()?
        .map(|(state, uploaded_bytes, verified_images, failure)| {
            Ok(ResultImportJob {
                import_id: id,
                uploaded_bytes,
                verified_images,
                state: ResultImportState::parse(&state)
                    .ok_or_else(|| PersistenceError::invalid("Photo import state"))?,
                failure_code: failure
                    .map(|value| {
                        ResultImportFailure::parse(&value)
                            .ok_or_else(|| PersistenceError::invalid("Photo import failure"))
                    })
                    .transpose()?,
            })
        })
        .transpose()
}

pub(in crate::component::photo) fn load_manifest(
    tx: &mut Transaction<'_>,
    id: Uuid,
) -> Result<Option<(ResultImportState, Option<String>)>, PersistenceError> {
    jobs::table
        .find(id.to_string())
        .select((jobs::state, jobs::manifest_json))
        .first::<(String, Option<String>)>(tx.connection())
        .optional()?
        .map(|(state, json)| {
            Ok((
                ResultImportState::parse(&state)
                    .ok_or_else(|| PersistenceError::invalid("Photo import state"))?,
                json,
            ))
        })
        .transpose()
}

pub(in crate::component::photo) fn interrupted_uploads(
    tx: &mut Transaction<'_>,
) -> Result<Vec<Uuid>, PersistenceError> {
    jobs::table
        .filter(jobs::state.eq(ResultImportState::Uploading.as_str()))
        .order(jobs::created_at_unix_ms)
        .select(jobs::import_id)
        .load::<String>(tx.connection())?
        .into_iter()
        .map(|id| {
            id.parse().context(UuidSnafu {
                field: "Photo import ID",
            })
        })
        .collect()
}

pub(in crate::component::photo) fn next_processing(
    tx: &mut Transaction<'_>,
) -> Result<Option<(Uuid, u64)>, PersistenceError> {
    jobs::table
        .filter(jobs::state.eq(ResultImportState::Processing.as_str()))
        .order(jobs::created_at_unix_ms)
        .select((jobs::import_id, jobs::uploaded_bytes))
        .first::<(String, i64)>(tx.connection())
        .optional()?
        .map(|(id, bytes)| {
            Ok((
                id.parse().context(UuidSnafu {
                    field: "Photo import ID",
                })?,
                u64::try_from(bytes).map_err(|_| PersistenceError::invalid("Photo upload size"))?,
            ))
        })
        .transpose()
}

pub(in crate::component::photo) fn manifest_conflicts(
    tx: &mut Transaction<'_>,
    signed: &SignedResults,
    hash: &Sha256Digest,
) -> Result<bool, PersistenceError> {
    jobs::table
        .filter(jobs::manifest_id.eq(signed.manifest.manifest_id.as_str()))
        .filter(jobs::manifest_sha256.ne(hash.as_str()))
        .count()
        .get_result::<i64>(tx.connection())
        .map(|count| count != 0)
        .map_err(PersistenceError::from)
}

pub(in crate::component::photo) fn mark_ready(
    tx: &mut Transaction<'_>,
    id: Uuid,
    signed: &SignedResults,
    json: &str,
    hash: &Sha256Digest,
    images: usize,
) -> Result<(), PersistenceError> {
    let now = now(tx)?;
    let changed = diesel::update(
        jobs::table
            .find(id.to_string())
            .filter(jobs::state.eq(ResultImportState::Processing.as_str())),
    )
    .set((
        jobs::state.eq(ResultImportState::Ready.as_str()),
        jobs::verified_images
            .eq(i64::try_from(images)
                .map_err(|_| PersistenceError::invalid("Photo verified count"))?),
        jobs::manifest_id.eq(signed.manifest.manifest_id.as_str()),
        jobs::manifest_sha256.eq(hash.as_str()),
        jobs::manifest_json.eq(json),
        jobs::failure_code.eq(None::<String>),
        jobs::updated_at_unix_ms.eq(now),
    ))
    .execute(tx.connection())?;
    changed_once(changed, "Photo validation publication")
}

pub(in crate::component::photo) fn mark_committed(
    tx: &mut Transaction<'_>,
    id: Uuid,
) -> Result<(), PersistenceError> {
    let now = now(tx)?;
    let changed = diesel::update(
        jobs::table
            .find(id.to_string())
            .filter(jobs::state.eq(ResultImportState::Ready.as_str())),
    )
    .set((
        jobs::state.eq(ResultImportState::Committed.as_str()),
        jobs::updated_at_unix_ms.eq(now),
    ))
    .execute(tx.connection())?;
    changed_once(changed, "Photo import commit")
}

fn changed_once(changed: usize, field: &'static str) -> Result<(), PersistenceError> {
    if changed == 1 {
        Ok(())
    } else {
        Err(PersistenceError::invalid(field))
    }
}
