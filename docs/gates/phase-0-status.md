# Phase 0 状态

> 状态：`DRAFT-STEP0`
> 最后更新：2026-07-31
> G0：`OPEN`（0/15 PASS）

Phase 0 工程基线尚未完成。本文件手写追踪 G0 进度。Gate 通过需可定位 evidence，不得以文档存在、scaffold 或截图替代可复现结果。

## G0 Gate（15 项，当前全 OPEN）

主要类别（工作包见 [路线图 §4](../roadmap.md)）：

- 工具链与单一 lockfile 可重现；
- 真实 CI（Rust/Web/契约/策略/package smoke）；
- SNAFU + stable ErrorCode 边界；
- 契约骨架（OpenAPI/Protobuf/D-Bus/SQL）；
- Server/Client 空 Deb；
- 目标环境高风险验证（IP-SAN/endpoint、证书阶梯、Caddy/DOMjudge、Machine identity、Session/Home、package/systemd）evidence 可定位；
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

## 目标环境验证

尚未开始。当前无目标 OS、双桌面或物理硬件，六类高风险验证均无法执行——被 `G0-IN-001` 至 `G0-IN-006` 阻塞。

每次验证记录：主题、`COMMIT_SHA`、精确环境或硬件标识、步骤、正向与负向结果、artifact 路径、日期、已知限制。部分通过记为未通过。

## 关闭条件

15 项 gate 全 `PASS`，且每项有可定位 evidence。证据标准见 [路线图 §6](../roadmap.md)。
