//! Redacted wrappers and report sanitization.

use std::{error::Error, fmt, sync::OnceLock};

use regex::Regex;

use crate::{AsErrorCode, ErrorCode};

static PRIVATE_KEY_BLOCK: OnceLock<Regex> = OnceLock::new();
static CSR_BLOCK: OnceLock<Regex> = OnceLock::new();
static SECRET_MARKER: OnceLock<Regex> = OnceLock::new();
static SENSITIVE_HEADER: OnceLock<Regex> = OnceLock::new();
static URL_USERINFO: OnceLock<Regex> = OnceLock::new();
static CREDENTIAL_ASSIGNMENT: OnceLock<Regex> = OnceLock::new();
static SOURCE_CHAIN_LINE: OnceLock<Regex> = OnceLock::new();
static POSIX_PATH: OnceLock<Regex> = OnceLock::new();
static WINDOWS_PATH: OnceLock<Regex> = OnceLock::new();
static LONG_HEX: OnceLock<Regex> = OnceLock::new();
static LONG_BASE64: OnceLock<Regex> = OnceLock::new();

/// Value whose `Debug` and `Display` representations are always opaque.
#[derive(Clone, PartialEq, Eq)]
pub struct Redacted<T>(T);

impl<T> Redacted<T> {
    /// Wraps a value that must never be formatted directly.
    #[must_use]
    pub const fn new(value: T) -> Self {
        Self(value)
    }

    /// Exposes the value only to code that explicitly opts into secret handling.
    #[must_use]
    pub const fn expose(&self) -> &T {
        &self.0
    }

    /// Consumes the wrapper and returns the value.
    #[must_use]
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> fmt::Debug for Redacted<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

impl<T> fmt::Display for Redacted<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

/// Sanitized text that is safe to format in an operator-facing report.
#[derive(Clone, PartialEq, Eq)]
pub struct RedactedString(String);

impl RedactedString {
    /// Returns the sanitized text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes the wrapper and returns the sanitized text.
    #[must_use]
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl fmt::Debug for RedactedString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl fmt::Display for RedactedString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Stable code plus sanitized domain-error text for a binary report boundary.
#[derive(Clone, PartialEq, Eq)]
pub struct CodedReport {
    code: ErrorCode,
    detail: RedactedString,
}

impl CodedReport {
    /// Builds a report from an explicitly coded domain error.
    #[must_use]
    pub fn from_error<E>(error: &E) -> Self
    where
        E: AsErrorCode + fmt::Display,
    {
        Self {
            code: error.error_code(),
            detail: redact_report(&error.to_string()),
        }
    }

    /// Returns the stable code carried by this report.
    #[must_use]
    pub const fn code(&self) -> ErrorCode {
        self.code
    }

    /// Returns the sanitized non-stable detail.
    #[must_use]
    pub const fn detail(&self) -> &RedactedString {
        &self.detail
    }
}

impl fmt::Debug for CodedReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CodedReport")
            .field("code", &self.code.as_str())
            .field("detail", &self.detail)
            .finish()
    }
}

impl fmt::Display for CodedReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code.as_str(), self.detail)
    }
}

impl Error for CodedReport {}

