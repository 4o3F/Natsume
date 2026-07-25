# Natsume V2 支持平台与环境冻结

> 状态：`DRAFT-STEP0`  
> 最后复核：2026-07-24  
> Phase 0 窗口：2026-07-23 至 2026-08-12  
> 结论：当前没有可将目标环境标记为 `ENV-FROZEN` 的完整证据；G0 仍为 `OPEN`

本文档只记录平台、版本和环境的候选/冻结状态。架构规则来自规范文档，实际签收来自 probe、CI、package 和实验室 evidence。

## 1. 状态定义

| 状态 | 含义 |
|---|---|
| `ARCH-FROZEN` | 架构层已经决定，环境选择不得违反 |
| `ENV-PROPOSED` | 已提出具体候选，但目标环境证据不完整 |
| `ENV-UNFROZEN` | 关键输入尚未确定或尚无可复现证据 |
| `ENV-FROZEN` | Owner 与 reviewer 已签收具体版本/组合、evidence 可定位 |
| `REJECTED` | 已验证不满足 requirement 或架构约束 |
| `REPO-PINNED` | 仓库已固定工具/artifact，不等于目标环境已签收 |

升级为 `ENV-FROZEN` 必须同时满足：

1. 精确版本/镜像/硬件组合；
2. 可复现实验步骤；
3. 正向和关键负向结果；
4. artifact/日志定位；
5. owner 与 reviewer；
6. 日期和限制；
7. 关联 requirement/Gate。

## 2. 角色字典

| 角色 | 责任 |
|---|---|
| `ROLE_ARCHITECTURE` | 架构一致性与 ADR |
| `ROLE_BUILD` | 工具链、CI、lockfile |
| `ROLE_SERVER_PLATFORM` | Server OS/systemd/网络 |
| `ROLE_CLIENT_PLATFORM` | Client OS/systemd/package |
| `ROLE_DESKTOP` | DM/DE/Wayland/X11/XDG/Slint/lock |
| `ROLE_LAB_NETWORK` | IP、端口、地址段、路由 |
| `ROLE_LAB_HARDWARE` | 物理机与 Machine ID fixture |
| `ROLE_DOMJUDGE` | DOMjudge 版本和访问契约 |
| `ROLE_CADDY_SUPPLY` | Caddy artifact/module/checksum |
| `ROLE_PKI` | PKI material 和 certificate probe |
| `ROLE_PROTOCOL` | QUIC/Protobuf |
| `ROLE_PACKAGING` | Deb/preseed/upgrade |
| `ROLE_RELEASE` | release、rollback、rehearsal |
| `ROLE_SECURITY` | secret、policy、threat review |
| `ROLE_GATE_G0` | G0 decision |

角色不是具体人员。签收记录必须补充姓名或组织身份。

## 3. 架构已冻结的平台边界

| 项目 | 决策 | 状态 |
|---|---|---|
| Server init system | systemd-compatible Linux | `ARCH-FROZEN` |
| Client init/session discovery | systemd + logind-compatible Linux | `ARCH-FROZEN` |
| Operator UI | 现代浏览器中的 Web Panel | `ARCH-FROZEN` |
| Client data plane | package-pinned Caddy，loopback HTTPS | `ARCH-FROZEN` |
| Device control | QUIC + mandatory mTLS + Protobuf | `ARCH-FROZEN` |
| Session Agent launch | system-level XDG Autostart，直接 resident process | `ARCH-FROZEN` |
| Session Agent GUI | build-time Slint，winit backend + Skia renderer | `ARCH-FROZEN` |
| Desktop matrix | 至少 GNOME/GDM/Wayland + 一个 LightDM 启动的目标 X11 desktop | `ARCH-FROZEN` |
| Home backend | 部署时在 OverlayFS/staged-copy 二选一，运行时不 silent fallback | `ARCH-FROZEN` |
| Package | Server/Client Deb，nFPM 映射 artifact | `ARCH-FROZEN` |
| Runtime download | 禁止 postinstall/运行时下载 | `ARCH-FROZEN` |

这些规则不能用环境不方便为由绕过；应选择满足规则的环境或提交 ADR。

## 4. 仓库工具与 artifact pin

下表描述当前仓库 pin，不代表目标 OS 已验证。

| 项目 | 当前 pin | 状态 | 验证 |
|---|---:|---|---|
| Rust | 1.97.1 | `REPO-PINNED` | `just toolchain` |
| Rust edition | 2024 | `REPO-PINNED` | Cargo workspace |
| Node.js | 24.1.0 | `REPO-PINNED` | `just toolchain` |
| pnpm | 11.1.0 | `REPO-PINNED` | `just toolchain` |
| Mermaid | 11.16.0 | `REPO-PINNED` | `pnpm diagrams` |
| nFPM | 2.47.0 | `REPO-PINNED` | package supply checks |
| Caddy | 2.11.4 | `ENV-PROPOSED` / `REPO-PINNED` | checksum + Probe C/F |
| protoc | vendored crate | `REPO-PINNED` | Cargo lock/contract CI |

