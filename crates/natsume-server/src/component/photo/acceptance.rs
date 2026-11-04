//! Photo-owned acceptance transactions include the current Contest roster check.
use super::{PhotoComponent, PhotoRoster, db, validation};
use crate::db::{Transaction, TransactionError};
use natsume_photo_protocol::{
    Sha256Digest, SourceId,
    results::{ResultId, SignedResults, SubmissionId, TeamResult},
};
use snafu::{OptionExt as _, ResultExt as _, Snafu, ensure};
use std::collections::{BTreeSet, HashMap};
use uuid::Uuid;

#[derive(Debug, Snafu)]
pub(crate) enum AcceptanceError {
    #[snafu(display("Photo roster could not be read"))]
    Roster {
        source: crate::component::contest::ContestError,
    },
    #[snafu(display("Photo import does not exist"))]
    NotFound,
    #[snafu(display("Photo result or expected revision conflicts"))]
    Conflict,
    #[snafu(display("Photo decision is invalid"))]
    Invalid,
    #[snafu(display("Photo acceptance persistence failed"))]
    Persistence { source: db::PersistenceError },
    #[snafu(display("Photo acceptance authorization failed"))]
    Authorization { source: validation::ValidationError },
    #[snafu(display("Photo saved manifest is invalid"))]
    Json { source: serde_json::Error },
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
    pub(super) fn parse(raw: &str) -> Option<Self> {
        match raw {
            "active" => Some(Self::Active),
            "pending_review" => Some(Self::PendingReview),
            "needs_association" => Some(Self::NeedsAssociation),
            "rejected" => Some(Self::Rejected),
            "superseded" => Some(Self::Superseded),
            _ => None,
        }
    }
}

pub(super) struct SavedResult {
    pub submission_id: SubmissionId,
    pub result_id: ResultId,
    pub account_id: String,
    pub account: String,
    pub image_sha256: Sha256Digest,
    pub assigned_account_id: Option<String>,
    pub state: SubmissionState,
    pub revision: i64,
}

impl SavedResult {
    fn effective_state(&self, roster: &PhotoRoster) -> SubmissionState {
        effective_state(
            self.state,
            &self.account_id,
            self.assigned_account_id.as_deref(),
            roster,
        )
    }
}

#[derive(Clone, serde::Serialize, specta::Type)]
pub(crate) struct CurrentPhoto {
    pub source_id: SourceId,
    pub submission_id: SubmissionId,
    #[specta(type = specta_typescript::Number)]
    pub revision: i64,
    pub image_sha256: Sha256Digest,
}
#[derive(serde::Serialize, specta::Type)]
pub(crate) struct PhotoPreviewEntry {
    pub submission_id: SubmissionId,
    pub result_id: ResultId,
    pub account_id: String,
    pub account: String,
    pub image_sha256: Sha256Digest,
    pub state: SubmissionState,
    #[specta(type = specta_typescript::Number)]
    pub submission_revision: i64,
    pub current: Option<CurrentPhoto>,
    pub active_account_id: Option<String>,
}
#[derive(serde::Serialize, specta::Type)]
pub(crate) struct ResultImportPreview {
    pub import_id: Uuid,
    pub committed: bool,
    pub entries: Vec<PhotoPreviewEntry>,
}

#[derive(Default, serde::Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(crate) struct PendingPhotoQuery {
    pub after_source_id: Option<SourceId>,
    pub after_submission_id: Option<SubmissionId>,
    pub limit: Option<u16>,
}

#[derive(serde::Serialize, specta::Type)]
pub(crate) struct PendingPhoto {
    pub source_id: SourceId,
    pub submission_id: SubmissionId,
    pub import_id: Uuid,
    pub account_id: String,
    pub account: String,
    pub assigned_account_id: Option<String>,
    pub image_sha256: Sha256Digest,
    pub state: SubmissionState,
    #[specta(type = specta_typescript::Number)]
    pub revision: i64,
}

#[derive(serde::Serialize, specta::Type)]
pub(crate) struct PendingPhotoCursor {
    pub after_source_id: SourceId,
    pub after_submission_id: SubmissionId,
}

