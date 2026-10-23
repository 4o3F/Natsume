use serde::{Deserialize, Serialize};
use snafu::ensure;
use uuid::Uuid;

use super::error::{ResponseSnafu, Result};

fn required_option<'de, D, T>(deserializer: D) -> std::result::Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

pub(super) fn canonical_id(value: &str) -> bool {
    Uuid::parse_str(value).is_ok_and(|id| {
        id.get_version_num() == 7
            && id.get_variant() == uuid::Variant::RFC4122
            && id.to_string() == value
    })
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Binding {
    pub printer_id: String,
    pub client_id: String,
    pub confirmed: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Pairing {
    pub printer_id: String,
    pub client_id: String,
    pub token: String,
}

impl Pairing {
    pub fn validate(&self) -> Result<()> {
        let prefix = format!("npc1.{}.", self.client_id);
        ensure!(
            canonical_id(&self.printer_id)
                && canonical_id(&self.client_id)
                && self
                    .token
                    .strip_prefix(&prefix)
                    .is_some_and(|secret| secret.len() == 64
                        && secret
                            .bytes()
                            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))),
            ResponseSnafu
        );
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PrinterInfo {
    pub printer_id: String,
    pub name: String,
    #[serde(deserialize_with = "required_option")]
    pub client_id: Option<String>,
    pub client_confirmed: bool,
    #[serde(deserialize_with = "required_option")]
    pub pairing_expires_at_unix_ms: Option<i64>,
}

impl PrinterInfo {
    pub fn validate(&self, binding: &Binding) -> Result<()> {
        ensure!(
            self.printer_id == binding.printer_id
                && self.client_id.as_ref() == Some(&binding.client_id)
                && self.client_confirmed
                && !self.name.is_empty()
                && !self.name.chars().any(char::is_control)
                && self.pairing_expires_at_unix_ms.is_none(),
            ResponseSnafu
        );
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum AttemptState {
    Preparing,
    Observing,
    Printed,
    Failed,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_field_names)] // The fields are canonical Server protocol identifiers.
pub(super) struct Attempt {
    pub attempt_id: String,
    pub ready_sequence: i64,
    pub printer_id: String,
    pub client_id: String,
    pub attempt_number: i32,
    pub queue_name: String,
    pub label: String,
    pub state: AttemptState,
    pub claimed_at_unix_ms: i64,
    pub prepare_expires_at_unix_ms: i64,
    pub lease_expires_at_unix_ms: i64,
    pub last_contact_at_unix_ms: i64,
    #[serde(deserialize_with = "required_option")]
    pub last_observed_at_unix_ms: Option<i64>,
    #[serde(deserialize_with = "required_option")]
    pub job_id: Option<u32>,
    #[serde(deserialize_with = "required_option")]
    pub finished_at_unix_ms: Option<i64>,
    pub document_sha256: String,
    pub document_bytes: u64,
    pub document_pages: u32,
}

impl Attempt {
    pub fn active(&self) -> bool {
        matches!(
            self.state,
            AttemptState::Preparing | AttemptState::Observing
        )
    }

    pub fn validate(&self, binding: &Binding, queue: &str) -> Result<()> {
        ensure!(
            canonical_id(&self.attempt_id)
                && self.printer_id == binding.printer_id
                && self.client_id == binding.client_id
                && self.queue_name == queue
                && self.label == format!("Natsume/{}", self.attempt_id)
                && self.ready_sequence > 0
                && (1..=3).contains(&self.attempt_number)
                && (1..=8 * 1024 * 1024).contains(&self.document_bytes)
                && self.document_pages > 0
                && self.document_sha256.len() == 64
                && self
                    .document_sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                && self.job_id != Some(0)
                && (self.finished_at_unix_ms.is_none() == self.active())
                && (self.state != AttemptState::Preparing || self.job_id.is_none())
                && (!matches!(self.state, AttemptState::Observing | AttemptState::Printed)
                    || self.job_id.is_some()),
            ResponseSnafu
        );
        Ok(())
    }

    pub fn same_identity(&self, other: &Self) -> bool {
        self.attempt_id == other.attempt_id
            && self.ready_sequence == other.ready_sequence
            && self.printer_id == other.printer_id
            && self.client_id == other.client_id
            && self.attempt_number == other.attempt_number
            && self.queue_name == other.queue_name
            && self.label == other.label
            && self.document_sha256 == other.document_sha256
            && self.document_bytes == other.document_bytes
            && self.document_pages == other.document_pages
            && self.claimed_at_unix_ms == other.claimed_at_unix_ms
            && self.prepare_expires_at_unix_ms == other.prepare_expires_at_unix_ms
    }

    pub fn job(&self, job_id: u32) -> Job {
        Job {
            queue_name: self.queue_name.clone(),
            job_id,
            label: self.label.clone(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_field_names)] // The fields are canonical Server protocol identifiers.
pub(super) struct Job {
    pub queue_name: String,
    pub job_id: u32,
    pub label: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(
    tag = "kind",
    content = "job",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub(super) enum Report {
    Contact,
    Observed(Job),
    Printed(Job),
    Failed,
    Unknown,
}
