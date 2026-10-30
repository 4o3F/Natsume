//! Validated identities used by the Photo initialization protocol.
use serde::{Deserialize, Serialize};
use snafu::{ResultExt as _, Snafu, ensure};
use std::{fmt, str::FromStr};

/// Invalid identifiers and digests never include the rejected value.
#[derive(Debug, Snafu)]
pub enum ValueError {
    #[snafu(display("expected a canonical UUIDv7"))]
    Uuid,
    #[snafu(display("expected a lowercase SHA-256 digest"))]
    Digest,
    #[snafu(display("expected a canonical non-weak Ed25519 public key"))]
    PublicKey,
    #[snafu(display("public key encoding is invalid"))]
    PublicKeyEncoding { source: hex::FromHexError },
    #[snafu(display("public key point is invalid"))]
    PublicKeyPoint {
        source: ed25519_dalek::SignatureError,
    },
}

/// A validated `UUIDv7` whose role prevents mixing different identities.
/// ```compile_fail
/// use natsume_photo_protocol::{SourceId, EnrollmentId};
/// let enrollment: EnrollmentId = SourceId::generate();
/// ```
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Identity<Role> {
    value: String,
    role: std::marker::PhantomData<Role>,
}
// Roles exist only for Rust's type checker; the wire value is always a string.
#[cfg(feature = "typescript")]
impl<Role> specta::Type for Identity<Role> {
    fn definition(types: &mut specta::Types) -> specta::datatype::DataType {
        <String as specta::Type>::definition(types)
    }
}
impl<Role> Serialize for Identity<Role> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.value.serialize(serializer)
    }
}
impl<'de, Role> Deserialize<'de> for Identity<Role> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .try_into()
            .map_err(serde::de::Error::custom)
    }
}
impl<Role> Identity<Role> {
    /// Generates a new `UUIDv7` for this role.
    #[must_use]
    pub fn generate() -> Self {
        Self {
            value: uuid::Uuid::now_v7().to_string(),
            role: std::marker::PhantomData,
        }
    }
    /// Borrows the canonical representation.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.value
    }
}
impl<Role> FromStr for Identity<Role> {
    type Err = ValueError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value.to_owned().try_into()
    }
}
impl<Role> TryFrom<String> for Identity<Role> {
    type Error = ValueError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        ensure!(is_canonical_uuid_v7(&value), UuidSnafu);
        Ok(Self {
            value,
            role: std::marker::PhantomData,
        })
    }
}
impl<Role> From<Identity<Role>> for String {
    fn from(value: Identity<Role>) -> Self {
        value.value
    }
}
impl<Role> fmt::Display for Identity<Role> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.value)
    }
}
/// Distinguishes the Source identity from other UUID roles.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SourceRole {}
/// A canonical `UUIDv7` identifying a Source.
pub type SourceId = Identity<SourceRole>;
/// Distinguishes the Enrollment identity from other UUID roles.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EnrollmentRole {}
/// A canonical `UUIDv7` identifying a Enrollment.
pub type EnrollmentId = Identity<EnrollmentRole>;
/// Distinguishes the Dataset identity from other UUID roles.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DatasetRole {}
/// A canonical `UUIDv7` identifying a Dataset.
pub type DatasetId = Identity<DatasetRole>;
/// Distinguishes the Initialization identity from other UUID roles.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum InitializationRole {}
/// A canonical `UUIDv7` identifying a Initialization.
pub type InitializationId = Identity<InitializationRole>;
/// Distinguishes the `InitializationRequest` identity from other UUID roles.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum InitializationRequestRole {}
/// A canonical `UUIDv7` identifying a `InitializationRequest`.
pub type InitializationRequestId = Identity<InitializationRequestRole>;

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
    is_lowercase_hex_32(value)
}

fn is_lowercase_hex_32(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// Canonical non-weak Ed25519 public key; distinct from a digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(specta::Type))]
#[serde(try_from = "String", into = "String")]
#[cfg_attr(feature = "typescript", specta(type = String))]
pub struct SigningPublicKey(String);
impl SigningPublicKey {
    /// Canonical hexadecimal representation for persistence and signatures.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl TryFrom<String> for SigningPublicKey {
    type Error = ValueError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        ensure!(is_lowercase_hex_32(&value), PublicKeySnafu);
        let mut bytes = [0; 32];
        hex::decode_to_slice(&value, &mut bytes).context(PublicKeyEncodingSnafu)?;
        let key = ed25519_dalek::VerifyingKey::from_bytes(&bytes).context(PublicKeyPointSnafu)?;
        ensure!(!key.is_weak(), PublicKeySnafu);
        Ok(Self(value))
    }
}
impl FromStr for SigningPublicKey {
    type Err = ValueError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value.to_owned().try_into()
    }
}
impl From<SigningPublicKey> for String {
    fn from(value: SigningPublicKey) -> Self {
        value.0
    }
}