#[derive(serde::Serialize, specta::Type)]
pub(crate) struct PendingPhotoPage {
    pub entries: Vec<PendingPhoto>,
    pub next: Option<PendingPhotoCursor>,
}

#[derive(serde::Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExpectedPhoto {
    pub source_id: SourceId,
    pub submission_id: SubmissionId,
}
#[derive(serde::Deserialize, specta::Type)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum PhotoAction {
    Reject,
    Activate {
        account_id: String,
        expected_current: Option<ExpectedPhoto>,
    },
}
#[derive(serde::Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(crate) struct PhotoDecision {
    pub submission_id: SubmissionId,
    #[specta(type = specta_typescript::Number)]
    pub expected_submission_revision: i64,
    pub action: PhotoAction,
}

fn load_verified_manifest(tx: &mut Transaction<'_>, id: Uuid) -> Result<(bool, SignedResults)> {
    let (state, json) = db::jobs::load_manifest(tx, id)
        .context(PersistenceSnafu)?
        .context(NotFoundSnafu)?;
    ensure!(
        matches!(
            state,
            super::imports::ResultImportState::Ready | super::imports::ResultImportState::Committed
        ),
        ConflictSnafu
    );
    let json = json.context(CorruptSnafu)?;
    ensure!(
        json.len() <= natsume_photo_protocol::MANIFEST_LIMIT,
        CorruptSnafu
    );
    let signed = serde_json::from_str(&json).context(JsonSnafu)?;
    Ok((
        state == super::imports::ResultImportState::Committed,
        signed,
    ))
}

// One pair of bulk queries per review snapshot, independent of team count.
struct ReviewFacts {
    saved: HashMap<SubmissionId, SavedResult>,
    results: HashMap<ResultId, SubmissionId>,
    current: HashMap<String, CurrentPhoto>,
}
impl ReviewFacts {
    fn load(tx: &mut Transaction<'_>, signed: &SignedResults) -> Result<Self> {
        let saved = db::acceptance::saved_submissions(tx, signed).context(PersistenceSnafu)?;
        let targets: BTreeSet<_> = signed
            .manifest
            .results
            .iter()
            .map(|image| image.account_id.as_str())
            .chain(
                saved
                    .iter()
                    .filter_map(|row| row.assigned_account_id.as_deref()),
            )
            .collect();
        let current = db::acceptance::current_assignments(tx, targets.into_iter().collect())
            .context(PersistenceSnafu)?;
        let results = saved
            .iter()
            .map(|row| (row.result_id.clone(), row.submission_id.clone()))
            .collect();
        Ok(Self {
            saved: saved
                .into_iter()
                .map(|row| (row.submission_id.clone(), row))
                .collect(),
            results,
            current,
        })
    }

    // New/uncommitted input must not reuse either signed identity with other content.
    fn find_saved_submission(&self, image: &TeamResult) -> Result<Option<&SavedResult>> {
        let by_result = self.results.get(&image.result_id);
        if let Some(id) = by_result {
            ensure!(id == &image.submission_id, ConflictSnafu);
        }
        let row = self.saved.get(&image.submission_id);
        if let Some(row) = row {
            ensure!(
                row.result_id == image.result_id
                    && row.account_id == image.account_id
                    && row.account == image.account
                    && row.image_sha256 == image.image_sha256,
                ConflictSnafu
            );
        }
        Ok(row)
    }
}

fn initial_state(
    image: &TeamResult,
    roster: &PhotoRoster,
    current: Option<&CurrentPhoto>,
) -> SubmissionState {
    if !roster.contains(&image.account_id) {
        SubmissionState::NeedsAssociation
    } else if let Some(current) = current {
        if current.image_sha256 == image.image_sha256 {
            SubmissionState::Superseded
        } else {
            SubmissionState::PendingReview
        }
    } else {
        SubmissionState::Active
    }
}

fn effective_state(
    state: SubmissionState,
    original: &str,
    assigned: Option<&str>,
    roster: &PhotoRoster,
) -> SubmissionState {
    match state {
        SubmissionState::Active if !assigned.is_some_and(|id| roster.contains(id)) => {
            SubmissionState::NeedsAssociation
        }
        SubmissionState::PendingReview if !roster.contains(original) => {
            SubmissionState::NeedsAssociation
        }
        _ => state,
    }
}

