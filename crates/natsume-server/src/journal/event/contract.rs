use std::net::IpAddr;

use serde::{Deserialize, Deserializer, Serialize};
use snafu::{IntoError, ResultExt, Snafu};
use uuid::Uuid;

use super::types::{Category, Severity};

pub(super) mod sealed {
    pub trait Sealed {}
}

pub(crate) trait JournalField: sealed::Sealed + Serialize {}

pub(crate) trait JournalEvent: sealed::Sealed {
    const KIND: &'static str;
    const CATEGORY: Category;
    const SEVERITY: Severity;

    fn write(&self, w: &mut DetailsWriter);
}

impl sealed::Sealed for bool {}
impl JournalField for bool {}
impl sealed::Sealed for u16 {}
impl JournalField for u16 {}
impl sealed::Sealed for u32 {}
impl JournalField for u32 {}
impl sealed::Sealed for u64 {}
impl JournalField for u64 {}
impl sealed::Sealed for i64 {}
impl JournalField for i64 {}
impl sealed::Sealed for Uuid {}
impl JournalField for Uuid {}
impl sealed::Sealed for IpAddr {}
impl JournalField for IpAddr {}
impl<T: JournalField> sealed::Sealed for Option<T> {}
impl<T: JournalField> JournalField for Option<T> {}

#[derive(Debug, Snafu)]
pub(crate) enum EventError {
    #[snafu(display("journal event serialization failed"))]
    Serialize { source: serde_json::Error },
    #[snafu(display("journal event exceeds 2 KiB"))]
    TooLarge,
}

pub(crate) struct DetailsWriter {
    fields: serde_json::Map<String, serde_json::Value>,
    error: Option<serde_json::Error>,
}

impl DetailsWriter {
    pub(crate) fn field<T: JournalField>(&mut self, name: &'static str, value: &T) {
        if self.error.is_some() {
            return;
        }
        match serde_json::to_value(value) {
            Ok(value) => {
                self.fields.insert(name.to_owned(), value);
            }
            Err(source) => self.error = Some(source),
        }
    }

    pub(super) fn encode<T: JournalEvent>(event: &T) -> Result<String, EventError> {
        let mut writer = Self {
            fields: serde_json::Map::new(),
            error: None,
        };
        event.write(&mut writer);
        if let Some(source) = writer.error {
            return Err(SerializeSnafu.into_error(source));
        }
        writer.fields.insert("kind".to_owned(), T::KIND.into());
        writer.fields.insert("v".to_owned(), 1.into());
        let encoded = serde_json::to_string(&writer.fields).context(SerializeSnafu)?;
        if encoded.len() > 2048 {
            return TooLargeSnafu.fail();
        }
        Ok(encoded)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub(crate) struct Bounded<T, const N: usize>(T);

impl<T: JournalField, const N: usize> sealed::Sealed for Bounded<Vec<T>, N> {}
impl<T: JournalField, const N: usize> JournalField for Bounded<Vec<T>, N> {}

impl<T, const N: usize> Bounded<Vec<T>, N> {
    #[cfg_attr(not(test), expect(dead_code, reason = "wired in S3"))]
    pub(crate) fn from_typed_targets(values: Vec<T>) -> Result<Self, BoundError> {
        if values.len() > N {
            return Err(BoundError::TooMany);
        }
        Ok(Self(values))
    }
}

impl<'de, T: Deserialize<'de>, const N: usize> Deserialize<'de> for Bounded<Vec<T>, N> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let values = Vec::<T>::deserialize(deserializer)?;
        if values.len() > N {
            return Err(serde::de::Error::custom("too many journal values"));
        }
        Ok(Self(values))
    }
}

#[derive(Debug, Snafu)]
pub(crate) enum BoundError {
    #[snafu(display("too many journal values"))]
    TooMany,
    #[snafu(display("invalid journal seat code"))]
    SeatCode,
    #[snafu(display("invalid journal error token"))]
    ErrorToken,
}