/// Sanitizes a report or log blob before it crosses an operator-facing boundary.
#[must_use]
pub fn redact_report(input: &str) -> RedactedString {
    let mut output = replace_all(
        &SOURCE_CHAIN_LINE,
        r"(?im)^\s*(?:caused by:|source:|at\s+.*(?:\.rs:\d+|src[/\\])).*$",
        input,
        "[REDACTED_SOURCE_CHAIN]",
    );
    output = replace_all(
        &PRIVATE_KEY_BLOCK,
        r"(?s)-----BEGIN (?:[A-Z0-9]+ )*PRIVATE KEY-----.*?-----END (?:[A-Z0-9]+ )*PRIVATE KEY-----",
        &output,
        "[REDACTED_PRIVATE_KEY]",
    );
    output = replace_all(
        &CSR_BLOCK,
        r"(?s)-----BEGIN (?:NEW )?CERTIFICATE REQUEST-----.*?-----END (?:NEW )?CERTIFICATE REQUEST-----",
        &output,
        "[REDACTED_CSR]",
    );
    output = replace_all(
        &SECRET_MARKER,
        r"(?s)-----BEGIN (?:[A-Z0-9]+ )*(?:PRIVATE KEY|CERTIFICATE REQUEST)-----.*\z",
        &output,
        "[REDACTED_SECRET_REMAINDER]",
    );
    output = replace_all(
        &SENSITIVE_HEADER,
        r"(?im)^\s*(?:authorization|proxy-authorization|cookie|set-cookie)\s*:[^\r\n]*$",
        &output,
        "[REDACTED_HEADER]",
    );
    output = replace_all(
        &URL_USERINFO,
        r"(?i)\b([a-z][a-z0-9+.-]*://)[^/\s:@]+:[^@\s/]+@",
        &output,
        "${1}[REDACTED_USERINFO]@",
    );
    output = replace_all(
        &CREDENTIAL_ASSIGNMENT,
        r#"(?i)\b(?:password|passwd|token|secret|api[_-]?key)\s*[:=]\s*(?:"[^"]*"|'[^']*'|[^\s,;]+)"#,
        &output,
        "[REDACTED_CREDENTIAL]",
    );
    output = replace_all(
        &POSIX_PATH,
        r#"(?m)(^|[\s"'(=])/(?:[A-Za-z0-9._~-]+/)*[A-Za-z0-9._~-]+"#,
        &output,
        "${1}[REDACTED_PATH]",
    );
    output = replace_all(
        &WINDOWS_PATH,
        r"(?i)\b[A-Z]:\\(?:[^\s\\]+\\)*[^\s\\]+",
        &output,
        "[REDACTED_PATH]",
    );
    output = replace_all(
        &LONG_HEX,
        r"\b[0-9A-Fa-f]{64,}\b",
        &output,
        "[REDACTED_HEX]",
    );
    output = replace_all(
        &LONG_BASE64,
        r"[A-Za-z0-9+/_-]{48,}={0,2}",
        &output,
        "[REDACTED_BASE64]",
    );

    RedactedString(output)
}

fn replace_all(
    slot: &'static OnceLock<Regex>,
    pattern: &'static str,
    input: &str,
    replacement: &str,
) -> String {
    regex(slot, pattern)
        .replace_all(input, replacement)
        .into_owned()
}

