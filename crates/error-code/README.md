# natsume-error-code

Shared value-only registry for stable Natsume public error identifiers.

## Responsibilities

- define 34 stable codes in the `common`, `operator`, `enrollment`, `control`, `device`, `session`, and `home` categories;
- give every category variant an explicit stable wire string and Serde representation;
- provide the unified `ErrorCode` value for boundaries that carry codes from multiple categories;
- keep the registry independent of domain errors and transport runtimes.

## Consumers

Production consumers keep typed errors in their owning modules and use explicit exhaustive matches to select a categorized stable code. HTTP status, error response body, Protobuf, D-Bus, `CommandStatus`, logging, and operator-facing rendering remain owned by their respective adapters.

## Command request errors

- `COMMAND_ID_INVALID` means a Command ID is not a canonical lowercase hyphenated UUIDv7. The HTTP adapter maps it to `400` and must not echo the rejected value.
- `COMMAND_REQUEST_CONFLICT` means a canonical Command ID is already bound to a different canonical request. The HTTP adapter maps it to `409`.

## Usage

```rust
use natsume_error_code::{ErrorCode, control::ControlErrorCode};

enum CommandValidationError {
    MissingBody,
    UnknownKind,
}

fn stable_code(error: &CommandValidationError) -> ErrorCode {
    match error {
        CommandValidationError::MissingBody | CommandValidationError::UnknownKind => {
            ErrorCode::Control(ControlErrorCode::ProtocolInvalidEnvelope)
        }
    }
}

assert_eq!(
    stable_code(&CommandValidationError::MissingBody).as_str(),
    "PROTOCOL_INVALID_ENVELOPE"
);
```

## Non-goals

- domain errors, retry policy, or authorization decisions;
- transport status/title/error-name mappings or response construction;
- redaction wrappers, reports, or source-error formatting;
- Axum, Prost, WSS, or zbus runtime behavior;
- deriving behavior or stable strings from `Display`, `Debug`, or Rust variant names.

See [`DESIGN.md`](DESIGN.md) and [`ADR-0036`](../../docs/adr/0036-error-architecture-and-public-codes.md).
