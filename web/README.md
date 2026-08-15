# Web Panel

`@natsume/web` is the only Node workspace package. It owns React, shadcn/ui, browser tests, and the generated OpenAPI client. Rust owns the OpenAPI semantics: `cargo run --locked --offline -p natsume-server --bin export-openapi -- web/openapi/natsume.openapi.json` exports the document, then `pnpm --dir web api:generate` formats the JSON and regenerates TypeScript. Both `openapi/natsume.openapi.json` and `src/api/generated/` are committed contract snapshots; Web builds never invoke Cargo.

**2026-08-15 修订**：面板随 Server Deb 交付，并由 `natsume-server serve` 同源托管；开发迭代仍使用经 Vite proxy 转发的 `pnpm dev`。

The planned Panel surface contains the authoritative UTF-8 CSV import, provisioning-window Enrollment, binding, explicit state/secret workflows, Enrollment-time Gateway-certificate progress, session lock/unlock, readiness checks, and polling-based views. It never renders or retrieves contest passwords, Gateway private keys, CSRs, or unredacted audit evidence. It does not rely on SSE or ChangeEvent streams.

Before creating every direct Command, the Panel generates a canonical lowercase hyphenated UUIDv7 `command_id` and uses `PUT /api/v2/commands/{command_id}`. Replaying the same ID with the same canonical request retrieves the existing Command; changing the request under that ID conflicts. A bulk action creates one ID per Device and may attach an optional query-only `group_correlation_id`; it does not create an Operation/Attempt or use `Idempotency-Key`.

This README records the frozen client contract, not a completion claim. Phase 0 does not assert a working Panel mutation flow, HTTP Command handler, dispatcher, or Device journal.
