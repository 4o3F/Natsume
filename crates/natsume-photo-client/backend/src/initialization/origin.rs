//! Validated server trust origin, distinct from the user's configuration input.
use serde::{Deserialize, Serialize};
use snafu::{ResultExt as _, Snafu, ensure};

/// Origin validation failures never retain the rejected URL.
#[derive(Debug, Snafu)]
pub enum OriginError {
    #[snafu(display("server URL could not be parsed"))]
    Url { source: url::ParseError },
    #[snafu(display("expected a canonical HTTPS server origin"))]
    Invalid,
}
type Result<T> = std::result::Result<T, OriginError>;

/// A canonical HTTPS origin with no credentials, business path, query or fragment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ServerOrigin(String);
impl ServerOrigin {
    /// Validates user input and normalizes its origin.
    /// # Errors
    /// Rejects URLs outside the HTTPS origin contract.
    pub fn parse(value: &str) -> Result<Self> {
        let url = url::Url::parse(value).context(UrlSnafu)?;
        ensure!(
            url.scheme() == "https"
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.path() == "/"
                && url.query().is_none()
                && url.fragment().is_none(),
            InvalidSnafu
        );
        Ok(Self(url.origin().ascii_serialization()))
    }
    /// Borrows the canonical origin for a transport or platform adapter.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl TryFrom<String> for ServerOrigin {
    type Error = OriginError;
    fn try_from(value: String) -> Result<Self> {
        let origin = Self::parse(&value)?;
        ensure!(origin.0 == value, InvalidSnafu);
        Ok(origin)
    }
}
impl From<ServerOrigin> for String {
    fn from(value: ServerOrigin) -> Self {
        value.0
    }
}
impl std::fmt::Display for ServerOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
#[path = "../../tests/unit/origin.rs"]
mod tests;
