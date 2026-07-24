# Shared Rust contracts

Only stable multi-consumer contracts live here:

- `error-code`: stable error strings, explicit HTTP/protocol/D-Bus mappings and report redaction; domain crates retain typed SNAFU errors;
- `device-protocol`: Protobuf wire schema for mandatory-mTLS QUIC, Device-only Enrollment and `SYNC_STATE`-bound Gateway certificate requests;
- `local-control-api`: typed D-Bus values/interfaces, desktop-only Session lock, no Caddy mutation;
- `machine-identity`: pure candidate derivation and startup comparison; no Linux I/O, installation instance or alias graph.

No generic `common`, `utils` or `helpers` crate is allowed.