Caddy 只有在 source、archive checksum、binary checksum、module closure、目标 OS 执行和 package lifecycle 全部签收后才可标为 `ENV-FROZEN`。

## 5. 目标 OS

| ID | 角色 | 精确发行版/架构 | systemd | 状态 | Owner | 阻塞 |
|---|---|---|---|---|---|---|
| `PLAT-SERVER-OS` | Server | 未冻结 | 未冻结 | `ENV-UNFROZEN` | `ROLE_SERVER_PLATFORM` | G0-003/012 |
| `PLAT-CLIENT-OS` | Client | 未冻结 | 未冻结 | `ENV-UNFROZEN` | `ROLE_CLIENT_PLATFORM` | G0-010/012 |

冻结 evidence 必须包含：

- image/repository identifier；
- kernel、glibc、systemd；
- CPU architecture；
- clean install；
- reboot；
- upgrade/reinstall/remove/purge；
- filesystem/permission；
- Caddy execution；
- D-Bus/logind；
- time synchronization；
- firewall 对 TCP/UDP 同端口的行为；
- rollback。

WSL、普通开发机或非目标 VM 不能替代目标 OS 签收。

## 6. Server 网络

| 项目 | 当前值 | 状态 | Owner | 截止 |
|---|---|---|---|---|
| 地址族 | IPv4 必选；IPv6 主支持待定 | `ENV-UNFROZEN` | `ROLE_LAB_NETWORK` | 2026-07-29 |
| Server IP literal | 未分配 | `ENV-UNFROZEN` | `ROLE_LAB_NETWORK` | 2026-07-29 |
| Server port | 候选 `8443` | `ENV-PROPOSED` | `ROLE_SERVER_PLATFORM` | 2026-07-29 |
| TCP HTTPS + UDP QUIC 同数字端口 | 架构允许，待目标网络验证 | `ENV-PROPOSED` | `ROLE_LAB_NETWORK` | 2026-07-29 |
| Client 地址段 | 未分配 | `ENV-UNFROZEN` | `ROLE_LAB_NETWORK` | 2026-07-29 |

必须验证：

- 正确 IP-SAN；
- 错误 IP、错误 CA、过期证书失败；
- TCP/UDP firewall/NAT；
- DNS 不作为必需 fallback；
- Client preseed/upgrade 保留 endpoint；
- 不使用 TOFU 或 dangerous verifier。

## 7. Operator 浏览器

| 项目 | 当前状态 |
|---|---|
| Browser family/version | `ENV-UNFROZEN` |
| OS | `ENV-UNFROZEN` |
| 分辨率/缩放 | `ENV-UNFROZEN` |
| 中文输入 | `ENV-UNFROZEN` |
| 安全 policy | `ENV-UNFROZEN` |
| 自动更新策略 | `ENV-UNFROZEN` |

冻结需要：

- Preparation Center 核心 journey；
- RBAC/session；
- CSV upload/preview/commit；
- bulk operation；
- SSE/reconnect；
- error/accessibility；
- secret 不进入 storage/analytics；
- kiosk/现场策略（如采用）；
- 版本更新窗口。

## 8. DOMjudge

| 项目 | 当前状态 | Owner |
|---|---|---|
| 版本 | `ENV-UNFROZEN` | `ROLE_DOMJUDGE` |
| upstream scheme/host/port/path | `ENV-UNFROZEN` | `ROLE_DOMJUDGE` |
| 登录/会话契约 | `ENV-UNFROZEN` | `ROLE_DOMJUDGE` |
| TLS/trust | `ENV-UNFROZEN` | `ROLE_DOMJUDGE` |
| 健康检查 | `ENV-UNFROZEN` | `ROLE_DOMJUDGE` |
| 竞赛账号约束 | `ENV-UNFROZEN` | `ROLE_DOMJUDGE` |

Natsume 不应在核心状态机中加入 DOMjudge 版本特例。适配差异留在 data-plane adapter，并通过 contract test 固定。

## 9. Desktop 和 Session Agent

目标矩阵：

| ID | DM/DE/协议 | 当前状态 | Owner |
|---|---|---|---|
| `PLAT-DESKTOP-WAYLAND` | GNOME + GDM + Wayland | `ENV-UNFROZEN` | `ROLE_DESKTOP` |
| `PLAT-DESKTOP-X11` | LightDM + 选定 X11 desktop | `ENV-UNFROZEN` | `ROLE_DESKTOP` |

