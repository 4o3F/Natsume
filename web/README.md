# Web Panel

`@natsume/web` is the only Node workspace package. It owns React, shadcn/ui, browser tests, and the generated OpenAPI client. Rust owns the OpenAPI semantics: `cargo run --locked --offline -p natsume-server --bin export-openapi -- web/openapi/natsume.openapi.json` exports the document, then `pnpm --dir web api:generate` formats the JSON and regenerates TypeScript. Both `openapi/natsume.openapi.json` and `src/api/generated/` are committed contract snapshots; Web builds never invoke Cargo.

**2026-08-15 修订**：面板随 Server Deb 交付，并由 `natsume-server serve` 同源托管；开发迭代仍使用经 Vite proxy 转发的 `pnpm dev`。

The target Panel surface follows `docs/architecture.md`: contest preparation, provisioning, Enrollment review, Device lifecycle, Gateway and Binding facts, Runtime/Session/Home targets, and readiness views. It never renders or retrieves contest passwords, Gateway private keys, or CSRs. It does not rely on SSE, ChangeEvent streams, an audit page, or an HTTP Command resource.

Only routes present in the generated OpenAPI snapshot are mounted in the current Panel. Future component surfaces are added together with their Server HTTP adapter and regenerated contract; the Panel does not keep placeholder routes for unimplemented resources.

Enrollment shows the enrollment window state and lets administrators open or close it through `/api/v2/provisioning-window`; viewers can only read the state. The window closes whenever the Server restarts, and the Panel polls for changes.

Preparation downloads the fixed Teams XLSX template and previews a complete roster, including team/school metadata, generated INST IDs, account and password changes, seat mappings and affected devices. Removing an occupied seat blocks commit; other affected bindings are shown for confirmation.

The Server persists only a redacted diff and fingerprints. The browser keeps the exact reviewed File and preview token in the current login session, including during page navigation. Upload and commit send raw XLSX bytes; commit sends authorization in `x-natsume-preview-token`. A reload or session change requires discard and re-upload. Passwords are never rendered or stored in browser storage.
