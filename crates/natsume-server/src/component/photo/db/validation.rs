//! Typed verification-task persistence and immutable object metadata.
use super::{PersistenceError, now};
use crate::{
    db::Transaction,
    diesel_schema::{photo_import_jobs as jobs, photo_objects as objects},
};
use diesel::{ExpressionMethods as _, OptionalExtension as _, QueryDsl as _, RunQueryDsl as _};
use natsume_photo_protocol::{
    Sha256Digest,
    results::{SignedResults, TeamResult},
};
use uuid::Uuid;

pub(in crate::component::photo) fn uploading(
    tx: &mut Transaction<'_>,
) -> Result<Vec<Uuid>, PersistenceError> {
    jobs::table
        .filter(jobs::state.eq("uploading"))
        .order(jobs::created_at_unix_ms)
        .limit(2)
        .select(jobs::import_id)
        .load::<String>(tx.connection())?
        .into_iter()
        .map(|id| {
            id.parse()
                .map_err(|_| PersistenceError::invalid("Photo import ID"))
        })
        .collect()
}

pub(in crate::component::photo) fn pending(
    tx: &mut Transaction<'_>,
) -> Result<Option<(Uuid, u64)>, PersistenceError> {
    jobs::table
        .filter(jobs::state.eq_any(["uploaded", "validating"]))
        .order(jobs::created_at_unix_ms)
        .select((jobs::import_id, jobs::uploaded_bytes))
        .first::<(String, i64)>(tx.connection())
        .optional()?
        .map(|(id, size)| {
            Ok((
                id.parse()
                    .map_err(|_| PersistenceError::invalid("Photo import ID"))?,
                u64::try_from(size).map_err(|_| PersistenceError::invalid("Photo upload size"))?,
            ))
        })
        .transpose()
}

pub(in crate::component::photo) fn claim(
    tx: &mut Transaction<'_>,
    id: Uuid,
) -> Result<(), PersistenceError> {
    let now = now(tx)?;
    let changed = diesel::update(
        jobs::table
            .find(id.to_string())
            .filter(jobs::state.eq_any(["uploaded", "validating"])),
    )
    .set((
        jobs::state.eq("validating"),
        jobs::verified_images.eq(0),
        jobs::updated_at_unix_ms.eq(now),
    ))
    .execute(tx.connection())?;
    if changed != 1 {
        return Err(PersistenceError::invalid("Photo validation claim"));
    }
    Ok(())
}

pub(in crate::component::photo) fn progress(
    tx: &mut Transaction<'_>,
    id: Uuid,
    count: usize,
) -> Result<(), PersistenceError> {
    let now = now(tx)?;
    let count =
        i64::try_from(count).map_err(|_| PersistenceError::invalid("Photo verified count"))?;
    let changed = diesel::update(
        jobs::table
            .find(id.to_string())
            .filter(jobs::state.eq("validating")),
    )
    .set((
        jobs::verified_images.eq(count),
        jobs::updated_at_unix_ms.eq(now),
    ))
    .execute(tx.connection())?;
    if changed != 1 {
        return Err(PersistenceError::invalid("Photo validation progress"));
    }
    Ok(())
}

pub(in crate::component::photo) fn fail(
    tx: &mut Transaction<'_>,
    id: Uuid,
    interrupted: bool,
    code: &str,
) -> Result<(), PersistenceError> {
    let now = now(tx)?;
    let changed = diesel::update(jobs::table.find(id.to_string()).filter(jobs::state.eq_any([
        "uploading",
        "uploaded",
        "validating",
    ])))
    .set((
        jobs::state.eq(if interrupted { "interrupted" } else { "failed" }),
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

pub(in crate::component::photo) fn object(
    tx: &mut Transaction<'_>,
    image: &TeamResult,
) -> Result<(), PersistenceError> {
    let bytes = i64::try_from(image.image_bytes)
        .map_err(|_| PersistenceError::invalid("Photo image bytes"))?;
    let format = match image.image_format {
        natsume_photo_protocol::results::PhotoFormat::Jpeg => "jpeg",
        natsume_photo_protocol::results::PhotoFormat::Png => "png",
    };
    diesel::insert_into(objects::table)
        .values((
            objects::image_sha256.eq(image.image_sha256.as_str()),
            objects::image_format.eq(format),
            objects::image_bytes.eq(bytes),
            objects::width.eq(i64::from(image.width)),
            objects::height.eq(i64::from(image.height)),
        ))
        .on_conflict(objects::image_sha256)
        .do_nothing()
        .execute(tx.connection())?;
    let metadata = objects::table
        .find(image.image_sha256.as_str())
        .select((
            objects::image_format,
            objects::image_bytes,
            objects::width,
            objects::height,
        ))
        .first::<(String, i64, i64, i64)>(tx.connection())?;
    if metadata
        != (
            format.to_owned(),
            bytes,
            i64::from(image.width),
            i64::from(image.height),
        )
    {
        return Err(PersistenceError::invalid("Photo object metadata"));
    }
    Ok(())
}

pub(in crate::component::photo) fn ready(
    tx: &mut Transaction<'_>,
    id: Uuid,
    signed: &SignedResults,
    json: &str,
    hash: &Sha256Digest,
) -> Result<(), PersistenceError> {
    let now = now(tx)?;
    let changed = diesel::update(
        jobs::table
            .find(id.to_string())
            .filter(jobs::state.eq("validating")),
    )
    .set((
        jobs::state.eq("ready"),
        jobs::source_id.eq(signed.manifest.context.source_id.as_str()),
        jobs::initialization_id.eq(signed.manifest.context.initialization_id.as_str()),
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
