//! Photo persistence only; acceptance policy lives in the parent component.
use super::super::acceptance::{CurrentPhoto, PendingPhoto, SavedResult, SubmissionState};
use super::{PersistenceError, now};
use crate::{
    db::Transaction,
    diesel_schema::{
        photo_import_jobs as jobs, photo_submissions as submissions, team_photos as current,
    },
};
use diesel::{
    BoolExpressionMethods as _, ExpressionMethods as _, JoinOnDsl as _,
    NullableExpressionMethods as _, OptionalExtension as _, QueryDsl as _, RunQueryDsl as _,
};
use natsume_photo_protocol::results::{SignedResults, TeamResult};
use uuid::Uuid;

type PackageRow = (String, Option<String>, Option<String>);

pub(in crate::component::photo) fn pending(
    tx: &mut Transaction<'_>,
    after: Option<(&str, &str)>,
    limit: i64,
) -> Result<Vec<PendingPhoto>, PersistenceError> {
    type Row = (String, String, String, String, String, String, String, i64);
    // Each state uses the existing (state, source_id, submission_id) index. Merge
    // two bounded pages rather than sorting the entire pending history in SQL.
    let mut rows: Vec<Row> = Vec::new();
    for state in ["pending_review", "needs_association", "active"] {
        let mut query = submissions::table
            .filter(submissions::state.eq(state))
            .select((
                submissions::source_id,
                submissions::submission_id,
                submissions::import_id,
                submissions::account_id,
                submissions::account,
                submissions::image_sha256,
                submissions::state,
                submissions::revision,
            ))
            .order((submissions::source_id, submissions::submission_id))
            .limit(limit)
            .into_boxed();
        if let Some((source, submission)) = after {
            query = query.filter(
                submissions::source_id.gt(source).or(submissions::source_id
                    .eq(source)
                    .and(submissions::submission_id.gt(submission))),
            );
        }
        rows.extend(query.load::<Row>(tx.connection())?);
    }
    rows.sort_unstable_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
    let limit = usize::try_from(limit)
        .map_err(|_| PersistenceError::invalid("Photo pending page limit"))?;
    rows.into_iter()
        .take(limit)
        .map(
            |(
                source_id,
                submission_id,
                import_id,
                account_id,
                account,
                image_sha256,
                state,
                revision,
            )| {
                Ok(PendingPhoto {
                    source_id,
                    submission_id,
                    import_id: import_id
                        .parse()
                        .map_err(|_| PersistenceError::invalid("Photo pending import ID"))?,
                    account_id,
                    account,
                    image_sha256,
                    state: state_value(&state)?,
                    revision,
                })
            },
        )
        .collect()
}

