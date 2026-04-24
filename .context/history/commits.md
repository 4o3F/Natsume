# Commit Decision History

> 此文件是 `commits.jsonl` 的人类可读视图，可由工具重生成。
> Canonical store: `commits.jsonl` (JSONL, append-only)

| Date | Context-Id | Commit | Summary | Decisions | Bugs | Risk |
|------|-----------|--------|---------|-----------|------|------|
| 2026-04-16 | dae1618a | pending | feat: add GUI bind mode with zenity prompt | Re-exec detach (not fork); Cross-user GUI via runuser+loginctl | - | Medium: depends on zenity/loginctl/runuser availability |
| 2026-04-24 | cb54caeb | pending | chore(assets): tighten firefox contest browser policy | Apply Firefox enterprise policies in bootstrap | - | Low: browser defaults only |
| 2026-04-24 | f658e748 | pending | chore(assets): tighten firefox contest browser policy | Apply Firefox enterprise policies in bootstrap | - | Low: browser defaults only |
| 2026-04-24 | 17b6238c | pending | fix(client): remove lightdm seat header on cleanup | Remove stale LightDM seat header during cleanup | Duplicate [Seat:*] entries in lightdm.conf | Low: managed cleanup only |
