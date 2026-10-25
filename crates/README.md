# Rust workspace packages

The workspace contains the eight application and contract packages below. XLSX parsing and templates are internal to Server Import; school-logo file and image processing are internal to Server Contest. Server Print admits documents from the standalone DOMjudge wrapper. Photo and shared contest-domain packages are not implemented. The sole architecture authority is [docs/architecture.md](../docs/architecture.md#17-crate模块与依赖).

| Directory / Cargo package | Responsibility | Binary targets |
| --- | --- | --- |
| [`natsume-server`](natsume-server/) | HTTPS APIs, contest data, workstation control and durable print admission | `natsume-server`, `export-typescript` |
| [`natsume-print-domjudge`](natsume-print-domjudge/) | Independent DOMjudge HTTPS admission client and fixed Typst wrapper | `natsume-print-domjudge` |
| [`natsume-print-client`](natsume-print-client/) | Windows pairing, durable task execution and recovery, native printing and exact Job ID observation | `natsume-print-client` (Windows) |
| [`natsume-workstation-daemon`](natsume-workstation-daemon/) | Workstation identity, control connection and reconciliation | `natsume-device-daemon` |
| [`natsume-workstation-helper`](natsume-workstation-helper/) | Closed privileged operations over typed D-Bus | `natsume-privileged-helper` |
| [`natsume-workstation-agent`](natsume-workstation-agent/) | Waiting and seat-binding UI | `natsume-session-agent` |
| [`natsume-workstation-protocol`](natsume-workstation-protocol/) | Device Control Protobuf wire schema, open ErrorCode-token grammar and Rust facade | — |
| [`natsume-workstation-ipc`](natsume-workstation-ipc/) | Typed D-Bus values/interfaces and desktop-only Session lock; no Caddy mutation | — |
