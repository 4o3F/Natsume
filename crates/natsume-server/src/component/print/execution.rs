use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use snafu::{OptionExt as _, ensure};
use tokio::time::Instant;

use super::{
    AuthenticationSnafu, InvalidSnafu, PrintComponent, PrintError, canonical_id, db,
    expiry::{CheckRequest, check_at},
    lowercase_hash,
};
use crate::db::{Transaction, TransactionError};

/// Verified client credentials; binding and confirmation are rechecked in each transaction.
#[derive(Clone)]
pub(crate) struct ClientIdentity {
    pub(super) client_id: String,
    pub(super) token_hash: [u8; 32],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AttemptOutcome {
    Printed,
    Failed,
    Unknown,
}

impl AttemptOutcome {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Printed => "printed",
            Self::Failed => "failed",
            Self::Unknown => "unknown",
        }
    }

    pub(super) fn parse(value: &str) -> Result<Self, PrintError> {
        match value {
            "printed" => Ok(Self::Printed),
            "failed" => Ok(Self::Failed),
            "unknown" => Ok(Self::Unknown),
            _ => Err(PrintError::Persistence),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AttemptState {
    Preparing,
    Observing,
    Printed,
    Failed,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ExecutionState {
    Queued,
    Active,
    Waiting,
    Completed,
    Exhausted,
}

impl ExecutionState {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Active => "active",
            Self::Waiting => "waiting",
            Self::Completed => "completed",
            Self::Exhausted => "exhausted",
        }
    }

    pub(super) fn parse(value: &str) -> Result<Self, PrintError> {
        match value {
            "queued" => Ok(Self::Queued),
            "active" => Ok(Self::Active),
            "waiting" => Ok(Self::Waiting),
            "completed" => Ok(Self::Completed),
            "exhausted" => Ok(Self::Exhausted),
            _ => Err(PrintError::Persistence),
        }
    }
}

#[derive(Debug, Serialize, specta::Type)]
pub(crate) struct PrintExecution {
    #[specta(type = specta_typescript::Number)]
    pub ready_sequence: i64,
    pub state: ExecutionState,
    pub attempts_used: i32,
    #[specta(type = Option<specta_typescript::Number>)]
    pub retry_at_unix_ms: Option<i64>,
    pub last_outcome: Option<AttemptOutcome>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, specta::Type)]
pub(crate) struct PrintAttempt {
    pub attempt_id: String,
    #[specta(type = specta_typescript::Number)]
    pub ready_sequence: i64,
    pub printer_id: String,
    pub client_id: String,
    pub attempt_number: i32,
    pub queue_name: String,
    pub label: String,
    pub state: AttemptState,
    #[specta(type = specta_typescript::Number)]
    pub claimed_at_unix_ms: i64,
    #[specta(type = specta_typescript::Number)]
    pub prepare_expires_at_unix_ms: i64,
    #[specta(type = specta_typescript::Number)]
    pub lease_expires_at_unix_ms: i64,
    #[specta(type = specta_typescript::Number)]
    pub last_contact_at_unix_ms: i64,
    #[specta(type = Option<specta_typescript::Number>)]
    pub last_observed_at_unix_ms: Option<i64>,
    #[specta(type = Option<specta_typescript::Number>)]
    pub job_id: Option<i64>,
    #[specta(type = Option<specta_typescript::Number>)]
    pub finished_at_unix_ms: Option<i64>,
    pub document_sha256: String,
    #[specta(type = specta_typescript::Number)]
    pub document_bytes: i64,
    #[specta(type = specta_typescript::Number)]
    pub document_pages: i64,
}

#[derive(Debug, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(crate) struct PrintClaim {
    pub request_id: String,
    pub queue_name: String,
}

#[derive(Clone, Debug, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub(crate) struct JobIdentity {
    pub queue_name: String,
    #[specta(type = specta_typescript::Number)]
    pub job_id: i64,
    pub label: String,
}

#[derive(Debug, Deserialize, specta::Type)]
#[serde(
    tag = "kind",
    content = "job",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub(crate) enum AttemptReport {
    Contact,
    Observed(JobIdentity),
    Printed(JobIdentity),
    Failed,
    Unknown,
}

impl PrintComponent {
    pub(crate) async fn authenticate_client(
        &self,
        token: &str,
    ) -> Result<ClientIdentity, PrintError> {
        let mut parts = token.split('.');
        let (Some("npc1"), Some(client), Some(secret), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return AuthenticationSnafu.fail();
        };
        ensure!(
            canonical_id(client).is_ok() && lowercase_hash(secret),
            AuthenticationSnafu
        );
        let identity = ClientIdentity {
            client_id: client.to_owned(),
            token_hash: Sha256::digest(token.as_bytes()).into(),
        };
        self.database
            .read(move |tx| {
                db::execution::authenticate(tx, &identity)?;
                Ok(identity)
            })
            .await
            .map_err(TransactionError::into_error)
    }

