# Shared Rust contracts

Only stable multi-consumer contracts live here:

- `device-protocol`: Protobuf wire schema, open ErrorCode-token grammar and Rust facade for Device Control;
- `local-control-api`: typed D-Bus values/interfaces, desktop-only Session lock, no Caddy mutation;
- `machine-identity`: pure candidate derivation and startup comparison; no Linux I/O, installation instance or alias graph.

Public error codes are owned by the boundary that emits them. HTTP mappings stay
in Server, WSS token validation stays in `device-protocol`, and typed local IPC
failures stay in `local-control-api`; there is no cross-transport error registry.

No generic `common`, `utils` or `helpers` crate is allowed.
