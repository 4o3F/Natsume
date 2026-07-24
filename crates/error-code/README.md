# natsume-error-code

Shared Phase 0 contract for stable Natsume error identifiers and boundary-safe rendering.

## Responsibilities

- define the 33 Phase 0 stable error strings, including the cross-desktop Session Agent lifecycle set;
- map each code explicitly to HTTP status/title, protocol string and D-Bus error name;
- provide RFC 9457-shaped Problem Details without a default detail field;
- provide redacted wrappers and reports for operator-facing error boundaries.

## Consumers

`server`, `client/device-daemon`, `client/privileged-helper` and `client/session-agent` are production consumers. A consumer keeps its own typed SNAFU errors and implements `AsErrorCode` or an equivalent exhaustive mapping.

## Usage

```rust
use natsume_error_code::{AsErrorCode, CodedReport, ErrorCode};

# struct DomainError;
# impl core::fmt::Display for DomainError {
#     fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
#         formatter.write_str("operation failed")
#     }
# }
impl AsErrorCode for DomainError {
    fn error_code(&self) -> ErrorCode {
        ErrorCode::VaultCorrupt
    }
}

let report = CodedReport::from_error(&DomainError);
assert_eq!(report.code(), ErrorCode::VaultCorrupt);
```

## Non-goals

- domain error definitions or retry policy;
- Axum response construction;
- Prost messages or QUIC framing;
- zbus introspection/runtime behavior;
- parsing `Display` text to determine program behavior.

See [`DESIGN.md`](DESIGN.md) and [`ADR-0019`](../../docs/adr/0019-stable-error-code-registry.md).