每个环境必须验证 capability：

| Capability | Wayland | X11 | 证据 |
|---|---|---|---|
| XDG Autostart 直接启动同一 binary | 未验证 | 未验证 | Probe E/F |
| 初始 resident + hidden | 未验证 | 未验证 | Probe E |
| typed trigger 后 lazy Slint window | 未验证 | 未验证 | Probe E |
| current logind session 识别 | 未验证 | 未验证 | Probe E |
| owner-only singleton | 未验证 | 未验证 | Probe E |
| 中文/IME | 未验证 | 未验证 | Probe E |
| HiDPI | 未验证 | 未验证 | Probe E |
| focus result 可观察 | 未验证 | 未验证 | Probe E |
| lock | 未验证 | 未验证 | Probe E |
| unlock | 未验证 | 未验证 | Probe E |
| terminate/replacement | 未验证 | 未验证 | Probe E |
| display lost/crash recovery | 未验证 | 未验证 | Probe E |
| 无 systemd user unit | 未验证 | 未验证 | package scan |
| lock/unlock 不调用 Caddy | 未验证 | 未验证 | call counter/hash |

核心应用依赖 capability，不直接依赖桌面名称。无法保证 focus 时报告 `VISIBLE_UNFOCUSED`，不加入 desktop-specific 强制聚焦 hack。

## 10. Slint runtime closure

冻结前必须记录：

- Slint 精确版本；
- enabled features；
- winit backend；
- Skia renderer；
- 动态链接库；
- font/input/IME；
- Wayland/X11 库；
- 无 Qt/GTK/WebKit/runtime interpreter；
- package size；
- cold start；
- display lost 和 crash 行为。

`docs/reference/session-agent-slint/` 只是 Phase 6 scaffold，不是目标环境 evidence。

## 11. Home backend

| Backend | 状态 | 必须证明 |
|---|---|---|
| OverlayFS | `ENV-PROPOSED` | kernel/filesystem、mount namespace、cleanup、crash/reboot、ownership |
| staged-copy | `ENV-PROPOSED` | copy time/space、atomic staging、cleanup、crash/reboot、ownership |

部署冻结时选择一个。若 OverlayFS 失败，应通过 ADR/部署决策选择 staged-copy；运行时不得自动 fallback。

## 12. 物理硬件

要求：

- 至少 6 台真实工作站；
- 至少 2 个 OEM 或主板系列；
- SATA 和 NVMe；
- placeholder/缺失/permission denied/重复 source；
- configured-disk copy；
- 原始 serial 不入库。

当前：`0 / 6`，`ENV-UNFROZEN`。详细槽位见 [`lab/phase-0-inventory.md`](lab/phase-0-inventory.md)。

## 13. 支持声明

在 G0 通过前：

- 不发布“支持某发行版/桌面/浏览器”的产品声明；
- 只能称为候选目标；
- repo pin 不等于环境支持；
- VM 证据不替代物理 Machine ID；
- 单桌面通过不关闭双桌面 requirement；
- 文档 scaffold 不等于 GUI 实现；
- package build 成功不等于 lifecycle 签收。

## 14. 状态更新模板

每次冻结写入：

```text
ITEM_ID:
PREVIOUS_STATUS:
NEW_STATUS:
EXACT_VERSION_OR_HW:
COMMIT_SHA:
ENVIRONMENT:
TEST_OR_PROBE:
ARTIFACT_PATH:
RESULT:
OWNER:
REVIEWER:
DATE:
LIMITATIONS:
RELATED_REQS:
RELATED_GATES:
```

缺任一关键字段时保持 `ENV-PROPOSED` 或 `ENV-UNFROZEN`。

## 15. 当前输入门禁

| ID | 输入 | 截止 | 状态 |
|---|---|---|---|
| `G0-IN-001` | Server/Client OS、architecture、systemd | 2026-07-29 | `BLOCKED-INPUT` |
| `G0-IN-002` | Server IP literal 与 TCP/UDP port | 2026-07-29 | `BLOCKED-INPUT` |
| `G0-IN-003` | Caddy version/modules/source/checksum | 2026-07-29 | `BLOCKED-INPUT` |
| `G0-IN-004` | Browser、DOMjudge、双桌面、XDG、Slint、lock API | 2026-08-01 | `BLOCKED-INPUT` |
| `G0-IN-005` | 六台物理硬件 | 2026-08-01 | `BLOCKED-INPUT` |
| `G0-IN-006` | PKI test material 与 owner | 2026-08-01 | `BLOCKED-INPUT` |
| `G0-IN-007` | Step 0 文档/ID/术语签收 | Step 0 | `OPEN` |

Gate 的机器状态以 [`verification/registry.json`](verification/registry.json) 为准。