    async fn execute_client<T: Send + 'static>(
        &self,
        client: ClientIdentity,
        operation: impl FnOnce(&mut Transaction<'_>, &ClientIdentity, i64) -> Result<T, PrintError>
        + Send
        + 'static,
    ) -> Result<T, PrintError> {
        self.database
            .write(move |tx| {
                db::execution::authenticate(tx, &client)?;
                let now = db::printers::now(tx)?;
                db::execution::expire(tx, now)?;
                operation(tx, &client, now)
            })
            .await
            .map_err(TransactionError::into_error)
    }

    pub(crate) async fn claim(
        &self,
        client: ClientIdentity,
        input: PrintClaim,
    ) -> Result<Option<PrintAttempt>, PrintError> {
        canonical_id(&input.request_id)?;
        validate_queue(&input.queue_name)?;
        let inbox = std::sync::Arc::clone(&self.expiry);
        let sampled_at = Instant::now();
        self.execute_client(client, move |tx, client, now| {
            let result =
                db::execution::claim(tx, client, &input.request_id, &input.queue_name, now)?;
            if let Some(attempt) = &result
                && matches!(
                    attempt.state,
                    AttemptState::Preparing | AttemptState::Observing
                )
            {
                let deadline = if attempt.job_id.is_some() {
                    attempt.lease_expires_at_unix_ms
                } else {
                    attempt
                        .lease_expires_at_unix_ms
                        .min(attempt.prepare_expires_at_unix_ms)
                };
                // This is a check request, not evidence that the transaction committed.
                inbox
                    .schedule(CheckRequest {
                        attempt_id: attempt.attempt_id.clone(),
                        check_at: check_at(sampled_at, now, deadline),
                    })
                    .map_err(|error| {
                        tracing::error!(%error, "print expiry check registration failed");
                        PrintError::Persistence
                    })?;
            }
            Ok(result)
        })
        .await
    }

    pub(crate) async fn current_attempt(
        &self,
        client: ClientIdentity,
    ) -> Result<Option<PrintAttempt>, PrintError> {
        self.execute_client(client, |tx, client, _| db::execution::current(tx, client))
            .await
    }

    pub(crate) async fn get_attempt(
        &self,
        client: ClientIdentity,
        id: String,
    ) -> Result<PrintAttempt, PrintError> {
        canonical_id(&id)?;
        self.execute_client(client, move |tx, client, _| {
            db::execution::attempt(tx, client, &id)
        })
        .await
    }

    pub(crate) async fn register_job(
        &self,
        client: ClientIdentity,
        id: String,
        job: JobIdentity,
    ) -> Result<PrintAttempt, PrintError> {
        canonical_id(&id)?;
        validate_queue(&job.queue_name)?;
        ensure!(
            (1..=i64::from(u32::MAX)).contains(&job.job_id),
            InvalidSnafu
        );
        self.execute_client(client, move |tx, client, now| {
            db::execution::register_job(tx, client, &id, &job, now)
        })
        .await
    }

    pub(crate) async fn report_attempt(
        &self,
        client: ClientIdentity,
        id: String,
        report: AttemptReport,
    ) -> Result<PrintAttempt, PrintError> {
        canonical_id(&id)?;
        self.execute_client(client, move |tx, client, now| {
            db::execution::report(tx, client, &id, &report, now)
        })
        .await
    }

    pub(crate) async fn attempt_document(
        &self,
        client: ClientIdentity,
        id: String,
    ) -> Result<Vec<u8>, PrintError> {
        let attempt = self.get_attempt(client.clone(), id.clone()).await?;
        ensure!(
            matches!(
                attempt.state,
                AttemptState::Preparing | AttemptState::Observing
            ),
            super::ConflictSnafu
        );
        let store = self.documents.clone();
        let bytes = tokio::task::spawn_blocking(move || {
            store.read_verified(&attempt.document_sha256, attempt.document_bytes)
        })
        .await
        .map_err(|_| PrintError::Persistence)??;
        // File I/O never holds SQLite's write lock. Recheck revocation and ownership afterwards.
        let current = self.get_attempt(client, id).await?;
        ensure!(
            matches!(
                current.state,
                AttemptState::Preparing | AttemptState::Observing
            ),
            super::ConflictSnafu
        );
        Ok(bytes)
    }

    pub(crate) async fn list_executions(&self) -> Result<Vec<PrintExecution>, PrintError> {
        self.database
            .write(|tx| {
                let now = db::printers::now(tx)?;
                db::execution::expire(tx, now)?;
                db::execution::list(tx, now)
            })
            .await
            .map_err(TransactionError::into_error)
    }
}

fn validate_queue(name: &str) -> Result<(), PrintError> {
    ensure!(
        (1..=256).contains(&name.chars().count()) && !name.chars().any(char::is_control),
        InvalidSnafu
    );
    Ok(())
}

pub(super) fn deadline(now: i64, duration: i64) -> Result<i64, PrintError> {
    now.checked_add(duration).context(super::PersistenceSnafu)
}
