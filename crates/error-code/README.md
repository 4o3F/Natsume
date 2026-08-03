# natsume-error-code

Shared Phase 0 contract for stable Natsume error identifiers and boundary-safe rendering.

## Responsibilities

- define the 30 Phase 0 stable error strings, including the cross-desktop Session Agent lifecycle set and Command request failures;
- map each code explicitly to HTTP status/title, protocol string and D-Bus error name;
- provide RFC 9457-shaped Problem Details without a default detail field;
- provide redacted wrappers and reports for operator-facing error boundaries.

## Consumers

`server`, `client/device-daemon`, `client/privileged-helper` and `client/session-agent` are production consumers. A consumer keeps its own typed SNAFU errors and implements `AsErrorCode` or an equivalent exhaustive mapping.

## Command request errors

- `COMMAND_ID_INVALID` maps to HTTP `400` when a Command ID is not a canonical lowercase hyphenated UUIDv7. Public responses must not echo the invalid value.
- `COMMAND_REQUEST_CONFLICT` maps to HTTP `409` when a canonical Command ID is already bound to a different normalized request.

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
- Prost messages or WSS frame handling;
- zbus introspection/runtime behavior;
- parsing `Display` text to determine program behavior.

See [`DESIGN.md`](DESIGN.md) and [`ADR-0036`](../../docs/adr/0036-error-architecture-and-public-codes.md).
