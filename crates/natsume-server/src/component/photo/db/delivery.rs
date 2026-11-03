//! Current Photo-owned associations and immutable object facts.
use super::super::delivery::PhotoRecord;
use super::{PersistenceError, ValueSnafu};
use crate::{
    db::Transaction,
    diesel_schema::{
        photo_objects as objects, photo_submissions as submissions, team_photos as current,
    },
};
use diesel::{
    BoolExpressionMethods as _, ExpressionMethods as _, JoinOnDsl as _,
    NullableExpressionMethods as _, QueryDsl as _, RunQueryDsl as _,
};
use snafu::ResultExt as _;

pub(in crate::component::photo) fn read(
    tx: &mut Transaction<'_>,
    after: Option<&str>,
    exact: Option<&str>,
    accounts: Option<&[&str]>,
    limit: i64,
) -> Result<Vec<PhotoRecord>, PersistenceError> {
    type Row = (String, String, String, String, i64, String, String, i64);
    let mut query = current::table
        .inner_join(
            submissions::table.on(current::source_id
                .eq(submissions::source_id.nullable())
                .and(current::submission_id.eq(submissions::submission_id.nullable()))),
        )
        .inner_join(objects::table.on(objects::image_sha256.eq(submissions::image_sha256)))
        .filter(submissions::state.eq("active"))
        .select((
            current::account_id,
            current::account,
            submissions::source_id,
            submissions::submission_id,
            current::revision,
            objects::image_sha256,
            objects::image_format,
            objects::image_bytes,
        ))
        .order(current::account_id)
        .limit(limit)
        .into_boxed();
    if let Some(after) = after {
        query = query.filter(current::account_id.gt(after));
    }
    if let Some(exact) = exact {
        query = query.filter(current::account_id.eq(exact));
    }
    if let Some(accounts) = accounts {
        query = query.filter(current::account_id.eq_any(accounts.iter().copied()));
    }
    let rows: Vec<Row> = query.load(tx.connection())?;
    rows.into_iter()
        .map(
            |(account_id, account, source_id, submission_id, revision, hash, format, bytes)| {
                Ok(PhotoRecord {
                    account_id,
                    account,
                    source_id: source_id.parse().context(ValueSnafu {
                        field: "Photo source",
                    })?,
                    submission_id: submission_id.parse().context(ValueSnafu {
                        field: "Photo submission",
                    })?,
                    revision,
                    image_sha256: hash.parse().context(ValueSnafu {
                        field: "Photo object hash",
                    })?,
                    image_format: match format.as_str() {
                        "jpeg" => natsume_photo_protocol::results::PhotoFormat::Jpeg,
                        "png" => natsume_photo_protocol::results::PhotoFormat::Png,
                        _ => return Err(PersistenceError::invalid("Photo object format")),
                    },
                    image_bytes: u64::try_from(bytes)
                        .map_err(|_| PersistenceError::invalid("Photo object size"))?,
                })
            },
        )
        .collect()
}
