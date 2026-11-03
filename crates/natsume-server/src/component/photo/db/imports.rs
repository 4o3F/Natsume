//! Upload persistence only; no filesystem or HTTP policy.
use super::{PersistenceError, now};
use crate::component::photo::imports::ImportFailure;
use crate::component::photo::{ImportJob, ImportState};
use crate::{db::Transaction, diesel_schema::photo_import_jobs as jobs};
use diesel::{ExpressionMethods as _, OptionalExtension as _, QueryDsl as _, RunQueryDsl as _};
use uuid::Uuid;

pub(in crate::component::photo) fn active_count(
    tx: &mut Transaction<'_>,
) -> Result<i64, PersistenceError> {
    jobs::table
        .filter(jobs::state.eq_any(["uploading", "uploaded", "validating"]))
        .count()
        .get_result::<i64>(tx.connection())
        .map_err(PersistenceError::from)
}

pub(in crate::component::photo) fn begin(
    tx: &mut Transaction<'_>,
    id: Uuid,
) -> Result<(), PersistenceError> {
    let now = now(tx)?;
    diesel::insert_into(jobs::table)
        .values((
            jobs::import_id.eq(id.to_string()),
            jobs::state.eq("uploading"),
            jobs::created_at_unix_ms.eq(now),
            jobs::updated_at_unix_ms.eq(now),
        ))
        .execute(tx.connection())?;
    Ok(())
}

pub(in crate::component::photo) fn progress(
    tx: &mut Transaction<'_>,
    id: Uuid,
    bytes: u64,
) -> Result<(), PersistenceError> {
    let bytes = i64::try_from(bytes).map_err(|_| PersistenceError::invalid("upload byte count"))?;
    let now = now(tx)?;
    let changed = diesel::update(
        jobs::table
            .find(id.to_string())
            .filter(jobs::state.eq("uploading")),
    )
    .set((
        jobs::uploaded_bytes.eq(bytes),
        jobs::updated_at_unix_ms.eq(now),
    ))
    .execute(tx.connection())?;
    if changed != 1 {
        return Err(PersistenceError::invalid("upload progress state"));
    }
    Ok(())
}

pub(in crate::component::photo) fn finish(
    tx: &mut Transaction<'_>,
    id: Uuid,
    bytes: u64,
    failure: Option<&str>,
) -> Result<(), PersistenceError> {
    let bytes = i64::try_from(bytes).map_err(|_| PersistenceError::invalid("upload byte count"))?;
    let now = now(tx)?;
    let changed = diesel::update(
        jobs::table
            .find(id.to_string())
            .filter(jobs::state.eq("uploading")),
    )
    .set((
        jobs::state.eq(if failure.is_some() {
            "failed"
        } else {
            "uploaded"
        }),
        jobs::uploaded_bytes.eq(bytes),
        jobs::failure_code.eq(failure),
        jobs::updated_at_unix_ms.eq(now),
    ))
    .execute(tx.connection())?;
    if changed != 1 {
        return Err(PersistenceError::invalid("upload completion state"));
    }
    Ok(())
}

pub(in crate::component::photo) fn read(
    tx: &mut Transaction<'_>,
    id: Uuid,
) -> Result<Option<ImportJob>, PersistenceError> {
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
        .map(|(state, uploaded_bytes, verified_images, failure_code)| {
            let state = match state.as_str() {
                "uploading" => ImportState::Uploading,
                "uploaded" => ImportState::Uploaded,
                "validating" => ImportState::Validating,
                "ready" => ImportState::Ready,
                "committed" => ImportState::Committed,
                "failed" => ImportState::Failed,
                "interrupted" => ImportState::Interrupted,
                _ => return Err(PersistenceError::invalid("Photo import state")),
            };
            let failure_code = failure_code
                .map(|code| match code.as_str() {
                    "limit" => Ok(ImportFailure::Limit),
                    "input" => Ok(ImportFailure::Input),
                    "storage" => Ok(ImportFailure::Storage),
                    _ => Err(PersistenceError::invalid("Photo import failure")),
                })
                .transpose()?;
            Ok(ImportJob {
                import_id: id,
                state,
                uploaded_bytes,
                verified_images,
                failure_code,
            })
        })
        .transpose()
}
