//! Explicit acceptance of verified results. All decisions run inside the caller's transaction.
use super::{PhotoComponent, db, validation};
use crate::{component::contest::PhotoTeam, db::Transaction};
use natsume_photo_protocol::{
    Sha256Digest,
    results::{SignedResults, TeamResult},
};
use snafu::{OptionExt as _, ResultExt as _, Snafu, ensure};
use uuid::Uuid;

#[derive(Debug, Snafu)]
pub(crate) enum AcceptanceError {
    #[snafu(display("Photo import does not exist"))]
    NotFound,
    #[snafu(display("Photo result or expected revision conflicts"))]
    Conflict,
    #[snafu(display("Photo decision is invalid"))]
    Invalid,
    #[snafu(display("Photo acceptance persistence failed"))]
    Persistence { source: db::PersistenceError },
    #[snafu(display("Photo acceptance authorization failed"))]
    Validation { source: validation::ValidationError },
    #[snafu(display("Photo saved manifest is invalid"))]
    Json { source: serde_json::Error },
    #[snafu(display("Photo saved manifest fingerprint failed"))]
    Fingerprint {
        source: natsume_photo_protocol::results::ResultError,
    },
    #[snafu(display("Photo saved manifest is inconsistent"))]
    Corrupt,
}
impl From<crate::db::PersistenceError> for AcceptanceError {
    fn from(source: crate::db::PersistenceError) -> Self {
        Self::Persistence {
            source: source.into(),
        }
    }
}
type Result<T> = std::result::Result<T, AcceptanceError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SubmissionState {
    Active,
    PendingReview,
    NeedsAssociation,
    Rejected,
    Superseded,
}
impl SubmissionState {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::PendingReview => "pending_review",
            Self::NeedsAssociation => "needs_association",
            Self::Rejected => "rejected",
            Self::Superseded => "superseded",
        }
    }
}

pub(super) struct SavedResult {
    pub submission_id: String,
    pub result_id: String,
    pub account_id: String,
    pub account: String,
    pub image_sha256: String,
    pub initialization_id: String,
    pub state: SubmissionState,
    pub revision: i64,
}

#[derive(serde::Serialize, specta::Type)]
pub(crate) struct CurrentPhoto {
    pub account: String,
    pub source_id: String,
    pub submission_id: String,
    #[specta(type = specta_typescript::Number)]
    pub revision: i64,
    pub image_sha256: String,
}
#[derive(serde::Serialize, specta::Type)]
pub(crate) struct PhotoPreviewEntry {
    pub submission_id: String,
    pub result_id: String,
    pub account_id: String,
    pub account: String,
    pub image_sha256: String,
    pub state: SubmissionState,
    #[specta(type = specta_typescript::Number)]
    pub submission_revision: i64,
    pub current: Option<CurrentPhoto>,
    pub active_account_id: Option<String>,
}
#[derive(serde::Serialize, specta::Type)]
pub(crate) struct ImportPreview {
    pub import_id: Uuid,
    pub committed: bool,
    pub entries: Vec<PhotoPreviewEntry>,
}

#[derive(Default, serde::Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(crate) struct PendingPhotoQuery {
    pub after_source_id: Option<natsume_photo_protocol::SourceId>,
    pub after_submission_id: Option<natsume_photo_protocol::results::SubmissionId>,
    pub limit: Option<u16>,
}

#[derive(serde::Serialize, specta::Type)]
pub(crate) struct PendingPhoto {
    pub source_id: String,
    pub submission_id: String,
    pub import_id: Uuid,
    pub account_id: String,
    pub account: String,
    pub image_sha256: String,
    pub state: SubmissionState,
    #[specta(type = specta_typescript::Number)]
    pub revision: i64,
}

#[derive(serde::Serialize, specta::Type)]
pub(crate) struct PendingPhotoCursor {
    pub after_source_id: String,
    pub after_submission_id: String,
}

#[derive(serde::Serialize, specta::Type)]
pub(crate) struct PendingPhotoPage {
    pub entries: Vec<PendingPhoto>,
    pub next: Option<PendingPhotoCursor>,
}

