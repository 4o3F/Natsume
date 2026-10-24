# Web Panel

`@natsume/web` is a self-contained Node project under `web/`; the repository root uses Deno for orchestration. It owns React, shadcn/ui, browser tests, and named HTTP calls in `src/api/client.ts`. Server HTTP DTOs derive `specta::Type`; Specta automatically collects them without a separate type registry. Run `deno task prepare rust web` once, then `deno task api` to export and format `src/api/generated/types.ts`, which is committed and checked for regeneration differences in CI. Web builds consume this file and never invoke Cargo. For Web-only work, run `deno task prepare web`, followed by `deno task check web` or `deno task build web`; pnpm remains the Web package manager and Deno does not install another set of Web dependencies.

## Visual design

Web 界面的视觉规范统一见根目录 [DESIGN.md](../DESIGN.md)。修改布局、组件和样式前先阅读该文件；业务契约与运行边界仍以 [架构文档](../docs/architecture.md)为准。

## Development and integration

`package.json`, `pnpm-lock.yaml`, `pnpm-workspace.yaml` and `.node-version` all live in `web/`, and dependencies are installed only in `web/node_modules`. The pnpm configuration retains dependency overrides and install-script permissions; there is no root Node package or multi-package Web workspace. From the repository root, use `pnpm --dir web <command>`; inside this directory, use `pnpm <command>`.

`pnpm typecheck` checks application code, browser tests and their fixtures through the same TypeScript configuration. When Playwright starts its own preview server, `pnpm e2e` builds the production Web output before launching Vite preview; the Web CI job uses this path without a separate production build.

## Browser verification

Run `deno task prepare web`, then `pnpm --dir web e2e` from the repository root. Playwright builds the production Web output and tests it in Chromium with mocked `/api/v2` responses. These tests verify browser interactions, permission-dependent controls, session isolation, one-time credential display, polling failures and recovery; they do not demonstrate a real Server deployment or Windows printing.

Shared interaction helpers live in `e2e/fixtures/ui.ts`. Open the mobile navigation or detail sheet before interacting with its controls, and locate the seat column by its header rather than assuming it is the first column. Use the virtual clock for polling and delayed responses. Keep assertions about unknown or stale states, explicit confirmations, original-operation retries and session cleanup when adapting tests to layout changes.

## API boundaries

The generated bindings describe JSON types, not HTTP routes or runtime validation. The client explicitly owns methods, paths, CSV queries, JSON requests, raw XLSX uploads and ZIP downloads, while retaining session cancellation and unauthorized-session handling. Responses use serialization-facing types where Serde conditionally omits a field. Existing JSON integers remain TypeScript `number`; this does not add lossless 64-bit transport.

**2026-08-15 修订**：面板随 Server Deb 交付，并由 `natsume-server serve` 同源托管；开发迭代仍使用经 Vite proxy 转发的 `pnpm dev`。

The target Panel surface follows `docs/architecture.md`: contest preparation, provisioning, Enrollment review, Device lifecycle, Gateway and Binding facts, Runtime/Session/Home targets, and readiness views. It never renders or retrieves contest passwords, Gateway private keys, or CSRs. It does not rely on SSE, ChangeEvent streams, an audit page, or an HTTP Command resource.

The Panel calls only implemented Server HTTP routes. New surfaces are added together with their Server adapter, named client calls, tests and regenerated types; the Panel does not keep placeholder routes for unimplemented resources.

Enrollment shows the enrollment window state and lets administrators open or close it through `/api/v2/provisioning-window`; viewers can only read the state. The window closes whenever the Server restarts, and the Panel polls for changes.

Preparation downloads the fixed Teams XLSX template and previews a complete roster, including team/school metadata, generated INST IDs, account and password changes, seat mappings and affected devices. Removing an occupied seat blocks commit; other affected bindings are shown for confirmation.

The Server persists only a redacted diff and fingerprints. The browser keeps the exact reviewed File and preview token in the current login session, including during page navigation. Upload and commit send raw XLSX bytes; commit sends authorization in `x-natsume-preview-token`. A reload or session change requires discard and re-upload. Passwords are never rendered or stored in browser storage.
