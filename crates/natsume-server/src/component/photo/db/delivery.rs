//! Current Photo assignments and image metadata, selected before pagination.
use super::super::{PhotoRoster, delivery::PhotoRecord};
use super::{PersistenceError, ValueSnafu};
use crate::{db::Transaction, diesel_schema::photo_submissions as submissions};
use diesel::{ExpressionMethods as _, QueryDsl as _, RunQueryDsl as _};
use snafu::ResultExt as _;

pub(in crate::component::photo) fn read(
    tx: &mut Transaction<'_>,
    after: Option<&str>,
    exact: Option<&str>,
    roster: &PhotoRoster,
    limit: i64,
) -> Result<Vec<PhotoRecord>, PersistenceError> {
    type Row = (Option<String>, String, String, i64, String, String, i64);
    let mut query = submissions::table
        .filter(submissions::state.eq("active"))
        .filter(submissions::assigned_account_id.eq_any(roster.ids()))
        .select((
            submissions::assigned_account_id,
            submissions::source_id,
            submissions::submission_id,
            submissions::revision,
            submissions::image_sha256,
            submissions::image_format,
            submissions::image_bytes,
        ))
        .order(submissions::assigned_account_id)
        .limit(limit)
        .into_boxed();
    if let Some(after) = after {
        query = query.filter(submissions::assigned_account_id.gt(after));
    }
    if let Some(exact) = exact {
        query = query.filter(submissions::assigned_account_id.eq(exact));
    }
    query
        .load::<Row>(tx.connection())?
        .into_iter()
        .map(|(id, source, submission, revision, hash, format, bytes)| {
            let account_id =
                id.ok_or_else(|| PersistenceError::invalid("Photo assigned identity"))?;
            let account = roster
                .account(&account_id)
                .ok_or_else(|| PersistenceError::invalid("Photo assigned identity"))?
                .to_owned();
            Ok(PhotoRecord {
                account_id,
                account,
                source_id: source.parse().context(ValueSnafu {
                    field: "Photo source",
                })?,
                submission_id: submission.parse().context(ValueSnafu {
                    field: "Photo submission",
                })?,
                revision,
                image_sha256: hash.parse().context(ValueSnafu {
                    field: "Photo hash",
                })?,
                image_format: match format.as_str() {
                    "jpeg" => natsume_photo_protocol::results::PhotoFormat::Jpeg,
                    "png" => natsume_photo_protocol::results::PhotoFormat::Png,
                    _ => return Err(PersistenceError::invalid("Photo image format")),
                },
                image_bytes: u64::try_from(bytes)
                    .map_err(|_| PersistenceError::invalid("Photo image bytes"))?,
            })
        })
        .collect()
}