#[derive(serde::Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExpectedPhoto {
    pub source_id: String,
    pub submission_id: String,
    #[specta(type = specta_typescript::Number)]
    pub revision: i64,
}
#[derive(serde::Deserialize, specta::Type)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum PhotoAction {
    Reject {},
    Activate {
        account_id: String,
        account: String,
        expected_current: Option<ExpectedPhoto>,
    },
}
#[derive(serde::Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(crate) struct PhotoDecision {
    pub submission_id: String,
    #[specta(type = specta_typescript::Number)]
    pub expected_submission_revision: i64,
    pub decision: PhotoAction,
}

fn package(tx: &mut Transaction<'_>, id: Uuid) -> Result<(bool, SignedResults)> {
    let (state, json, hash) = db::acceptance::package(tx, id)
        .context(PersistenceSnafu)?
        .context(NotFoundSnafu)?;
    ensure!(
        matches!(state.as_str(), "ready" | "committed"),
        ConflictSnafu
    );
    let json = json.context(CorruptSnafu)?;
    ensure!(
        json.len() <= natsume_photo_protocol::MANIFEST_LIMIT,
        CorruptSnafu
    );
    let signed: SignedResults = serde_json::from_str(&json).context(JsonSnafu)?;
    ensure!(
        hash.as_deref()
            == Some(
                Sha256Digest::of(&signed.manifest.signing_bytes().context(FingerprintSnafu)?)
                    .as_str()
            ),
        CorruptSnafu
    );
    validation::trusted(tx, &signed).context(ValidationSnafu)?;
    Ok((state == "committed", signed))
}

fn existing(
    tx: &mut Transaction<'_>,
    signed: &SignedResults,
    image: &TeamResult,
) -> Result<Option<SavedResult>> {
    let rows = db::acceptance::matching(tx, signed, image).context(PersistenceSnafu)?;
    ensure!(rows.len() <= 1, ConflictSnafu);
    let row = rows.into_iter().next();
    if let Some(row) = &row {
        ensure!(
            row.submission_id == image.submission_id.as_str()
                && row.result_id == image.result_id.as_str()
                && row.account_id == image.account_id
                && row.account == image.account
                && row.image_sha256 == image.image_sha256.as_str()
                && row.initialization_id == signed.manifest.context.initialization_id.as_str(),
            ConflictSnafu
        );
    }
    Ok(row)
}

fn proposed(
    image: &TeamResult,
    roster: &[PhotoTeam],
    current: Option<&CurrentPhoto>,
) -> SubmissionState {
    if !roster
        .iter()
        .any(|team| team.account_id == image.account_id && team.account == image.account)
    {
        SubmissionState::NeedsAssociation
    } else if let Some(current) = current {
        if current.account == image.account && current.image_sha256 == image.image_sha256.as_str() {
            SubmissionState::Superseded
        } else {
            SubmissionState::PendingReview
        }
    } else {
        SubmissionState::Active
    }
}

fn association(
    tx: &mut Transaction<'_>,
    source: &str,
    submission: &str,
    roster: &[PhotoTeam],
) -> Result<Option<(String, bool)>> {
    let Some(account_id) =
        db::acceptance::active_account(tx, source, submission).context(PersistenceSnafu)?
    else {
        return Ok(None);
    };
    let current = db::acceptance::current(tx, &account_id)
        .context(PersistenceSnafu)?
        .context(CorruptSnafu)?;
    let valid = roster
        .iter()
        .any(|team| team.account_id == account_id && team.account == current.account);
    Ok(Some((account_id, valid)))
}