fn regex(slot: &'static OnceLock<Regex>, pattern: &'static str) -> &'static Regex {
    slot.get_or_init(|| match Regex::new(pattern) {
        Ok(regex) => regex,
        Err(error) => panic!("invalid built-in redaction pattern: {error}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockError(&'static str);

    impl fmt::Display for MockError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str(self.0)
        }
    }

    impl AsErrorCode for MockError {
        fn error_code(&self) -> ErrorCode {
            ErrorCode::VaultCorrupt
        }
    }

    #[derive(Debug)]
    struct SensitiveSource;

    impl fmt::Display for SensitiveSource {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("source at /home/operator/private password=hunter2")
        }
    }

    impl Error for SensitiveSource {}

    #[derive(Debug)]
    struct ChainedMockError {
        source: SensitiveSource,
    }

    impl fmt::Display for ChainedMockError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("vault operation failed")
        }
    }

    impl Error for ChainedMockError {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            Some(&self.source)
        }
    }

    impl AsErrorCode for ChainedMockError {
        fn error_code(&self) -> ErrorCode {
            ErrorCode::VaultCorrupt
        }
    }

    fn private_key_marker(boundary: &str) -> String {
        ["-----", boundary, " PRI", "VATE KEY-----"].concat()
    }

    fn certificate_request_marker(boundary: &str) -> String {
        ["-----", boundary, " CERTIFICATE RE", "QUEST-----"].concat()
    }

    #[test]
    fn raw_redacted_values_never_format_the_inner_value() {
        let secret = Redacted::new("operator-password");

        assert_eq!(format!("{secret}"), "[REDACTED]");
        assert_eq!(format!("{secret:?}"), "[REDACTED]");
        assert_eq!(secret.expose(), &"operator-password");
    }

    #[test]
    fn report_sanitizer_removes_all_required_canaries() {
        let input = format!(
            r"operation failed
{}
MC4CAQAwBQYDK2VwBCIEIJ8examplePrivateKeyMaterialThatIsLongEnoughXX
{}
{}
MIICexampleCSRDataThatShouldNotAppearInReportsOrProblemDetailsXXXX
{}
password=super-secret-pass
token: abcdef
Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.payload
Cookie: session=deadbeefsessionvalue
origin=https://operator:super-url-secret@example.invalid/api
path was /home/operator/natsume/secret.pem
also C:\Users\operator\vault.key
base64: QUJDREVGR0hJSktMTU5PUFFSU1RVVldYWVphYmNkZWZnaGlqa2xtbm9wcXJzdHV2d3h5ejAxMjM0NTY3ODkrLw==
hex: 0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
at /home/operator/natsume/server/src/main.rs:18:5
",
            private_key_marker("BEGIN"),
            private_key_marker("END"),
            certificate_request_marker("BEGIN"),
            certificate_request_marker("END")
        );
        let redacted = redact_report(&input);
        let text = redacted.as_str();

        for canary in [
            "PRIVATE KEY",
            "CERTIFICATE REQUEST",
            "super-secret-pass",
            "abcdef",
            "Bearer ",
            "deadbeefsessionvalue",
            "super-url-secret",
            "/home/operator",
            r"C:\Users",
            "QUJDREVGR0hJSktMTU5PUFFSU1RVVldY",
            "0123456789abcdef0123456789abcdef",
            "server/src/main.rs",
        ] {
            assert!(!text.contains(canary), "canary survived: {canary}: {text}");
        }

        assert_eq!(format!("{redacted}"), text);
        assert_eq!(format!("{redacted:?}"), text);
    }

    #[test]
    fn unterminated_secret_blocks_are_redacted_to_end_of_input() {
        let input = format!(
            "operation failed\n{}\nunterminated-secret-value",
            private_key_marker("BEGIN")
        );
        let redacted = redact_report(&input);

        assert_eq!(
            redacted.as_str(),
            "operation failed\n[REDACTED_SECRET_REMAINDER]"
        );
        assert!(!redacted.as_str().contains("unterminated-secret-value"));
    }

    #[test]
    fn coded_report_uses_stable_code_and_sanitized_detail() {
        let error = MockError("vault at /var/lib/natsume/vault.db password=hunter2");
        let report = CodedReport::from_error(&error);
        let display = format!("{report}");
        let debug = format!("{report:?}");

        assert_eq!(report.code(), ErrorCode::VaultCorrupt);
        assert!(display.starts_with("VAULT_CORRUPT: "));
        assert!(!display.contains("/var/lib/natsume"));
        assert!(!display.contains("hunter2"));
        assert!(!debug.contains("/var/lib/natsume"));
        assert!(!debug.contains("hunter2"));
    }

    #[test]
    fn coded_report_does_not_retain_a_sensitive_source_chain() {
        let error = ChainedMockError {
            source: SensitiveSource,
        };
        assert!(error.source().is_some());

        let report = CodedReport::from_error(&error);
        let display = format!("{report}");
        let debug = format!("{report:?}");

        assert_eq!(display, "VAULT_CORRUPT: vault operation failed");
        assert!(!display.contains("/home/operator"));
        assert!(!display.contains("hunter2"));
        assert!(!debug.contains("/home/operator"));
        assert!(!debug.contains("hunter2"));
        assert!(report.source().is_none());
    }
}
