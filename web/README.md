# Web Panel

`@natsume/web` is the only Node workspace package. It owns React, shadcn/ui, browser tests and the generated OpenAPI client. Rust owns the OpenAPI semantics: `cargo run --locked --offline -p natsume-server --bin export-openapi -- web/openapi/natsume.openapi.json` exports the document, then `pnpm --dir web api:generate` formats the JSON and regenerates TypeScript. Both `openapi/natsume.openapi.json` and `src/api/generated/` are committed contract snapshots; Web builds never invoke Cargo.

The Preparation Center exposes one authoritative UTF-8 CSV import, Device-only enrollment, binding, explicit state/secret workflows, Gateway-certificate progress driven by `SYNC_STATE`, session lock/unlock, Machine-ID/Device-SPKI conflict review and readiness checks. It never renders or retrieves contest passwords, Gateway private keys or CSRs.
