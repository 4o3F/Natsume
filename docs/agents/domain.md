# Domain Docs

This repo uses a single-context layout across Server, Client, shared
crates, and Web.

## Before exploring, read these

1. `docs/README.md` for the maintained documentation index.
2. Relevant sections of `docs/architecture.md`, including its authority
   and reading rules.
3. Root `CONTEXT.md`, if present, for shared domain vocabulary.
4. Relevant records under `docs/adr/`, if present.

If `CONTEXT.md` or `docs/adr/` is absent, proceed silently. Do not flag
their absence or suggest creating them upfront. `/domain-modeling`
creates domain documentation lazily as terms and decisions are resolved.

## Layout

- `CONTEXT.md`: one repository-wide glossary and context entry point.
- `docs/adr/`: repository-wide decision records, created as needed.
- `docs/architecture.md`: the sole manually maintained architecture authority.

Paths are relative to the repository root.

## Preserve the existing authority boundary

`CONTEXT.md` and ADRs reference `docs/architecture.md`; they must not
establish a parallel architecture authority. Accepted architecture
changes must be reflected in `docs/architecture.md`. ADRs may record
their rationale and history.

The architecture document describes the target state. Verify completion
against current implementation and relevant acceptance evidence.

## Use the glossary's vocabulary

Use terms defined in `CONTEXT.md` when it exists. Otherwise, use the
vocabulary in `docs/architecture.md`. Avoid introducing synonyms for
existing concepts. Note genuine vocabulary gaps for `/domain-modeling`.

## Surface conflicts

Explicitly identify proposals that conflict with an existing ADR or
`docs/architecture.md`, and explain why the decision should be revisited.
Historical records do not override the current architecture authority.