pub(in crate::component::photo) fn active_account(
    tx: &mut Transaction<'_>,
    source: &str,
    submission: &str,
) -> Result<Option<String>, PersistenceError> {
    current::table
        .filter(current::source_id.eq(source))
        .filter(current::submission_id.eq(submission))
        .select(current::account_id)
        .first(tx.connection())
        .optional()
        .map_err(PersistenceError::from)
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
        String,
        String,
        i64,
        Option<String>,
    );
    let rows: Vec<Row> = submissions::table
        .inner_join(jobs::table.on(jobs::import_id.eq(submissions::import_id)))
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
            submissions::image_sha256,
            submissions::state,
            submissions::revision,
            jobs::initialization_id,
        ))
        .limit(2)
        .load(tx.connection())?;
    rows.into_iter()
        .map(
            |(
                submission_id,
                result_id,
                account_id,
                account,
                image_sha256,
                state,
                revision,
                initialization_id,
            )| {
                Ok(SavedResult {
                    submission_id,
                    result_id,
                    account_id,
                    account,
                    image_sha256,
                    state: state_value(&state)?,
                    revision,
                    initialization_id: initialization_id.ok_or_else(|| {
                        PersistenceError::invalid("Photo submission initialization")
                    })?,
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
    type Row = (String, String, String, i64, String);
    let row: Option<Row> = current::table
        .inner_join(
            submissions::table.on(current::source_id
                .eq(submissions::source_id.nullable())
                .and(current::submission_id.eq(submissions::submission_id.nullable()))),
        )
        .filter(current::account_id.eq(account_id))
        .select((
            current::account,
            submissions::source_id,
            submissions::submission_id,
            current::revision,
            submissions::image_sha256,
        ))
        .first(tx.connection())
        .optional()?;
    Ok(row.map(
        |(account, source_id, submission_id, revision, image_sha256)| CurrentPhoto {
            account,
            source_id,
            submission_id,
            revision,
            image_sha256,
        },
    ))
}

pub(in crate::component::photo) fn insert(
    tx: &mut Transaction<'_>,
    id: Uuid,
    signed: &SignedResults,
    image: &TeamResult,
    state: SubmissionState,
) -> Result<(), PersistenceError> {
    diesel::insert_into(submissions::table)
        .values((
            submissions::source_id.eq(signed.manifest.context.source_id.as_str()),
            submissions::submission_id.eq(image.submission_id.as_str()),
            submissions::result_id.eq(image.result_id.as_str()),
            submissions::import_id.eq(id.to_string()),
            submissions::account_id.eq(&image.account_id),
            submissions::account.eq(&image.account),
            submissions::image_sha256.eq(image.image_sha256.as_str()),
            submissions::state.eq(state.as_str()),
            submissions::revision.eq(1_i64),
        ))
        .execute(tx.connection())?;
    Ok(())
}

pub(in crate::component::photo) fn state(
    tx: &mut Transaction<'_>,
    source: &str,
    submission: &str,
    expected: i64,
    state: SubmissionState,
) -> Result<(), PersistenceError> {
    let next = expected
        .checked_add(1)
        .ok_or_else(|| PersistenceError::invalid("Photo submission revision"))?;
    let count = diesel::update(
        submissions::table
            .find((source, submission))
            .filter(submissions::revision.eq(expected)),
    )
    .set((
        submissions::state.eq(state.as_str()),
        submissions::revision.eq(next),
    ))
    .execute(tx.connection())?;
    if count != 1 {
        return Err(PersistenceError::invalid("Photo submission transition"));
    }
    Ok(())
}

pub(in crate::component::photo) fn set_current(
    tx: &mut Transaction<'_>,
    account_id: &str,
    account: &str,
    source: &str,
    submission: &str,
    revision: i64,
) -> Result<(), PersistenceError> {
    diesel::insert_into(current::table)
        .values((
            current::account_id.eq(account_id),
            current::account.eq(account),
            current::source_id.eq(source),
            current::submission_id.eq(submission),
            current::revision.eq(revision),
        ))
        .on_conflict(current::account_id)
        .do_update()
        .set((
            current::account.eq(account),
            current::source_id.eq(source),
            current::submission_id.eq(submission),
            current::revision.eq(revision),
        ))
        .execute(tx.connection())?;
    Ok(())
}

pub(in crate::component::photo) fn next_revision(
    tx: &mut Transaction<'_>,
    account_id: &str,
) -> Result<i64, PersistenceError> {
    let revision = current::table
        .find(account_id)
        .select(current::revision)
        .first::<i64>(tx.connection())
        .optional()?;
    revision.map_or(Ok(1), |revision| {
        revision
            .checked_add(1)
            .ok_or_else(|| PersistenceError::invalid("Photo binding revision"))
    })
}

pub(in crate::component::photo) fn clear_current(
    tx: &mut Transaction<'_>,
    account_id: &str,
) -> Result<(), PersistenceError> {
    let revision = next_revision(tx, account_id)?;
    diesel::update(current::table.find(account_id))
        .set((
            current::source_id.eq(None::<String>),
            current::submission_id.eq(None::<String>),
            current::revision.eq(revision),
        ))
        .execute(tx.connection())?;
    Ok(())
}

pub(in crate::component::photo) fn submission_state(
    tx: &mut Transaction<'_>,
    source: &str,
    submission: &str,
) -> Result<(SubmissionState, i64), PersistenceError> {
    let (state, revision): (String, i64) = submissions::table
        .find((source, submission))
        .select((submissions::state, submissions::revision))
        .first(tx.connection())?;
    Ok((state_value(&state)?, revision))
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
