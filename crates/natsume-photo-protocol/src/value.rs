//! Validated identities used by the Photo initialization protocol.
use serde::{Deserialize, Serialize};
use snafu::{Snafu, ensure};
use std::{fmt, str::FromStr};

/// Invalid identifiers and digests never include the rejected value.
#[derive(Debug, Snafu)]
pub enum ValueError {
    #[snafu(display("expected a canonical UUIDv7"))]
    Uuid,
    #[snafu(display("expected a lowercase SHA-256 digest"))]
    Digest,
}

macro_rules! identity {
    ($($name:ident),+ $(,)?) => {$ (
        /// A canonical `UUIDv7` with a distinct protocol role.
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[cfg_attr(feature = "typescript", derive(specta::Type))]
        #[serde(try_from = "String", into = "String")]
        #[cfg_attr(feature = "typescript", specta(type = String))]
        pub struct $name(String);
        impl $name {
            /// Allocates a new identity for this role.
            #[must_use]
            pub fn generate() -> Self { Self(uuid::Uuid::now_v7().to_string()) }
            /// Borrows the canonical wire representation.
            #[must_use]
            pub fn as_str(&self) -> &str { &self.0 }
        }
        impl FromStr for $name {
            type Err = ValueError;
            fn from_str(value: &str) -> Result<Self, Self::Err> {
                ensure!(is_canonical_uuid_v7(value), UuidSnafu);
                Ok(Self(value.to_owned()))
            }
        }
        impl TryFrom<String> for $name {
            type Error = ValueError;
            fn try_from(value: String) -> Result<Self, Self::Error> {
                ensure!(is_canonical_uuid_v7(&value), UuidSnafu);
                Ok(Self(value))
            }
        }
        impl From<$name> for String { fn from(value: $name) -> Self { value.0 } }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(&self.0) }
        }
    )+};
}
identity!(
    SourceId,
    EnrollmentId,
    DatasetId,
    InitializationId,
    InitializationRequestId
);

/// A validated lowercase SHA-256 digest, distinct from identities and keys.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(specta::Type))]
#[serde(try_from = "String", into = "String")]
#[cfg_attr(feature = "typescript", specta(type = String))]
pub struct Sha256Digest(String);
impl Sha256Digest {
    /// Hashes bytes without imposing any storage or network policy.
    #[must_use]
    pub fn of(bytes: &[u8]) -> Self {
        use sha2::Digest as _;
        Self(hex::encode(sha2::Sha256::digest(bytes)))
    }
    /// Borrows the canonical digest.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl FromStr for Sha256Digest {
    type Err = ValueError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        ensure!(is_lowercase_sha256(value), DigestSnafu);
        Ok(Self(value.to_owned()))
    }
}
impl TryFrom<String> for Sha256Digest {
    type Error = ValueError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}
impl From<Sha256Digest> for String {
    fn from(value: Sha256Digest) -> Self {
        value.0
    }
}
impl fmt::Display for Sha256Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Checks canonical spelling and UUID version at untyped legacy boundaries.
#[must_use]
pub fn is_canonical_uuid_v7(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|id| {
        id.get_version_num() == 7
            && id.get_variant() == uuid::Variant::RFC4122
            && id.to_string() == value
    })
}
/// Checks a lowercase hexadecimal SHA-256 representation.
#[must_use]
pub fn is_lowercase_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}
