# Error-code contract design

## Invariants

1. `ErrorCode::as_str` is the only source of stable SCREAMING_SNAKE strings.
2. HTTP, protocol and D-Bus mappings are exhaustive matches over `ErrorCode`.
3. Public behavior never depends on `Display` or `Debug` output.
4. Domain crates retain typed SNAFU errors and map them explicitly at a boundary.
5. Phase 0 Problem Details omit `detail`; fields are not publicly mutable, and any future detail API must accept reviewed redacted text.
6. Secret, path and source-chain content is removed before an operator report crosses a boundary.

## Surface model

```text
typed domain error
        |
        | AsErrorCode / exhaustive match
        v
    ErrorCode
     /  |  \
 HTTP protocol D-Bus
        |
        v
 CodedReport / redaction
```

The crate intentionally has no Axum, Prost or zbus runtime dependency. Step 4 owns framework integration and generated contracts.

## Redaction

`Redacted<T>` never formats the wrapped value. `RedactedString` stores only sanitized text. `CodedReport` combines a stable code with sanitized domain display text and does not expose the original source error.

`redact_report` is defense in depth for operator reports. Domain error messages must still avoid embedding raw secrets or paths. Built-in patterns cover private-key/CSR PEM, credential and HTTP authentication fields, URL userinfo, absolute paths, long encoded values and source-chain lines.

## Adding a code

1. add an `ErrorCode` variant and its stable string;
2. add it to `ALL_ERROR_CODES`;
3. add explicit HTTP status/title and D-Bus name mappings;
4. assign a domain owner and typed error mapping;
5. update ADR/requirements and contract tests;
6. do not rename an already published stable string without a compatibility decision.

## Verification

Run workspace fmt/check/Clippy/tests, crate doc tests, `cargo deny check` and the repository policy scan. These local results do not mark Phase 0 requirements or G0 items as PASS.