impl PhotoComponent {
    fn pending_in(
        tx: &mut Transaction<'_>,
        query: PendingPhotoQuery,
        roster: &PhotoRoster,
    ) -> Result<PendingPhotoPage> {
        let limit = query.limit.unwrap_or(50);
        ensure!((1..=100).contains(&limit), InvalidSnafu);
        let after = match (query.after_source_id, query.after_submission_id) {
            (None, None) => None,
            (Some(source), Some(submission)) => Some((source, submission)),
            _ => return InvalidSnafu.fail(),
        };
        let mut entries = db::acceptance::pending(
            tx,
            after
                .as_ref()
                .map(|(source, submission)| (source, submission)),
            i64::from(limit) + 1,
            roster,
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
        for entry in &mut entries {
            entry.state = effective_state(
                entry.state,
                &entry.account_id,
                entry.assigned_account_id.as_deref(),
                roster,
            );
        }
        Ok(PendingPhotoPage { entries, next })
    }

    fn preview_in(
        tx: &mut Transaction<'_>,
        id: Uuid,
        roster: &PhotoRoster,
    ) -> Result<ResultImportPreview> {
        let (committed, signed) = load_verified_manifest(tx, id)?;
        preview(tx, id, committed, &signed, roster)
    }

    fn commit_in(
        tx: &mut Transaction<'_>,
        id: Uuid,
        roster: &PhotoRoster,
    ) -> Result<ResultImportPreview> {
        let (committed, signed) = load_verified_manifest(tx, id)?;
        validation::verify_source_authorization(tx, &signed).context(AuthorizationSnafu)?;
        if !committed {
            let facts = ReviewFacts::load(tx, &signed)?;
            for image in &signed.manifest.results {
                if facts.find_saved_submission(image)?.is_some() {
                    continue;
                }
                let current = facts.current.get(&image.account_id);
                let state = initial_state(image, roster, current);
                let assigned = roster
                    .contains(&image.account_id)
                    .then_some(image.account_id.as_str());
                db::acceptance::insert(tx, id, &signed, image, state, assigned)
                    .context(PersistenceSnafu)?;
            }
            db::jobs::mark_committed(tx, id).context(PersistenceSnafu)?;
        }
        preview(tx, id, true, &signed, roster)
    }
}

fn preview(
    tx: &mut Transaction<'_>,
    id: Uuid,
    committed: bool,
    signed: &SignedResults,
    roster: &PhotoRoster,
) -> Result<ResultImportPreview> {
    let facts = ReviewFacts::load(tx, signed)?;
    let mut entries = Vec::with_capacity(signed.manifest.results.len());
    for image in &signed.manifest.results {
        let saved = if committed {
            facts.saved.get(&image.submission_id)
        } else {
            facts.find_saved_submission(image)?
        };
        ensure!(!committed || saved.is_some(), CorruptSnafu);
        let target = saved
            .as_ref()
            .and_then(|saved| saved.assigned_account_id.as_deref())
            .unwrap_or(&image.account_id);
        let current = facts.current.get(target).cloned();
        let (state, submission_revision, active_account_id) = if let Some(saved) = saved {
            let state = saved.effective_state(roster);
            let assigned = if state == SubmissionState::Active {
                saved.assigned_account_id.clone()
            } else {
                None
            };
            (state, saved.revision, assigned)
        } else {
            (initial_state(image, roster, current.as_ref()), 0, None)
        };
        entries.push(PhotoPreviewEntry {
            submission_id: image.submission_id.clone(),
            result_id: image.result_id.clone(),
            account_id: image.account_id.clone(),
            account: image.account.clone(),
            image_sha256: image.image_sha256.clone(),
            state,
            submission_revision,
            current,
            active_account_id,
        });
    }
    Ok(ResultImportPreview {
        import_id: id,
        committed,
        entries,
    })
}

impl PhotoComponent {
    fn decide_in(
        tx: &mut Transaction<'_>,
        id: Uuid,
        decision: PhotoDecision,
        roster: &PhotoRoster,
    ) -> Result<ResultImportPreview> {
        ensure!(decision.expected_submission_revision > 0, InvalidSnafu);
        let (committed, signed) = load_verified_manifest(tx, id)?;
        validation::verify_source_authorization(tx, &signed).context(AuthorizationSnafu)?;
        ensure!(committed, ConflictSnafu);
        let image = signed
            .manifest
            .results
            .iter()
            .find(|image| image.submission_id == decision.submission_id)
            .context(NotFoundSnafu)?;
        let facts = ReviewFacts::load(tx, &signed)?;
        let saved = facts
            .saved
            .get(&image.submission_id)
            .context(CorruptSnafu)?;
        ensure!(
            saved.revision == decision.expected_submission_revision
                && matches!(
                    saved.effective_state(roster),
                    SubmissionState::PendingReview | SubmissionState::NeedsAssociation
                ),
            ConflictSnafu
        );
        let source = &signed.manifest.context.source_id;
        match decision.action {
            PhotoAction::Reject => db::acceptance::transition(
                tx,
                source,
                &saved.submission_id,
                saved.revision,
                SubmissionState::Rejected,
                None,
            )
            .context(PersistenceSnafu)?,
            PhotoAction::Activate {
                account_id,
                expected_current,
            } => {
                ensure!(roster.contains(&account_id), ConflictSnafu);
                ensure!(
                    !roster.contains(&image.account_id) || account_id == image.account_id,
                    ConflictSnafu
                );
                let current = db::acceptance::current(tx, &account_id).context(PersistenceSnafu)?;
                ensure!(
                    matches_current(current.as_ref(), expected_current.as_ref()),
                    ConflictSnafu
                );
                if let Some(old) = current.filter(|old| {
                    &old.source_id != source || old.submission_id != saved.submission_id
                }) {
                    db::acceptance::transition(
                        tx,
                        &old.source_id,
                        &old.submission_id,
                        old.revision,
                        SubmissionState::Superseded,
                        None,
                    )
                    .context(PersistenceSnafu)?;
                }
                db::acceptance::transition(
                    tx,
                    source,
                    &saved.submission_id,
                    saved.revision,
                    SubmissionState::Active,
                    Some(&account_id),
                )
                .context(PersistenceSnafu)?;
            }
        }
        preview(tx, id, true, &signed, roster)
    }
}

fn matches_current(current: Option<&CurrentPhoto>, expected: Option<&ExpectedPhoto>) -> bool {
    match (current, expected) {
        (None, None) => true,
        (Some(current), Some(expected)) => {
            current.source_id == expected.source_id
                && current.submission_id == expected.submission_id
        }
        _ => false,
    }
}

impl PhotoComponent {
    pub(crate) async fn pending(&self, query: PendingPhotoQuery) -> Result<PendingPhotoPage> {
        self.database
            .read(move |tx| {
                let roster = super::load_roster(tx).context(RosterSnafu)?;
                Self::pending_in(tx, query, &roster)
            })
            .await
            .map_err(TransactionError::into_error)
    }

    pub(crate) async fn preview(&self, id: Uuid) -> Result<ResultImportPreview> {
        self.database
            .read(move |tx| {
                let roster = super::load_roster(tx).context(RosterSnafu)?;
                Self::preview_in(tx, id, &roster)
            })
            .await
            .map_err(TransactionError::into_error)
    }
    pub(crate) async fn commit(&self, id: Uuid) -> Result<ResultImportPreview> {
        self.database
            .write(move |tx| {
                let roster = super::load_roster(tx).context(RosterSnafu)?;
                Self::commit_in(tx, id, &roster)
            })
            .await
            .map_err(TransactionError::into_error)
    }
    pub(crate) async fn decide(
        &self,
        id: Uuid,
        decision: PhotoDecision,
    ) -> Result<ResultImportPreview> {
        self.database
            .write(move |tx| {
                let roster = super::load_roster(tx).context(RosterSnafu)?;
                Self::decide_in(tx, id, decision, &roster)
            })
            .await
            .map_err(TransactionError::into_error)
    }
}
