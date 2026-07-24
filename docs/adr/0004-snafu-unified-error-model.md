# ADR-0004: SNAFU unified error model

## Decision

Use SNAFU for typed module errors, context selectors, source chains, optional backtraces and binary `Report`. Remove the `anyhow + thiserror` split from production dependencies.

## Constraints

Stable HTTP/protocol/IPC error codes are explicit mappings. `Whatever`, string-only errors and generic boxing are restricted to documented bootstrap or truly heterogeneous boundaries. Secret-bearing values require redacted `Display` and tests.
