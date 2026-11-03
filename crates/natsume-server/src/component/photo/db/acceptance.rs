//! Photo persistence; decisions and roster semantics stay in the owning component.
use super::super::{
    PhotoRoster,
    acceptance::{CurrentPhoto, PendingPhoto, SavedResult, SubmissionState},
};
use super::{PersistenceError, UuidSnafu, ValueSnafu, now};
use crate::{
    db::Transaction,
    diesel_schema::{photo_import_jobs as jobs, photo_submissions as submissions},
};
use diesel::{
    BoolExpressionMethods as _, ExpressionMethods as _, OptionalExtension as _, QueryDsl as _,
    RunQueryDsl as _,
};
use natsume_photo_protocol::{
    SourceId,
    results::{SignedResults, SubmissionId, TeamResult},
};
use snafu::ResultExt as _;
use uuid::Uuid;

type PackageRow = (String, Option<String>, Option<String>);

pub(in crate::component::photo) fn pending(
    tx: &mut Transaction<'_>,
    after: Option<(&SourceId, &SubmissionId)>,
    limit: i64,
    roster: &PhotoRoster,
) -> Result<Vec<PendingPhoto>, PersistenceError> {
    type Row = (
        String,
        String,
        String,
        String,
        String,
        Option<String>,
        String,
        String,
        i64,
    );
    let mut query = submissions::table
        .filter(
            submissions::state
                .eq_any(["pending_review", "needs_association"])
                .or(submissions::state.eq("active").and(
                    submissions::assigned_account_id
                        .is_null()
                        .or(submissions::assigned_account_id.ne_all(roster.ids())),
                )),
        )
        .select((
            submissions::source_id,
            submissions::submission_id,
            submissions::import_id,
            submissions::account_id,
            submissions::account,
            submissions::assigned_account_id,
            submissions::image_sha256,
            submissions::state,
            submissions::revision,
        ))
        .order((submissions::source_id, submissions::submission_id))
        .limit(limit)
        .into_boxed();
    if let Some((source, submission)) = after {
        query = query.filter(
            submissions::source_id
                .gt(source.as_str())
                .or(submissions::source_id
                    .eq(source.as_str())
                    .and(submissions::submission_id.gt(submission.as_str()))),
        );
    }
    query
        .load::<Row>(tx.connection())?
        .into_iter()
        .map(
            |(
                source,
                submission,
                import,
                account_id,
                account,
                assigned_account_id,
                hash,
                state,
                revision,
            )| {
                Ok(PendingPhoto {
                    source_id: source.parse().context(ValueSnafu {
                        field: "Photo source",
                    })?,
                    submission_id: submission.parse().context(ValueSnafu {
                        field: "Photo submission",
                    })?,
                    import_id: import.parse().context(UuidSnafu {
                        field: "Photo import",
                    })?,
                    account_id,
                    account,
                    assigned_account_id,
                    image_sha256: hash.parse().context(ValueSnafu {
                        field: "Photo hash",
                    })?,
                    state: state_value(&state)?,
                    revision,
                })
            },
        )
        .collect()
}

pub(in crate::component::photo) fn package(
    tx: &mut Transaction<'_>,
    id: Uuid,
) -> Result<Option<PackageRow>, PersistenceError> {
    jobs::table
        .find(id.to_string())
        .select((jobs::state, jobs::manifest_json, jobs::manifest_sha256))
        .first(tx.connection())
        .optional()
        .map_err(PersistenceError::from)
}

pub(in crate::component::photo) fn matching(
    tx: &mut Transaction<'_>,
    signed: &SignedResults,
    image: &TeamResult,
) -> Result<Vec<SavedResult>, PersistenceError> {
    type Row = (
        String,
        String,
        String,
        String,
        Option<String>,
        String,
        String,
        i64,
    );
    submissions::table
        .filter(submissions::source_id.eq(signed.manifest.context.source_id.as_str()))
        .filter(
            submissions::submission_id
                .eq(image.submission_id.as_str())
                .or(submissions::result_id.eq(image.result_id.as_str())),
        )
        .select((
            submissions::submission_id,
            submissions::result_id,
            submissions::account_id,
            submissions::account,
            submissions::assigned_account_id,
            submissions::image_sha256,
            submissions::state,
            submissions::revision,
        ))
        .limit(2)
        .load::<Row>(tx.connection())?
        .into_iter()
        .map(
            |(
                submission,
                result,
                account_id,
                account,
                assigned_account_id,
                hash,
                state,
                revision,
            )| {
                Ok(SavedResult {
                    submission_id: submission.parse().context(ValueSnafu {
                        field: "Photo submission",
                    })?,
                    result_id: result.parse().context(ValueSnafu {
                        field: "Photo result",
                    })?,
                    account_id,
                    account,
                    assigned_account_id,
                    image_sha256: hash.parse().context(ValueSnafu {
                        field: "Photo hash",
                    })?,
                    state: state_value(&state)?,
                    revision,
                })
            },
        )
        .collect()
}

