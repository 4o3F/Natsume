# Shared Rust contracts

Only stable multi-consumer contracts live here:

- `error-code`: stable error strings, explicit HTTP/protocol/D-Bus mappings and report redaction; domain crates retain typed SNAFU errors;
- `device-protocol`: Protobuf wire schema for provisioning-window Enrollment and Device Token-authenticated WSS control; Commands carry no certificate or token issuance;
- `local-control-api`: typed D-Bus values/interfaces, desktop-only Session lock, no Caddy mutation;
- `machine-identity`: pure candidate derivation and startup comparison; no Linux I/O, installation instance or alias graph.

No generic `common`, `utils` or `helpers` crate is allowed.
