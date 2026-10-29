//! Identities owned only by this client's initialization journal and credential slots.
use natsume_photo_protocol::is_canonical_uuid_v7;
use serde::{Deserialize, Serialize};
use snafu::ensure;
use std::{fmt, str::FromStr};
/// Invalid local identity; the rejected value is never included in diagnostics.
#[derive(Debug, snafu::Snafu)]
#[snafu(display("expected a canonical local UUIDv7"))]
pub struct LocalIdentityError;
macro_rules! identity {
    ($($name:ident),+ $(,)?) => {$ (
        /// A canonical `UUIDv7` with a distinct local role.
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
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
            type Err = LocalIdentityError;
            fn from_str(value: &str) -> Result<Self, Self::Err> {
                ensure!(is_canonical_uuid_v7(value), LocalIdentitySnafu);
                Ok(Self(value.to_owned()))
            }
        }
        impl TryFrom<String> for $name {
            type Error = LocalIdentityError;
            fn try_from(value: String) -> Result<Self, Self::Error> {
                ensure!(is_canonical_uuid_v7(&value), LocalIdentitySnafu);
                Ok(Self(value))
            }
        }
        impl From<$name> for String { fn from(value: $name) -> Self { value.0 } }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(&self.0) }
        }
    )+};
}
identity!(ClientId, PreparedId);
