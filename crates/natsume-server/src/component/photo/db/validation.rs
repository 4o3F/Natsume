//! Durable validation input, outcomes and Manifest identity checks.
use super::{PersistenceError, UuidSnafu, now};
use crate::{db::Transaction, diesel_schema::photo_import_jobs as jobs};
use diesel::{ExpressionMethods as _, OptionalExtension as _, QueryDsl as _, RunQueryDsl as _};
use natsume_photo_protocol::{Sha256Digest, results::SignedResults};
use snafu::ResultExt as _;
use uuid::Uuid;

pub(in crate::component::photo) fn uploading(
    tx: &mut Transaction<'_>,
) -> Result<Vec<Uuid>, PersistenceError> {
    jobs::table
        .filter(jobs::state.eq("uploading"))
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

pub(in crate::component::photo) fn pending(
    tx: &mut Transaction<'_>,
) -> Result<Option<(Uuid, u64)>, PersistenceError> {
    jobs::table
        .filter(jobs::state.eq("processing"))
        .order(jobs::created_at_unix_ms)
        .select((jobs::import_id, jobs::uploaded_bytes))
        .first::<(String, i64)>(tx.connection())
        .optional()?
        .map(|(id, size)| {
            Ok((
                id.parse().context(UuidSnafu {
                    field: "Photo import ID",
                })?,
                u64::try_from(size).map_err(|_| PersistenceError::invalid("Photo upload size"))?,
            ))
        })
        .transpose()
}

pub(in crate::component::photo) fn fail(
    tx: &mut Transaction<'_>,
    id: Uuid,
    code: &str,
) -> Result<(), PersistenceError> {
    let now = now(tx)?;
    let changed = diesel::update(
        jobs::table
            .find(id.to_string())
            .filter(jobs::state.eq_any(["uploading", "processing"])),
    )
    .set((
        jobs::state.eq("failed"),
        jobs::failure_code.eq(code),
        jobs::updated_at_unix_ms.eq(now),
    ))
    .execute(tx.connection())?;
    if changed != 1 {
        return Err(PersistenceError::invalid("Photo validation failure"));
    }
    Ok(())
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

pub(in crate::component::photo) fn ready(
    tx: &mut Transaction<'_>,
    id: Uuid,
    signed: &SignedResults,
    json: &str,
    hash: &Sha256Digest,
    verified_images: usize,
) -> Result<(), PersistenceError> {
    let now = now(tx)?;
    let changed = diesel::update(
        jobs::table
            .find(id.to_string())
            .filter(jobs::state.eq("processing")),
    )
    .set((
        jobs::state.eq("ready"),
        jobs::verified_images.eq(i64::try_from(verified_images)
            .map_err(|_| PersistenceError::invalid("Photo verified count"))?),
        jobs::manifest_id.eq(signed.manifest.manifest_id.as_str()),
        jobs::manifest_sha256.eq(hash.as_str()),
        jobs::manifest_json.eq(json),
        jobs::failure_code.eq(None::<String>),
        jobs::updated_at_unix_ms.eq(now),
    ))
    .execute(tx.connection())?;
    if changed != 1 {
        return Err(PersistenceError::invalid("Photo validation publication"));
    }
    Ok(())
}
