# Phase 0 状态

> 状态：`DRAFT-STEP0`  
> 最后更新：2026-07-30  
> G0：`OPEN`（0/15 PASS）

Phase 0 工程基线尚未完成。本文件手写追踪 G0 进度。Gate 通过需可定位 evidence，不得以文档存在、scaffold 或截图替代可复现结果。

## G0 Gate（15 项，当前全 OPEN）

主要类别（详见 [Phase 0 计划](../implementation/phase-0-engineering-baseline.md)）：

- 工具链与单一 lockfile 可重现；
- 真实 CI（Rust/Web/契约/策略/package smoke）；
- SNAFU + stable ErrorCode 边界；
- 契约骨架（OpenAPI/Protobuf/D-Bus/SQL）；
- Server/Client 空 Deb；
- Probe A–F evidence 可定位；
- 目标 OS、网络、桌面、硬件、PKI、DOMjudge 输入冻结。

## 输入门禁

| ID | 输入 | 状态 |
|---|---|---|
| G0-IN-001 | Server/Client OS、architecture、systemd | `BLOCKED-INPUT` |
| G0-IN-002 | Server IP literal 与 TCP/UDP port | `BLOCKED-INPUT` |
| G0-IN-003 | Caddy version/modules/source/checksum | `BLOCKED-INPUT` |
| G0-IN-004 | Browser、DOMjudge、双桌面、XDG、Slint、lock API | `BLOCKED-INPUT` |
| G0-IN-005 | 六台物理硬件 | `BLOCKED-INPUT` |
| G0-IN-006 | PKI test material 与 owner | `BLOCKED-INPUT` |
| G0-IN-007 | Step 0 文档/ID/术语签收 | `OPEN` |

## Probe 状态

全部 `NOT-RUN`（A–F）。报告见 [probes/](../probes/)。

G0 关闭条件：15 项 gate 全 `PASS` 且 [G0 decision](g0-decision-template.md) 签署。
