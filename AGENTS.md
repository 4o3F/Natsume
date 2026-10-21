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

## Error handling

- Rust error handling must use `snafu`. Define typed errors with `#[derive(Snafu)]` and use Snafu context selectors and helpers to construct, wrap, and propagate failures, including in executables, libraries, and tests.
- Do not introduce alternative error frameworks such as `anyhow` or `thiserror`, use strings or `Box<dyn Error>` as application error types, or handwrite `Display` and `std::error::Error` implementations in place of Snafu derives.
- Convert dependency errors at the relevant application boundary, preserve useful source errors and context, and report failures through tracing. User-visible messages and logs must not expose credentials or sensitive source details.

## Logging and output

- Rust application output must use `tracing`, including normal results, startup failures, runtime errors, and test diagnostics. Do not use `println!`, `eprintln!`, `print!`, `eprint!`, `dbg!`, or handwritten stdout/stderr writes to bypass tracing.
- Initialize the tracing subscriber at the executable entry point before parsing CLI arguments or loading configuration. Libraries emit events through tracing and do not initialize global subscribers.
- Preserve required output channels when configuring the subscriber. In particular, DOMjudge reads the print wrapper's stdout; route its result and error events there. Do not log credentials, source contents, or PDF contents, and escape untrusted display values.
- Cargo build directives and generated files are protocol or artifact output, not logs. Keep their required formats; do not replace them with tracing events or use this distinction to bypass application logging.

## Agent skills

### Pre-commit hooks

This repository uses `prek` to run the `.pre-commit-config.yaml` hooks. After changes that affect the hook scope, run `prek run --all-files`; this is the only pre-commit check entry point. Do not assume the separate `pre-commit` CLI is installed.

### Issue tracker

Issues and specs are tracked in GitHub Issues for `4o3F/Natsume`. See `docs/agents/issue-tracker.md`.

### Domain docs

Single-context layout with root `CONTEXT.md` and `docs/adr/`; `docs/architecture.md` remains the sole manually maintained architecture authority. See `docs/agents/domain.md`.
