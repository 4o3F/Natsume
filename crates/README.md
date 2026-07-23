# Shared Rust contracts

Only stable multi-consumer contracts live here:

- `error-code`: stable error strings, explicit HTTP/protocol/D-Bus mappings and report redaction; domain crates retain typed SNAFU errors;
- `device-protocol`: Protobuf wire schema for mandatory-mTLS QUIC, including the `SYNC_STATE`-bound Gateway certificate request/result messages, plus Device-only Enrollment message types;
- `local-control-api`: typed D-Bus values/interfaces, with desktop-only Session lock and no Caddy mutation;
- `machine-identity`: pure candidate derivation, first-ID selection and startup comparison; no Linux I/O, installation instance or alias graph.
