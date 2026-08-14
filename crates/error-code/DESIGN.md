# Error-code contract design

## Invariants

1. Each category variant declares its stable SCREAMING_SNAKE string explicitly; `ErrorCode::as_str` delegates to that value.
2. The seven category enums and unified `ErrorCode` contain exactly the 32 normative public semantics.
3. Public behavior never depends on `Display` or `Debug` output.
4. Domain crates retain typed errors and map every implementation variant explicitly at a boundary.
5. Transport adapters own HTTP, Protobuf, D-Bus, `CommandStatus`, and operator-rendering mappings.
6. Public payloads are constructed from reviewed fields and never from a domain error's source chain or unreviewed `Display` text.
7. `COMMAND_ID_INVALID` means the supplied Command ID was not a canonical lowercase hyphenated UUIDv7; its public response never echoes the rejected value.
8. `COMMAND_REQUEST_CONFLICT` means a canonical Command ID is already bound to a different canonical request.
9. Oversized WSS frames are transport ingress resource-limit failures and do not enter this stable registry.

## Surface model

```text
typed domain error
        |
        | owning adapter's exhaustive match
        v
categorized ErrorCode value
        |
        | boundary-owned exhaustive match
        v
HTTP / Protobuf / D-Bus / CommandStatus
```

The crate is deliberately value-only. It has no Axum, Prost, zbus, UUID, regex, or domain-error dependency. Owning adapters integrate the 32-code registry with generated contracts without deriving behavior from error text.

## Redaction

The shared crate never accepts or formats a source error, so it cannot turn secret, path, rejected input, or source-chain text into a public payload. Each adapter constructs public output from the stable value plus explicitly reviewed fields. Typed validation errors should avoid retaining rejected values when the public boundary does not need them; `COMMAND_ID_INVALID` is the canonical no-echo example.

## Adding a code

1. establish the public semantic and category in the normative contract;
2. add the category variant with an explicit Serde wire string;
3. extend the 32-code registry contract test and its exhaustive stable-string match;
4. add or update exhaustive mappings in every owning domain and transport adapter;
5. verify no adapter exposes rejected input, secrets, paths, or source chains;
6. do not rename or remove a published stable string without a compatibility plan.

## Verification

Run locked workspace formatting, build, tests, Clippy, and the repository policy scan. Registry tests prove the exact 32-code catalog, category conversion, stable Serde values, unknown-code rejection, and exhaustive stable-string mapping. When real adapter mappings exist, their owning crates must provide exhaustive domain-to-stable and stable-to-transport behavior tests.