impl PhotoComponent {
    pub(crate) fn pending_in(
        tx: &mut Transaction<'_>,
        query: PendingPhotoQuery,
        roster: &[PhotoTeam],
    ) -> Result<PendingPhotoPage> {
        let limit = query.limit.unwrap_or(50);
        ensure!((1..=100).contains(&limit), InvalidSnafu);
        let after = match (query.after_source_id, query.after_submission_id) {
            (None, None) => None,
            (Some(source), Some(submission)) => Some((source.to_string(), submission.to_string())),
            _ => return InvalidSnafu.fail(),
        };
        let mut entries = db::acceptance::pending(
            tx,
            after
                .as_ref()
                .map(|(source, submission)| (source.as_str(), submission.as_str())),
            i64::from(limit) + 1,
        )
        .context(PersistenceSnafu)?;
        let next = if entries.len() > usize::from(limit) {
            entries.pop();
            entries.last().map(|entry| PendingPhotoCursor {
                after_source_id: entry.source_id.clone(),
                after_submission_id: entry.submission_id.clone(),
            })
        } else {
            None
        };
        let mut visible = Vec::with_capacity(entries.len());
        for mut entry in entries {
            if entry.state == SubmissionState::Active {
                let (_, valid) = association(tx, &entry.source_id, &entry.submission_id, roster)?
                    .context(CorruptSnafu)?;
                if valid {
                    continue;
                }
                entry.state = SubmissionState::NeedsAssociation;
            } else if entry.state == SubmissionState::PendingReview
                && !roster.iter().any(|team| {
                    team.account_id == entry.account_id && team.account == entry.account
                })
            {
                entry.state = SubmissionState::NeedsAssociation;
            }
            visible.push(entry);
        }
        Ok(PendingPhotoPage {
            entries: visible,
            next,
        })
    }

    pub(crate) fn preview_in(
        tx: &mut Transaction<'_>,
        id: Uuid,
        roster: &[PhotoTeam],
    ) -> Result<ImportPreview> {
        let (committed, signed) = package(tx, id)?;
        preview(tx, id, committed, &signed, roster)
    }

    pub(crate) fn commit_in(
        tx: &mut Transaction<'_>,
        id: Uuid,
        roster: &[PhotoTeam],
    ) -> Result<ImportPreview> {
        let (committed, signed) = package(tx, id)?;
        if !committed {
            for image in &signed.manifest.results {
                if existing(tx, &signed, image)?.is_some() {
                    continue;
                }
                let current =
                    db::acceptance::current(tx, &image.account_id).context(PersistenceSnafu)?;
                let state = proposed(image, roster, current.as_ref());
                db::acceptance::insert(tx, id, &signed, image, state).context(PersistenceSnafu)?;
                if state == SubmissionState::Active {
                    let revision = db::acceptance::next_revision(tx, &image.account_id)
                        .context(PersistenceSnafu)?;
                    db::acceptance::set_current(
                        tx,
                        &image.account_id,
                        &image.account,
                        signed.manifest.context.source_id.as_str(),
                        image.submission_id.as_str(),
                        revision,
                    )
                    .context(PersistenceSnafu)?;
                }
            }
            db::acceptance::committed(tx, id).context(PersistenceSnafu)?;
        }
        preview(tx, id, true, &signed, roster)
    }
}

fn preview(
    tx: &mut Transaction<'_>,
    id: Uuid,
    committed: bool,
    signed: &SignedResults,
    roster: &[PhotoTeam],
) -> Result<ImportPreview> {
    let mut entries = Vec::with_capacity(signed.manifest.results.len());
    for image in &signed.manifest.results {
        let saved = existing(tx, signed, image)?;
        let current = db::acceptance::current(tx, &image.account_id).context(PersistenceSnafu)?;
        ensure!(!committed || saved.is_some(), CorruptSnafu);
        let (mut state, submission_revision) = saved.map_or_else(
            || (proposed(image, roster, current.as_ref()), 0),
            |saved| (saved.state, saved.revision),
        );
        let associated = association(
            tx,
            signed.manifest.context.source_id.as_str(),
            image.submission_id.as_str(),
            roster,
        )?;
        if state == SubmissionState::Active && associated.as_ref().is_some_and(|(_, valid)| !valid)
        {
            state = SubmissionState::NeedsAssociation;
        }
        entries.push(PhotoPreviewEntry {
            submission_id: image.submission_id.to_string(),
            result_id: image.result_id.to_string(),
            account_id: image.account_id.clone(),
            account: image.account.clone(),
            image_sha256: image.image_sha256.to_string(),
            state,
            submission_revision,
            current,
            active_account_id: associated.and_then(|(id, valid)| valid.then_some(id)),
        });
    }
    Ok(ImportPreview {
        import_id: id,
        committed,
        entries,
    })
}

