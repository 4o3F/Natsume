# Natsume V2 支持平台与环境冻结

> 状态：`DRAFT-STEP0`
> 最后复核：2026-07-31
> Phase 0 窗口：2026-07-23 至 2026-08-12
> 结论：当前没有可将目标环境标记为 `ENV-FROZEN` 的完整证据

本文档记录平台、版本和环境的候选/冻结状态。架构规则来自规范文档，实际签收来自 CI、package 和目标环境 evidence。

## 1. 状态定义

| 状态 | 含义 |
|---|---|
| `ARCH-FROZEN` | 架构层已决定，环境选择不得违反 |
| `REPO-PINNED` | 仓库已固定工具/artifact，不等于目标环境已签收 |
| `ENV-UNFROZEN` | 关键输入尚未确定或尚无可复现证据 |
| `ENV-PROPOSED` | 已提出具体候选，但目标环境证据不完整 |
| `ENV-FROZEN` | 具体版本/硬件组合已验证签收，evidence 可定位 |
| `REJECTED` | 已验证不满足 requirement 或架构约束 |

升级为 `ENV-FROZEN` 必须有：精确版本或硬件标识、可复现步骤、正向与关键负向结果、artifact 定位、日期与已知限制。缺任一项时保持 `ENV-PROPOSED` 或 `ENV-UNFROZEN`。

## 2. 架构已冻结的平台边界

| 项目 | 决策 |
|---|---|
| Server init system | systemd-compatible Linux |
| Client init/session discovery | systemd + logind-compatible Linux |
| Operator UI | 现代浏览器中的 Web Panel |
| Client data plane | package-pinned Caddy，loopback HTTPS |
| Device control | QUIC + mandatory mTLS + Protobuf |
| Session Agent launch | system-level XDG Autostart，直接 resident process |
| Session Agent GUI | build-time Slint，winit backend + Skia renderer |
| Desktop matrix | 至少 GNOME/GDM/Wayland + 一个 LightDM 启动的目标 X11 desktop |
| Home backend | 部署时在 OverlayFS/staged-copy 二选一，运行时不 silent fallback |
| Package | Server/Client Deb，nFPM 映射 artifact |
| Runtime download | 禁止 postinstall/运行时下载 |

这些规则不能用环境不方便为由绕过；应选择满足规则的环境或提交 ADR。

## 3. 仓库工具与 artifact pin

下表描述当前仓库 pin，不代表目标 OS 已验证。

| 项目 | 当前 pin | 状态 | 验证 |
|---|---:|---|---|
| Rust | 1.97.1 | `REPO-PINNED` | `just toolchain` |
| Rust edition | 2024 | `REPO-PINNED` | Cargo workspace |
| Node.js | 24.1.0 | `REPO-PINNED` | `just toolchain` |
| pnpm | 11.1.0 | `REPO-PINNED` | `just toolchain` |
| Mermaid | 11.16.0 | `REPO-PINNED` | `pnpm diagrams` |
| nFPM | 2.47.0 | `REPO-PINNED` | package supply checks |
| Caddy | 2.11.4 | `ENV-PROPOSED` / `REPO-PINNED` | checksum + 目标环境验证 |
| protoc | vendored crate | `REPO-PINNED` | Cargo lock/contract CI |

Caddy 只有在 source、archive checksum、binary checksum、module closure、目标 OS 执行和 package lifecycle 全部签收后才可标为 `ENV-FROZEN`。

## 4. 目标环境冻结状态

全部 `ENV-UNFROZEN`。当前无任何目标环境或物理硬件 evidence。

| 项目 | 需要冻结的内容 |
|---|---|
| Server OS | 发行版/架构/kernel/glibc/systemd；clean install、reboot、包生命周期、firewall 对 TCP/UDP 同端口的行为 |
| Client OS | 同上，另加 Deb 安装、Caddy 执行、D-Bus/logind |
| Server 网络 | 地址族、Server IP literal、port（候选 `8443`）、Client 地址段 |
| Operator 浏览器 | family/version、OS、分辨率/缩放、中文输入、安全 policy、更新窗口 |
| DOMjudge | 版本、upstream scheme/host/port/path、登录契约、TLS/trust、健康检查 |
| Desktop | GNOME/GDM/Wayland 与 LightDM/选定 X11 desktop 两套环境 |
| Slint runtime closure | 精确版本、features、动态链接库、font/IME、package size、cold start |
| Home backend | OverlayFS 与 staged-copy 二选一（均为 `ENV-PROPOSED`） |
| 物理硬件 | 至少 6 台、2 个 OEM/主板系列、SATA + NVMe；当前 `0 / 6` |

网络必须验证：正确 IP-SAN 通过、错误 IP/错误 CA/过期证书失败、TCP/UDP firewall 与 NAT、DNS 不作为必需 fallback、preseed/upgrade 保留 endpoint、不使用 TOFU 或 dangerous verifier。

Desktop 两套环境各自必须验证的 capability：XDG Autostart 直接启动同一 binary、初始 resident + hidden、typed trigger 后 lazy Slint window、current logind session 识别、owner-only singleton、中文/IME、HiDPI、focus result 可观察、lock/unlock、terminate/replacement、display lost 与 crash recovery、无 systemd user unit、lock/unlock 不调用 Caddy。核心应用依赖 capability，不直接依赖桌面名称；无法保证 focus 时报告 `VISIBLE_UNFOCUSED`，不加入 desktop-specific 强制聚焦 hack。

物理硬件 fixture 只保存匿名化候选、quality、typed result 和 derived ID，不得保存原始 serial、private key、真实 password 或完整 Machine Hardware ID。必须覆盖 placeholder/缺失/permission denied/重复 source 与 configured-disk copy。

Natsume 不应在核心状态机中加入 DOMjudge 版本特例；适配差异留在 data-plane adapter 并通过 contract test 固定。

WSL、普通开发机、虚拟硬件 serial 或 reference scaffold 不得充当目标环境 evidence。

## 5. 支持声明

在 G0 通过前：

- 不发布"支持某发行版/桌面/浏览器"的产品声明；
- 只能称为候选目标；
- repo pin 不等于环境支持；
- VM 证据不替代物理 Machine ID；
- 单桌面通过不关闭双桌面 requirement；
- 文档 scaffold 不等于 GUI 实现；
- package build 成功不等于 lifecycle 签收。

## 6. 当前输入门禁

| ID | 输入 | 截止 | 状态 |
|---|---|---|---|
| `G0-IN-001` | Server/Client OS、architecture、systemd | 2026-07-29 | `BLOCKED-INPUT` |
| `G0-IN-002` | Server IP literal 与 TCP/UDP port | 2026-07-29 | `BLOCKED-INPUT` |
| `G0-IN-003` | Caddy version/modules/source/checksum | 2026-07-29 | `BLOCKED-INPUT` |
| `G0-IN-004` | Browser、DOMjudge、双桌面、XDG、Slint、lock API | 2026-08-01 | `BLOCKED-INPUT` |
| `G0-IN-005` | 六台物理硬件 | 2026-08-01 | `BLOCKED-INPUT` |
| `G0-IN-006` | PKI test material 与 owner | 2026-08-01 | `BLOCKED-INPUT` |
| `G0-IN-007` | Step 0 文档/ID/术语签收 | Step 0 | `OPEN` |

Gate 状态以 [`gates/phase-0-status.md`](gates/phase-0-status.md) 为准。
