# CLAUDE.md

Behavioral guidelines to reduce common LLM coding mistakes. Merge with project-specific instructions as needed.

**Tradeoff:** These guidelines bias toward caution over speed. For trivial tasks, use judgment.

## 1. Think Before Coding

**Don't assume. Don't hide confusion. Surface tradeoffs.**

Before implementing:
- State your assumptions explicitly. If uncertain, ask.
- If multiple interpretations exist, present them - don't pick silently.
- If a simpler approach exists, say so. Push back when warranted.
- If something is unclear, stop. Name what's confusing. Ask.

## 2. Simplicity First

**Minimum code that solves the problem. Nothing speculative.**

- No features beyond what was asked.
- No abstractions for single-use code.
- Do not add test-only fields to production structs, including fields guarded by `#[cfg(test)]`. Keep test state in fixtures and pass dependencies explicitly; do not make unused fields unconditional to bypass this rule.
- No "flexibility" or "configurability" that wasn't requested.
- No error handling for impossible scenarios.
- If you write 200 lines and it could be 50, rewrite it.

Ask yourself: "Would a senior engineer say this is overcomplicated?" If yes, simplify.

## 3. Surgical Changes

**Touch only what you must. Clean up only your own mess.**

When editing existing code:
- Don't "improve" adjacent code, comments, or formatting.
- Don't refactor things that aren't broken.
- Match existing style, even if you'd do it differently.
- If you notice unrelated dead code, mention it - don't delete it.

When your changes create orphans:
- Remove imports/variables/functions that YOUR changes made unused.
- Don't remove pre-existing dead code unless asked.

The test: Every changed line should trace directly to the user's request.

## 4. Goal-Driven Execution

**Define success criteria. Loop until verified.**

Transform tasks into verifiable goals:
- "Add validation" → "Write tests for invalid inputs, then make them pass"
- "Fix the bug" → "Write a test that reproduces it, then make it pass"
- "Refactor X" → "Ensure tests pass before and after"

For multi-step tasks, state a brief plan:
```
1. [Step] → verify: [check]
2. [Step] → verify: [check]
3. [Step] → verify: [check]
```

Strong success criteria let you loop independently. Weak criteria ("make it work") require constant clarification.

---

**These guidelines are working if:** fewer unnecessary changes in diffs, fewer rewrites due to overcomplication, and clarifying questions come before implementation rather than after mistakes.

## Database access

- Before adding or changing database access, read comparable implementations in existing components, such as `crates/natsume-server/src/component/contest/db.rs`, `crates/natsume-server/src/component/device/db/devices.rs`, and `crates/natsume-server/src/component/import/db/accounts.rs`. Follow the established approach instead of introducing a different database style for a new feature.
- Use the generated `crate::diesel_schema` tables and Diesel's typed query builder for ordinary reads, joins, filters, ordering, aggregates, inserts, updates, deletes, and upserts. Select the required columns explicitly. Do not replace these operations with handwritten SQL strings or duplicate table and column definitions in strings.
- For simple query results, prefer tuples or local tuple aliases, then construct named application values in the owning component's private database module. Introduce a separate row struct only when it materially improves mapping or persisted-data validation; do not add `QueryableByName` structs merely to support avoidable raw SQL.
- Keep database-specific row types, SQL type annotations, and persistence error handling inside the owning component's private `db.rs` or `db/` modules. Do not move database details into domain types or HTTP response types just to remove a conversion. Avoid redundant structures and field copying, but do not remove meaningful validation or representation boundaries.
- Raw SQL is an exception, not an alternative default. Before introducing it, explain the concrete SQLite capability or query requirement that cannot reasonably be expressed with Diesel; use a small typed SQL expression within the query builder when that is sufficient. Existing uses for database time, SQLite system tables, or atomic statements do not justify handwritten SQL for unrelated ordinary queries.
- When raw SQL is necessary, bind runtime values rather than interpolating or concatenating them, and format longer statements across readable lines. Ordinary test fixture inserts and updates should also use Diesel; migration statements, database fault injection, and `EXPLAIN QUERY PLAN` checks are distinct uses, not templates for application queries.
- Use the shared `Database::read` / `Database::write` and `Transaction` boundary. Keep business database operations in their owning components; do not add connection management or transaction orchestration to HTTP handlers.