impl PhotoComponent {
    pub(crate) fn decide_in(
        tx: &mut Transaction<'_>,
        id: Uuid,
        decision: PhotoDecision,
        roster: &[PhotoTeam],
    ) -> Result<ImportPreview> {
        ensure!(decision.expected_submission_revision > 0, InvalidSnafu);
        let (committed, signed) = package(tx, id)?;
        ensure!(committed, ConflictSnafu);
        let image = signed
            .manifest
            .results
            .iter()
            .find(|image| image.submission_id.as_str() == decision.submission_id)
            .context(NotFoundSnafu)?;
        let saved = existing(tx, &signed, image)?.context(CorruptSnafu)?;
        let source = signed.manifest.context.source_id.as_str();
        let associated = association(tx, source, &saved.submission_id, roster)?;
        let stale_active = saved.state == SubmissionState::Active
            && associated.as_ref().is_some_and(|(_, valid)| !valid);
        ensure!(
            saved.revision == decision.expected_submission_revision
                && (stale_active
                    || matches!(
                        saved.state,
                        SubmissionState::PendingReview | SubmissionState::NeedsAssociation
                    )),
            ConflictSnafu
        );
        match decision.decision {
            PhotoAction::Reject {} => {
                if stale_active {
                    db::acceptance::clear_current(
                        tx,
                        &associated.as_ref().context(CorruptSnafu)?.0,
                    )
                    .context(PersistenceSnafu)?;
                }
                db::acceptance::state(
                    tx,
                    source,
                    &saved.submission_id,
                    saved.revision,
                    SubmissionState::Rejected,
                )
                .context(PersistenceSnafu)?;
            }
            PhotoAction::Activate {
                account_id,
                account,
                expected_current,
            } => {
                ensure!(
                    roster
                        .iter()
                        .any(|team| team.account_id == account_id && team.account == account),
                    ConflictSnafu
                );
                // A matching original identity can replace only its own photo. Moving to
                // another identity is reserved for explicitly unassociated submissions.
                ensure!(
                    !roster
                        .iter()
                        .any(|team| team.account_id == image.account_id
                            && team.account == image.account)
                        || (account_id == image.account_id && account == image.account),
                    ConflictSnafu
                );
                let current = db::acceptance::current(tx, &account_id).context(PersistenceSnafu)?;
                ensure!(
                    matches_current(current.as_ref(), expected_current.as_ref()),
                    ConflictSnafu
                );
                let revision =
                    db::acceptance::next_revision(tx, &account_id).context(PersistenceSnafu)?;
                if stale_active {
                    let old_account = &associated.as_ref().context(CorruptSnafu)?.0;
                    if old_account != &account_id {
                        db::acceptance::clear_current(tx, old_account).context(PersistenceSnafu)?;
                    }
                }
                if let Some(old) = current.filter(|old| {
                    old.source_id != source || old.submission_id != saved.submission_id
                }) {
                    retire_current(tx, &old)?;
                }
                db::acceptance::state(
                    tx,
                    source,
                    &saved.submission_id,
                    saved.revision,
                    SubmissionState::Active,
                )
                .context(PersistenceSnafu)?;
                db::acceptance::set_current(
                    tx,
                    &account_id,
                    &account,
                    source,
                    &saved.submission_id,
                    revision,
                )
                .context(PersistenceSnafu)?;
            }
        }
        preview(tx, id, true, &signed, roster)
    }
}

fn retire_current(tx: &mut Transaction<'_>, old: &CurrentPhoto) -> Result<()> {
    let (state, revision) =
        db::acceptance::submission_state(tx, &old.source_id, &old.submission_id)
            .context(PersistenceSnafu)?;
    ensure!(state == SubmissionState::Active, CorruptSnafu);
    db::acceptance::state(
        tx,
        &old.source_id,
        &old.submission_id,
        revision,
        SubmissionState::Superseded,
    )
    .context(PersistenceSnafu)
}

fn matches_current(current: Option<&CurrentPhoto>, expected: Option<&ExpectedPhoto>) -> bool {
    match (current, expected) {
        (None, None) => true,
        (Some(current), Some(expected)) => {
            current.source_id == expected.source_id
                && current.submission_id == expected.submission_id
                && current.revision == expected.revision
        }
        _ => false,
    }
}
