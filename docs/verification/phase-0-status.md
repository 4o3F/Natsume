<!-- GENERATED FILE: DO NOT EDIT DIRECTLY -->
<!-- Source: docs/verification/registry.json; regenerate with node docs/verification/render.mjs --write -->

# Phase 0 当前状态

> 数据日期：2026-07-24  
> Phase 窗口：2026-07-23 至 2026-08-12  
> G0：`OPEN`，`0 / 15` PASS

## 结论

当前仓库仍是 Phase 0 工程基线。文档重构只消除重复和矛盾，不把任何实现、平台或 Gate 条目标记为完成。

| 指标 | 当前值 |
|---|---:|
| Requirements | 49 |
| OPEN requirements | 49 |
| SATISFIED requirements | 0 |
| G0 PASS | 0 / 15 |
| BLOCKED-INPUT inputs | 6 / 7 |
| Probe PASS | 0 / 6 |

## 主要阻塞

- `G0-IN-001`：Server/Client 目标 OS、architecture、systemd 已冻结（`BLOCKED-INPUT`，截止 2026-07-29）
- `G0-IN-002`：实验室 Server IP literal 与 TCP/UDP port 已冻结（`BLOCKED-INPUT`，截止 2026-07-29）
- `G0-IN-003`：Caddy version/modules/source/SHA-256 已冻结（`BLOCKED-INPUT`，截止 2026-07-29）
- `G0-IN-004`：Browser、DOMjudge、双桌面、XDG、Slint closure 与 lock API 已冻结（`BLOCKED-INPUT`，截止 2026-08-01）
- `G0-IN-005`：六台物理硬件已到位并登记（`BLOCKED-INPUT`，截止 2026-08-01）
- `G0-IN-006`：PKI test material 与 ownership 已登记（`BLOCKED-INPUT`，截止 2026-08-01）
- `G0-IN-007`：Step 0 文档、ID 和术语已正式对齐签收（`OPEN`，截止 Step 0）

## 下一步

1. 冻结目标 OS、Server endpoint、Caddy supply 和 PKI test material；
2. 到位并登记六台物理工作站；
3. 执行 Probe A–F 并提交可复现 evidence；
4. 根据 evidence 更新 registry；
5. 运行 renderer 和链接/契约检查；
6. 15 项 Gate 全部 PASS 后签署独立 G0 decision。

## 详情

- [Requirements](../requirements/phase-0.md)
- [G0 checklist](../gates/g0-checklist.md)
- [Supported platform](../supported-platform.md)
- [Lab inventory](../lab/phase-0-inventory.md)
- [Probe reports](../probes/README.md)
