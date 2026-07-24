# Natsume V2 integrated development snapshot

> Upstream base: `4o3F/Natsume` branch `v2` at `dcbefb68035ab2fb1df74f5ddafa0ce7a181820c`  
> Architecture baseline: **v2.7**  
> Roadmap baseline: **v1.4**  
> Integration date: **2026-07-23**

This tree is intended to replace a local Natsume V2 working copy **after backing up local-only changes**. It preserves the latest upstream Phase 0 engineering baseline and stable-error implementation, then merges the accepted v2.7 architecture, roadmap, phase plans, protocol/schema contracts and XDG Autostart packaging boundary.

## Start here

- Architecture: [`docs/v2-design.md`](docs/v2-design.md)
- Concise roadmap: [`docs/implementation-roadmap.md`](docs/implementation-roadmap.md)
- Detailed Phase 0–7 plans: [`docs/implementation/`](docs/implementation/)
- Upstream provenance: [`docs/upstream-base.md`](docs/upstream-base.md)
- Merge decisions and overwrite procedure: [`docs/merge-report.md`](docs/merge-report.md)
- Validation report: [`docs/validation-report.md`](docs/validation-report.md)
- Source integrity: `FILE_MANIFEST.sha256` and `FILE_MANIFEST.json`
- Phase 0 requirements and G0: [`docs/requirements/phase-0.md`](docs/requirements/phase-0.md), [`docs/gates/g0-checklist.md`](docs/gates/g0-checklist.md)
- Target platform and lab inventory: [`docs/supported-platform.md`](docs/supported-platform.md), [`docs/lab/phase-0-inventory.md`](docs/lab/phase-0-inventory.md)

## Current implementation boundary

The upstream branch is still a Phase 0 engineering baseline, not the completed Natsume product. The stable `natsume-error-code` crate, lockfiles, CI, package topology and contract tests are retained as real code. The v2.7 Session Agent launch boundary is applied now: package-owned XDG Autostart, hidden resident process, and no systemd user unit.

The mature Slint GUI remains a **Phase 6 implementation task**. Its reviewed scaffold is stored under `docs/reference/session-agent-slint/` instead of being inserted into the current Cargo graph prematurely. This keeps the upstream `Cargo.lock` valid and avoids claiming Phase 6 completion in a Phase 0 branch.

## Local replacement

1. Back up uncommitted or local-only files.
2. Remove the old working-tree contents except `.git` when preserving local history.
3. Copy this archive's `Natsume/` contents into the repository root.
4. Review `docs/merge-report.md` and run the verification commands available in your environment.
5. Commit the integration as one local merge commit before continuing feature work.

Do not copy `.git` from another checkout; this distribution intentionally contains source files only.
