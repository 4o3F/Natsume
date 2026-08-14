# Phase 0 状态

> 状态：`DRAFT-STEP0`
> 最后更新：2026-08-14
> G0：`OPEN`（4/12 PASS）

Phase 0 工程基线尚未完成。本文件手写追踪 G0 进度。条目通过需可定位 evidence（CI run / commit / artifact 链接 + 一行结论），不得以文档存在、scaffold 或截图替代可复现结果。

## G0 条目（12 项）

| # | 条目 | 状态 |
|---|---|---|
| 1 | 工具链与单一 lockfile 可重现（clean checkout） | `PASS` |
| 2 | 真实 CI：Rust/Web/契约 clean diff/policy scan/package smoke | `PASS` |
| 3 | SNAFU + stable ErrorCode 边界与 redaction tests | `OPEN` |
| 4 | 契约骨架重定向 v2.8：current-state SQL（无 freeze 或未消费 workflow history）、窗口门禁 Enrollment、Panel UUIDv7 `PUT /api/v2/commands/{command_id}` 的 `201/200/400/409` 声明、`request_fingerprint_*`/`frozen_payload_json`、WSS envelope、Observed/CommandStatus、D-Bus、golden clean diff；不以此声明 handler/dispatcher/journal/UI 完成 | `OPEN` |
| 5 | QUIC/framing/mTLS 骨架与测试残留清除（`crates/device-protocol` framing、CI 断言同步） | `PASS` |
| 6 | Server/Client 空 Deb 构建+安装+权限/preseed 验证 | `PASS` |
| 7 | 目标环境：IP-SAN/endpoint 与单 TCP 端口验证 | `BLOCKED-INPUT` |
| 8 | 目标环境：`INV-CERT-01` 两段阶梯 schema/路由负向断言 | `OPEN` |
| 9 | DOMjudge lab：xheaders 登录、brotli 透传、upstream TLS 三项结论 | `BLOCKED-INPUT` |
| 10 | identity fixture 集（v1 事故 + 代表性异构 + configured-disk copy）决策表测试 | `BLOCKED-INPUT` |
| 11 | 当期镜像桌面 capability 清单首次执行 | `BLOCKED-INPUT` |
| 12 | package/systemd 生命周期 smoke（install/upgrade/remove/purge/reboot） | `BLOCKED-INPUT` |

## 已登记证据

- 条目 1：[ci run 31794482161](https://github.com/4o3F/Natsume/actions/runs/31794482161)（commit `869804e`，2026-08-14）——clean checkout 上 pinned 工具链断言（`just toolchain`）与 Cargo/pnpm frozen lockfile 全套 lane 通过。
- 条目 2：[ci run 31794482161](https://github.com/4o3F/Natsume/actions/runs/31794482161) 与 [package-lifecycle run 31792908910](https://github.com/4o3F/Natsume/actions/runs/31792908910)（2026-08-14）——Rust/Web/契约 clean diff（含 diesel schema）/policy scan/package smoke 五条 lane 真实运行全绿；weekly 生命周期 lane 首次真实执行通过。已知限制：该 lane 为 same-version reinstall、无 reboot、非目标 OS（`packaging/README.md`），完整生命周期归条目 12。
- 条目 5：[ci run 31794482161](https://github.com/4o3F/Natsume/actions/runs/31794482161)（2026-08-14）——policy scan 的 QUIC/framing/mTLS/CSR 负向断言与 `protocol_contract` 冻结测试全绿，仓内残留 grep 为零。
- 条目 6：[package-lifecycle run 31792908910](https://github.com/4o3F/Natsume/actions/runs/31792908910)（2026-08-14）——双包真实 install/reinstall（client 另有 reconfigure）/remove/purge 与 sysusers 账户、tmpfiles mode/owner、endpoint conffile 断言全部通过（shared runner，非目标 OS；目标 OS 验证归条目 7/12）。

## 输入门禁

| ID | 输入 | 状态 |
|---|---|---|
| G0-IN-001 | Server/Client OS、architecture、systemd | `ENV-PROPOSED`（Ubuntu 24.04 已提供，缺 point release/kernel/glibc/systemd 精确值） |
| G0-IN-002 | Server endpoint 与单 TCP 端口 | `RESOLVED`：地址按部署配置，不需要仓库 IP literal；端口固定 `8443` |
| G0-IN-003 | Caddy version/modules/source/checksum | `RESOLVED`：2.11.4 标准发行版已固定并由 `just ci-packages` 校验 |
| G0-IN-004 | Browser、DOMjudge（xheaders/brotli/TLS）、当期桌面、XDG、Slint、lock API | 大部分推进：桌面 GNOME + X11；xheaders 协议契约已确认，认证语义核实为 password-verifying；Browser TLS 1.3 非阻塞。剩余 DOMjudge 部署事实（含部署版本 xheaders 语义复核）、Slint closure、lock API |
| G0-IN-005 | 硬件 fixture 集（v1 事故 + 代表性异构） | `BLOCKED-INPUT`：所需字段与场景清单见 [支持平台](../supported-platform.md) §4.1 |
| G0-IN-006 | PKI test material（control CA / origin CA）与 owner | `RESOLVED`：两根均自签；test material 由 `rcgen` 运行时生成 |
| G0-IN-007 | v2.8 current-state、BindingRevision、provisioning recovery、Panel Command ID 与 frozen-payload 文档/术语签收 | `OPEN` |

## 目标环境验证

尚未开始。条目 7–12 被 `G0-IN-001` 至 `G0-IN-006` 阻塞；条目 1–6、8 可在仓库内先行。

每次验证记录：主题、`COMMIT_SHA`、精确环境或硬件标识、步骤、正向与负向结果、artifact 路径、日期、已知限制。部分通过记为未通过。

## 关闭条件

12 项条目全 `PASS`，且每项有可定位 evidence。证据标准见 [路线图 §6](../roadmap.md)。