fn state_value(raw: &str) -> Result<SubmissionState, PersistenceError> {
    match raw {
        "active" => Ok(SubmissionState::Active),
        "pending_review" => Ok(SubmissionState::PendingReview),
        "needs_association" => Ok(SubmissionState::NeedsAssociation),
        "rejected" => Ok(SubmissionState::Rejected),
        "superseded" => Ok(SubmissionState::Superseded),
        _ => Err(PersistenceError::invalid("Photo submission state")),
    }
}

pub(in crate::component::photo) fn current(
    tx: &mut Transaction<'_>,
    account_id: &str,
) -> Result<Option<CurrentPhoto>, PersistenceError> {
    type Row = (String, String, i64, String);
    let row = submissions::table
        .filter(submissions::assigned_account_id.eq(account_id))
        .filter(submissions::state.eq("active"))
        .select((
            submissions::source_id,
            submissions::submission_id,
            submissions::revision,
            submissions::image_sha256,
        ))
        .first::<Row>(tx.connection())
        .optional()?;
    row.map(|(source, submission, revision, hash)| {
        Ok(CurrentPhoto {
            source_id: source.parse().context(ValueSnafu {
                field: "Photo current source",
            })?,
            submission_id: submission.parse().context(ValueSnafu {
                field: "Photo current submission",
            })?,
            revision,
            image_sha256: hash.parse().context(ValueSnafu {
                field: "Photo current hash",
            })?,
        })
    })
    .transpose()
}

pub(in crate::component::photo) fn insert(
    tx: &mut Transaction<'_>,
    id: Uuid,
    signed: &SignedResults,
    image: &TeamResult,
    state: SubmissionState,
    assigned: Option<&str>,
) -> Result<(), PersistenceError> {
    diesel::insert_into(submissions::table)
        .values((
            submissions::source_id.eq(signed.manifest.context.source_id.as_str()),
            submissions::submission_id.eq(image.submission_id.as_str()),
            submissions::result_id.eq(image.result_id.as_str()),
            submissions::import_id.eq(id.to_string()),
            submissions::account_id.eq(&image.account_id),
            submissions::account.eq(&image.account),
            submissions::assigned_account_id.eq(assigned),
            submissions::image_sha256.eq(image.image_sha256.as_str()),
            submissions::image_format.eq(match image.image_format {
                natsume_photo_protocol::results::PhotoFormat::Jpeg => "jpeg",
                natsume_photo_protocol::results::PhotoFormat::Png => "png",
            }),
            submissions::image_bytes.eq(i64::try_from(image.image_bytes)
                .map_err(|_| PersistenceError::invalid("Photo image bytes"))?),
            submissions::state.eq(state.as_str()),
            submissions::revision.eq(1_i64),
        ))
        .execute(tx.connection())?;
    Ok(())
}

pub(in crate::component::photo) fn transition(
    tx: &mut Transaction<'_>,
    source: &SourceId,
    submission: &SubmissionId,
    expected: i64,
    state: SubmissionState,
    assigned: Option<&str>,
) -> Result<(), PersistenceError> {
    let next = expected
        .checked_add(1)
        .ok_or_else(|| PersistenceError::invalid("Photo submission revision"))?;
    let count = diesel::update(
        submissions::table
            .find((source.as_str(), submission.as_str()))
            .filter(submissions::revision.eq(expected)),
    )
    .set((
        submissions::state.eq(state.as_str()),
        submissions::revision.eq(next),
        submissions::assigned_account_id.eq(assigned),
    ))
    .execute(tx.connection())?;
    if count != 1 {
        return Err(PersistenceError::invalid("Photo submission transition"));
    }
    Ok(())
}

pub(in crate::component::photo) fn committed(
    tx: &mut Transaction<'_>,
    id: Uuid,
) -> Result<(), PersistenceError> {
    let now = now(tx)?;
    let count = diesel::update(
        jobs::table
            .find(id.to_string())
            .filter(jobs::state.eq("ready")),
    )
    .set((
        jobs::state.eq("committed"),
        jobs::updated_at_unix_ms.eq(now),
    ))
    .execute(tx.connection())?;
    if count != 1 {
        return Err(PersistenceError::invalid("Photo import commit"));
    }
    Ok(())
}
