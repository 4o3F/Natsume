//! Photo persistence; decisions and roster semantics stay in the owning component.
use super::super::{
    PhotoRoster,
    acceptance::{CurrentPhoto, PendingPhoto, SavedResult, SubmissionState},
};
use super::{PersistenceError, UuidSnafu, ValueSnafu};
use crate::{db::Transaction, diesel_schema::photo_submissions as submissions};
use diesel::{BoolExpressionMethods as _, ExpressionMethods as _, QueryDsl as _, RunQueryDsl as _};
use natsume_photo_protocol::{
    SourceId,
    results::{SignedResults, SubmissionId, TeamResult},
};
use snafu::ResultExt as _;
use uuid::Uuid;

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
                .eq_any([
                    SubmissionState::PendingReview.as_str(),
                    SubmissionState::NeedsAssociation.as_str(),
                ])
                .or(submissions::state.eq(SubmissionState::Active.as_str()).and(
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
                    state: SubmissionState::parse(&state)
                        .ok_or_else(|| PersistenceError::invalid("Photo submission state"))?,
                    revision,
                })
            },
        )
        .collect()
}

pub(in crate::component::photo) fn saved_submissions(
    tx: &mut Transaction<'_>,
    signed: &SignedResults,
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
                .eq_any(
                    signed
                        .manifest
                        .results
                        .iter()
                        .map(|image| image.submission_id.as_str()),
                )
                .or(submissions::result_id.eq_any(
                    signed
                        .manifest
                        .results
                        .iter()
                        .map(|image| image.result_id.as_str()),
                )),
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
                    state: SubmissionState::parse(&state)
                        .ok_or_else(|| PersistenceError::invalid("Photo submission state"))?,
                    revision,
                })
            },
        )
        .collect()
}

pub(in crate::component::photo) fn current_assignments(
    tx: &mut Transaction<'_>,
    accounts: Vec<&str>,
) -> Result<std::collections::HashMap<String, CurrentPhoto>, PersistenceError> {
    type Row = (Option<String>, String, String, i64, String);
    submissions::table
        .filter(submissions::assigned_account_id.eq_any(accounts))
        .filter(submissions::state.eq(SubmissionState::Active.as_str()))
        .select((
            submissions::assigned_account_id,
            submissions::source_id,
            submissions::submission_id,
            submissions::revision,
            submissions::image_sha256,
        ))
        .load::<Row>(tx.connection())?
        .into_iter()
        .map(|(account, source, submission, revision, hash)| {
            Ok((
                account.ok_or_else(|| PersistenceError::invalid("Photo assigned account"))?,
                CurrentPhoto {
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
                },
            ))
        })
        .collect()
}

pub(in crate::component::photo) fn current(
    tx: &mut Transaction<'_>,
    account: &str,
) -> Result<Option<CurrentPhoto>, PersistenceError> {
    Ok(current_assignments(tx, vec![account])?.remove(account))
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
            submissions::image_format.eq(super::format_name(image.image_format)),
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