## Web visual design

Read [DESIGN.md](DESIGN.md) before changing Web layouts, components, or styles. It is the Web visual standard; `docs/architecture.md` remains the authority for API contracts, permissions, session boundaries, and business-state semantics.

## Native Windows printing

- Handwritten Rust `unsafe` is permitted only inside `crates/natsume-print-client/src/backend/` for the reviewed Windows printing boundary. Keep the workspace-wide denial elsewhere, including Server, networking, task state, and UI code.
- Each native call must document pointer validity, buffer bounds, ownership, and thread requirements as applicable. Keep printer handles, device contexts, and WinRT initialization on their owning thread; release them deterministically and abort unfinished print documents on failure.
- Obtain the Windows Job ID directly from the print submission API and verify its exact attempt label when observing it. Do not infer success from a disappearing job, match by filename, treat RAW PDF as universally supported, or claim physical output from spooler status.

## Error handling

- Rust error handling must use `snafu`. Define typed errors with `#[derive(Snafu)]` and use Snafu context selectors and helpers to construct, wrap, and propagate failures, including in executables, libraries, and tests.
- Do not introduce alternative error frameworks such as `anyhow` or `thiserror`, use strings or `Box<dyn Error>` as application error types, or handwrite `Display` and `std::error::Error` implementations in place of Snafu derives.
- Convert dependency errors at the relevant application boundary, preserve useful source errors and context, and report failures through tracing. User-visible messages and logs must not expose credentials or sensitive source details.

## Logging and output

- Rust application output must use `tracing`, including normal results, startup failures, runtime errors, and test diagnostics. Do not use `println!`, `eprintln!`, `print!`, `eprint!`, `dbg!`, or handwritten stdout/stderr writes to bypass tracing.
- Interactive terminal prompts and their answer context (the Server `bootstrap` / `reset-operator-password` credential prompts and the `natsume-server-setup` wizard, including its one-time encrypted key export) write directly to the controlling terminal rather than through tracing; secrets never enter tracing; progress, results and errors still use tracing. `inquire` renders prompts to stderr, so tools using it must require stdin, stdout and stderr to all be terminals.
- Initialize the tracing subscriber at the executable entry point before parsing CLI arguments or loading configuration. Libraries emit events through tracing and do not initialize global subscribers.
- Preserve required output channels when configuring the subscriber. In particular, DOMjudge reads the print wrapper's stdout; route its result and error events there. Do not log credentials, source contents, or PDF contents, and escape untrusted display values.
- Cargo build directives and generated files are protocol or artifact output, not logs. Keep their required formats; do not replace them with tracing events or use this distinction to bypass application logging.

## Privileged file operations and key material

- When running as root, never rename or unlink by name inside a directory that a less privileged user can write (for example the service-owned `/var/lib/natsume-server/keys`); the entry can be swapped between check and use. Move the object into a root-only `0700` quarantine directory on the same filesystem, verify device, inode and content through descriptors, then delete it there. Prepare and check that quarantine before the first write, not during rollback.
- Decide whether a directory needs that protection from its actual owner and mode (root-owned with no group/other write), not from its path. Disarm temporary-file cleanup guards once publication succeeds so a reused temporary name is never deleted.
- rcgen's `zeroize` feature only implements `Zeroize`; it does not wipe on drop. Wrap temporary `rcgen::KeyPair` values in an explicit zeroizing guard and borrow the guarded key instead of making additional serialized copies.

## Agent skills

### Pre-commit hooks

This repository uses `prek` to run the `.pre-commit-config.yaml` hooks. After changes that affect the hook scope, run `prek run --all-files`; this is the only pre-commit check entry point. Do not assume the separate `pre-commit` CLI is installed.

### Issue tracker

Issues and specs are tracked in GitHub Issues for `4o3F/Natsume`. See `docs/agents/issue-tracker.md`.

### Domain docs

Single-context layout with root `CONTEXT.md` and `docs/adr/`; `docs/architecture.md` remains the sole manually maintained architecture authority. See `docs/agents/domain.md`.
