use serde::{Deserialize, Deserializer, Serialize};
use uuid::{Uuid, Variant, Version};

use crate::{Error, Result};

#[derive(Serialize)]
pub(crate) struct Registration<'a> {
    pub protocol_version: u8,
    pub request_id: &'a str,
    pub account: &'a str,
    pub original_filename: &'a str,
    pub source_sha256: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FailureCode {
    CompileTimeout,
    CompileFailed,
    DocumentTooLarge,
    WrapperDeadline,
}

#[derive(Serialize)]
pub(crate) struct GenerationFailure {
    pub protocol_version: u8,
    pub code: FailureCode,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fact {
    pub protocol_version: u8,
    pub request_id: String,
    pub account_id: String,
    pub account: String,
    pub team_name: String,
    #[serde(deserialize_with = "nullable")]
    pub seat_label: Option<String>,
    pub state: State,
    #[serde(deserialize_with = "nullable")]
    pub document: Option<Document>,
    #[serde(deserialize_with = "nullable")]
    pub admission: Option<Admission>,
    #[serde(deserialize_with = "nullable")]
    pub generation_failure: Option<FailureCode>,
}

// deserialize_with intentionally makes nullable fields required on the wire.
fn nullable<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    de: D,
) -> std::result::Result<Option<T>, D::Error> {
    Option::deserialize(de)
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Preparing,
    GenerationFailed,
    Ready,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Document {
    pub sha256: String,
    pub bytes: u64,
    pub pages: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Admission {
    pub ready_sequence: u64,
    pub ahead_tasks: u64,
    pub ahead_pages: u64,
    pub in_progress_tasks: u64,
}

impl Fact {
    pub(crate) fn validate(&self, request_id: &str, account: &str) -> Result<()> {
        let valid_state = match self.state {
            State::Preparing => {
                self.document.is_none()
                    && self.admission.is_none()
                    && self.generation_failure.is_none()
            }
            State::GenerationFailed => {
                self.document.is_none()
                    && self.admission.is_none()
                    && self.generation_failure.is_some()
            }
            State::Ready => {
                self.generation_failure.is_none()
                    && self.document.as_ref().is_some_and(|d| {
                        sha256(&d.sha256) && (1..=8 * 1024 * 1024).contains(&d.bytes) && d.pages > 0
                    })
                    && self.admission.as_ref().is_some_and(|a| {
                        a.ready_sequence > 0
                            && a.in_progress_tasks == 0
                            && a.ahead_tasks < a.ready_sequence
                            && a.ahead_pages >= a.ahead_tasks
                    })
            }
        };
        if self.protocol_version != 1
            || self.request_id != request_id
            || !uuid_v7(&self.request_id)
            || !uuid_v7(&self.account_id)
            || self.account != account
            || !valid_state
        {
            return Err(Error::InvalidResponse);
        }
        Ok(())
    }

    pub(crate) fn same_registration(&self, other: &Self) -> bool {
        self.account_id == other.account_id
            && self.account == other.account
            && self.team_name == other.team_name
            && self.seat_label == other.seat_label
    }
}

pub(crate) fn uuid_v7(value: &str) -> bool {
    Uuid::parse_str(value).is_ok_and(|id| {
        id.get_version() == Some(Version::SortRand)
            && id.get_variant() == Variant::RFC4122
            && id.hyphenated().to_string() == value
    })
}

pub(crate) fn sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[cfg(test)]
mod tests {
    use super::uuid_v7;

    #[test]
    fn request_id_requires_canonical_uuid_v7_and_rfc_variant() {
        assert!(uuid_v7("01900000-0000-7000-8000-00000000000a"));
        for id in [
            "01900000-0000-7000-8000-00000000000A",
            "01900000-0000-7000-0000-000000000001",
            "01900000-0000-4000-8000-000000000001",
            "01900000000070008000000000000001",
        ] {
            assert!(!uuid_v7(id), "invalid ID accepted: {id}");
        }
    }
}
