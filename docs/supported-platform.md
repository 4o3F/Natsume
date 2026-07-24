# Phase 0 支持平台冻结表

> 状态：`DRAFT-STEP0`  
> 架构边界：部分 `ARCH-FROZEN`  
> 目标环境与供应链：尚无 `ENV-FROZEN` 签收项  
> Phase 0 窗口：2026-07-23 至 2026-08-12  
> 权威来源：`v2-design.md`、`implementation-roadmap.md`、`implementation/phase-0-engineering-baseline.md`  
> 规则：未知环境事实必须标记为 `ENV-UNFROZEN`，并记录 Owner、绝对截止日期和阻塞影响。

## 30 秒结论

| 项目 | 当前结论 |
|---|---|
| G0 是否可签署 | 否；15 项均为 `OPEN` |
| 架构冻结是否等于环境验证 | 否；`ARCH-FROZEN` 不关闭任何目标环境输入门禁 |
| Device 证书 | Enrollment 只签 Device Identity certificate |
| Gateway 证书 | 仅 authenticated mTLS QUIC + active `SYNC_STATE` |
| 目标 OS / Caddy / 实验室 | 尚未签收 |
| 物理硬件 | 0/6 已验证 |

## 状态定义

| 状态 | 含义 |
|---|---|
| `ARCH-FROZEN` | 权威架构已明确，变更必须更新设计、ADR 和测试；不表示目标环境已验证 |
| `ENV-PROPOSED` | 仓库已有默认值或候选值，但尚未在目标环境或供应链签收 |
| `ENV-UNFROZEN` | 尚未确定或验证，不能用于关闭 Probe/G0 输入门禁 |
| `ENV-FROZEN` | 已有可定位的目标环境或供应链签收证据 |
| `REJECTED` | 经 ADR 明确不支持 |

不得以 `ARCH-FROZEN`、`ENV-PROPOSED` 或 `ENV-UNFROZEN` 值宣称对应探针、证据或 Gate 已通过。

## 角色字典

以下角色 ID 是五份 Step 0 文档的唯一 Owner 词汇：

```text
ROLE_ARCHITECTURE  ROLE_BUILD           ROLE_SERVER
ROLE_SERVER_PLATFORM                    ROLE_CLIENT
ROLE_CLIENT_PLATFORM                    ROLE_DESKTOP
ROLE_BROWSER       ROLE_DOMJUDGE        ROLE_CADDY_SUPPLY
ROLE_LAB_HARDWARE  ROLE_LAB_NETWORK     ROLE_LAB_EVIDENCE
ROLE_PKI           ROLE_PACKAGING       ROLE_RELEASE
ROLE_PROTOCOL      ROLE_SECURITY        ROLE_HOME
ROLE_WEB           ROLE_DOCS            ROLE_GATE_G0
```

角色 ID 不代表 `.github/CODEOWNERS` 中的 GitHub 团队已经存在；实际人员映射必须在 Gate 签署前补全。

## 1. Server

| 字段 | 状态 | 当前值或待决项 | Owner | 截止日期 | 阻塞 |
|---|---|---|---|---|---|
| 服务形态 | `ARCH-FROZEN` | 单进程 `natsume-server`；HTTPS/Enrollment 与 QUIC 使用独立 listener 和 TLS 配置 | `ROLE_ARCHITECTURE` | — | — |
| 主数据库 | `ARCH-FROZEN` | SQLite + WAL、短事务、应用层 AEAD、独立 root key 文件 | `ROLE_SERVER` | — | — |
| 端口模型 | `ARCH-FROZEN` | TCP HTTPS 与 UDP QUIC 使用同一数字端口 | `ROLE_ARCHITECTURE` | — | Probe A |
| 端口值 | `ENV-PROPOSED` | 仓库默认 `8443`；仍需目标环境确认 | `ROLE_SERVER_PLATFORM` | 2026-07-29 | Probe A/F、G0-002/003/012 |
| 监听地址 | `ENV-PROPOSED` | 当前 skeleton 为 `0.0.0.0`；生产/实验室绑定策略未验证 | `ROLE_SERVER_PLATFORM` | 2026-07-29 | Probe A/F、G0-003/012 |
| OS 发行版和版本 | `ENV-UNFROZEN` | 待选择唯一主支持版本 | `ROLE_SERVER_PLATFORM` | 2026-07-29 | Probe F、G0-012 |
| CPU architecture | `ENV-UNFROZEN` | 目标包架构待确认；Step 1 记录的 `linux_amd64` Caddy 仅为 packaging candidate，不构成目标环境签收 | `ROLE_RELEASE` | 2026-07-29 | Probe F、G0-012 |
| systemd 版本下限 | `ENV-UNFROZEN` | 待目标 OS 实测 | `ROLE_SERVER_PLATFORM` | 2026-07-29 | Probe F、G0-012 |
| SQLite runtime 版本 | `ENV-UNFROZEN` | 待目标 OS 与 SQLx runtime 实测 | `ROLE_SERVER` | 2026-07-29 | G0-001/012 |
| 实验室 Server IP literal | `ENV-UNFROZEN` | 禁止编造；由实验室网络分配 | `ROLE_LAB_NETWORK` | 2026-07-29 | Probe A/B/F、G0-002/003/006 |

## 2. Client

| 字段 | 状态 | 当前值或待决项 | Owner | 截止日期 | 阻塞 |
|---|---|---|---|---|---|
| 产品进程拓扑 | `ARCH-FROZEN` | Daemon、Privileged Helper、Caddy path/service 为系统组件；Session Agent 位于认证后的桌面会话内并由系统级 XDG Autostart 直接启动；无 Session user unit、无 Identity Guard | `ROLE_CLIENT` | — | G0-010/013 |
| 安装 endpoint | `ARCH-FROZEN` | 仅 Server IP literal + port，经 debconf/preseed/显式环境输入；无 TOFU | `ROLE_PACKAGING` | — | Probe A/F、G0-002 |
| OS 发行版和版本 | `ENV-UNFROZEN` | 待选择唯一主支持版本 | `ROLE_CLIENT_PLATFORM` | 2026-07-29 | Probe D/E/F、G0-010/011/012 |
| kernel 基线 | `ENV-UNFROZEN` | 待目标 OS 确认 | `ROLE_CLIENT_PLATFORM` | 2026-07-29 | Probe D/E |
| 根文件系统 | `ENV-UNFROZEN` | 待验证 OverlayFS、ACL、xattr 和 root-disk identity 行为 | `ROLE_CLIENT_PLATFORM` | 2026-07-29 | Probe D/E、G0-011 |
| systemd/logind | `ENV-UNFROZEN` | 待目标 OS 实测 | `ROLE_CLIENT_PLATFORM` | 2026-07-29 | Probe E/F、G0-010/012 |
| 服务用户/组 | `ENV-PROPOSED` | 用户 `natsume`、`natsume-caddy`；组 `natsume-gateway`（不是独立用户）；当前 sysusers 使用动态 UID/GID | `ROLE_RELEASE` | 2026-08-01 | Probe F、G0-012 |
| contest 用户与 UID/GID | `ENV-UNFROZEN` | 设计要求固定 contest 用户，具体值未确定 | `ROLE_CLIENT_PLATFORM` | 2026-07-29 | Probe E、G0-010 |

## 3. Desktop / Kiosk

| 字段 | 状态 | 当前值或待决项 | Owner | 截止日期 | 阻塞 |
|---|---|---|---|---|---|
| Display Manager 边界 | `ARCH-FROZEN` | GDM/LightDM 只负责认证并启动 Desktop Session；Agent 不使用 DM plugin、greeter extension 或 DM-specific IPC。Daemon/Helper 通过 logind 与固定部署配置编排 session | `ROLE_ARCHITECTURE` | — | Probe E、G0-010 |
| 必测组合 A | `ENV-UNFROZEN` | GNOME + GDM + Wayland | `ROLE_DESKTOP` | 2026-07-29 | Probe E/F、G0-010/012 |
| 必测组合 B | `ENV-UNFROZEN` | LightDM 启动的目标 X11 desktop（Xfce/MATE 等由目标发行版冻结一种） | `ROLE_DESKTOP` | 2026-07-29 | Probe E/F、G0-010/012 |
| Agent 启动 | `ARCH-FROZEN` | `/etc/xdg/autostart/org.natsume.SessionAgent.desktop` 直接执行 `--autostart`；无 systemd user unit、无 display 环境转交；进程初始 resident + hidden | `ROLE_CLIENT` | — | Probe E/F、G0-010/012/013 |
| GUI 技术栈 | `ARCH-FROZEN` | Phase 6 使用 build-time compiled Slint + winit backend + Skia renderer；无 runtime interpreter、外部 GUI helper 或第一方低层 GUI 拼装 | `ROLE_CLIENT` | — | Probe E/F、G0-010/012/014 |
| Autologin | `ENV-UNFROZEN` | 待按各 Display Manager 的 package-owned 固定部署配置冻结；Agent 不参与 | `ROLE_DESKTOP` | 2026-08-01 | Probe E |
| lock/unlock API | `ENV-UNFROZEN` | 必须在两套目标桌面验证真实 desktop/logind lock state；失败时更换 Desktop 或明确不支持 | `ROLE_DESKTOP` | 2026-08-01 | Probe E、G0-010 |
| Session 与 Caddy 边界 | `ARCH-FROZEN` | lock/unlock/terminate 不得 reload、切换或阻断 Caddy | `ROLE_ARCHITECTURE` | — | Probe E、G0-010 |

## 4. Browser

| 字段 | 状态 | 当前值或待决项 | Owner | 截止日期 | 阻塞 |
|---|---|---|---|---|---|
| 唯一主支持浏览器 | `ENV-UNFROZEN` | Firefox 或 Chromium 二选一 | `ROLE_BROWSER` | 2026-07-29 | Probe C/E |
| 主支持版本 | `ENV-UNFROZEN` | 待与目标 OS package/policy 对齐 | `ROLE_BROWSER` | 2026-08-01 | Probe C/E |
| Managed policy | `ENV-UNFROZEN` | 路径与部署方式待验证 | `ROLE_BROWSER` | 2026-08-05 | Probe E |
| Local Origin trust store | `ENV-UNFROZEN` | 公共根注入方式待验证 | `ROLE_BROWSER` | 2026-08-05 | Probe C/E |

## 5. DOMjudge

| 字段 | 状态 | 当前值或待决项 | Owner | 截止日期 | 阻塞 |
|---|---|---|---|---|---|
| 唯一主支持版本 | `ENV-UNFROZEN` | 待选择 | `ROLE_DOMJUDGE` | 2026-07-29 | Probe C |
| HTTP contract | `ENV-UNFROZEN` | X-Headers、Cookie、CSRF、redirect、submission、Brotli | `ROLE_DOMJUDGE` | 2026-08-05 | Probe C |
| 网络边界 | `ARCH-FROZEN` | Browser 访问 loopback HTTPS Caddy；Caddy 访问 trusted-LAN DOMjudge | `ROLE_ARCHITECTURE` | — | Probe C |

## 6. Caddy

| 字段 | 状态 | 当前值或待决项 | Owner | 截止日期 | 阻塞 |
|---|---|---|---|---|---|
| 运行边界 | `ARCH-FROZEN` | 独立非 root、loopback HTTPS、Admin Unix socket、visual 503 bootstrap | `ROLE_CLIENT` | — | Probe C/F |
| 包内路径 | `ENV-PROPOSED` | `/usr/lib/natsume/caddy` | `ROLE_RELEASE` | 2026-07-29 | Probe C/F |
| 版本 | `ENV-PROPOSED` | 官方 Caddy `2.11.4`；版本已写入 `packaging/client/caddy.version`，目标 OS/contract 尚未验证 | `ROLE_CADDY_SUPPLY` | 2026-07-29 | Probe C/F、G0-012/013 |
| Module set | `ENV-PROPOSED` | 官方 standard distribution、无 custom modules；`caddy.modules` 记录 bootstrap/代理所需模块，完整运行时核验待 Probe C/F | `ROLE_CADDY_SUPPLY` | 2026-07-29 | Probe C |
| 构建/获取来源 | `ENV-PROPOSED` | 官方 GitHub `caddy_2.11.4_linux_amd64.tar.gz`；只允许构建阶段获取并验证，禁止 postinstall/runtime 下载 | `ROLE_CADDY_SUPPLY` | 2026-07-29 | Probe C/F、G0-013 |
| SHA-256 | `ENV-PROPOSED` | 已记录并本地核验官方 archive SHA-256 与 extracted binary SHA-256；本地 Deb content smoke 已通过，hosted run、目标 OS 和供应链签收仍待完成 | `ROLE_CADDY_SUPPLY` | 2026-07-29 | Probe C/F、G0-013 |

## 7. Hardware / Machine Identity

| 字段 | 状态 | 当前值或待决项 | Owner | 截止日期 | 阻塞 |
|---|---|---|---|---|---|
| Identity 模型 | `ARCH-FROZEN` | UUIDv5 `MachineHardwareId`；`fleet_namespace_uuid` 站点级不可变；无 installation instance | `ROLE_SECURITY` | — | Probe D、G0-011 |
| Collector 边界 | `ARCH-FROZEN` | 原始序列号只在 Helper 内存；Daemon 只接收候选 UUID/质量；无 `/etc/machine-id` fallback | `ROLE_SECURITY` | — | Probe D |
| 物理机数量 | `ENV-UNFROZEN` | 目标至少 6 台；当前无已验证资产 | `ROLE_LAB_HARDWARE` | 2026-08-01 | Probe D、G0-011 |
| OEM/主板覆盖 | `ENV-UNFROZEN` | 目标至少 2 个系列 | `ROLE_LAB_HARDWARE` | 2026-08-01 | Probe D、G0-011 |
| 存储覆盖 | `ENV-UNFROZEN` | 至少 SATA 与 NVMe | `ROLE_LAB_HARDWARE` | 2026-08-01 | Probe D、G0-011 |
| Fixture 截止 | `ENV-UNFROZEN` | 六台 sanitized fixture 与预期 UUIDv5 | `ROLE_LAB_EVIDENCE` | 2026-08-08 | Probe D、G0-011 |

## 8. Home Backend

| 字段 | 状态 | 当前值或待决项 | Owner | 截止日期 | 阻塞 |
|---|---|---|---|---|---|
| 默认后端 | `ARCH-FROZEN` | OverlayFS | `ROLE_CLIENT` | — | Probe E |
| Fallback | `ARCH-FROZEN` | 目标环境失败时部署期固定 staged-copy；不得运行时任意切换 | `ROLE_CLIENT` | — | Probe E |
| 目标环境兼容性 | `ENV-UNFROZEN` | OverlayFS、ACL、xattr、Browser/IDE、reboot 待实测 | `ROLE_HOME` | 2026-08-08 | Probe E、G0-014 |

## 9. PKI 与信任边界

| 字段 | 状态 | 当前值或待决项 | Owner | 截止日期 | 阻塞 |
|---|---|---|---|---|---|
| Enrollment | `ARCH-FROZEN` | server-auth HTTPS；只提交 Device CSR；只返回 Device leaf/chain | `ROLE_PKI` | — | Probe B、G0-005/006 |
| Control transport | `ARCH-FROZEN` | mandatory-mTLS QUIC；匿名连接不得进入 Protobuf；0-RTT 禁用 | `ROLE_PKI` | — | Probe B、G0-004 |
| Gateway issuance | `ARCH-FROZEN` | 仅 authenticated QUIC + active `SYNC_STATE`；忽略 CSR SAN，按 target 签发 | `ROLE_PKI` | — | Probe B、G0-006–009 |
| Control Trust Root | `ARCH-FROZEN` | 站点级 offline root；包只含公钥证书，私钥不进 runtime/package | `ROLE_PKI` | — | Probe A/F |
| Local Origin Root | `ARCH-FROZEN` | 站点级 offline root；公钥进入 Browser trust | `ROLE_PKI` | — | Probe C/F |
| Device Issuing CA | `ARCH-FROZEN` | 每实例 CA，只签 Device clientAuth leaf | `ROLE_PKI` | — | Probe B |
| Origin Intermediate | `ARCH-FROZEN` | 每实例 intermediate，只签 Gateway serverAuth leaf | `ROLE_PKI` | — | Probe B/C |
| 实际测试 CA 物料 | `ENV-UNFROZEN` | 只使用 test CA；生成和临时存储流程待定义，不提交私钥 | `ROLE_PKI` | 2026-08-01 | Probe A/B/F |
| 站点公开材料注入 | `ENV-UNFROZEN` | `site.toml`、Control Root 公钥证书、Local Origin Root 公钥证书的构建期输入和验证流程待签收 | `ROLE_PACKAGING` | 2026-08-01 | Probe A/C/F、G0-002/012/013 |

## 10. 构建工具链登记

| 工具 | 状态 | 当前值或待决项 | Owner | 截止日期 | 阻塞 |
|---|---|---|---|---|---|
| Rust | `ENV-PROPOSED` | 仓库声明 `1.97.1`；本地已通过 format、Clippy `-D warnings`、tests、doc tests 和 `cargo deny check`，GitHub-hosted run 尚未签收 | `ROLE_BUILD` | 2026-07-29 | G0-001 |
| pnpm | `ENV-PROPOSED` | 仓库固定 `11.1.0` 并生成 `pnpm-lock.yaml`；本地 frozen install、Web gates 和 contract generation 已通过，GitHub-hosted run 尚未签收 | `ROLE_WEB` | 2026-07-29 | G0-001 |
| Node.js | `ENV-PROPOSED` | `.node-version` 与 `engines.node` 固定 `24.1.0`，已在开发环境核对；GitHub-hosted run 尚未签收 | `ROLE_WEB` | 2026-07-29 | G0-001 |
| nFPM | `ENV-PROPOSED` | 官方 `2.47.0` Linux x86_64 host-tool archive 版本和 SHA-256 已记录；本地 Deb content smoke 已通过，目标 OS lifecycle 与 hosted run 尚未签收 | `ROLE_RELEASE` | 2026-07-29 | Probe F、G0-012 |
| Protobuf compiler | `ENV-PROPOSED` | `build.rs` 使用 `protoc-bin-vendored`，Cargo.lock 固定 crate `3.2.0`；当前 contract tests 已进入 CI，但 Step 4 descriptor 可复现性尚未实施 | `ROLE_PROTOCOL` | 2026-07-29 | G0-001 |
| Mermaid validator | `ENV-PROPOSED` | 精确固定 `mermaid@11.16.0`，由 `docs/validate-mermaid.mjs` 校验仓库 Mermaid fences；本地已验证 23 个图，hosted run 尚未签收 | `ROLE_DOCS` | 2026-07-29 | G0-001 |
| PR/push CI | `ENV-PROPOSED` | `.github/workflows/ci.yml` 已实现 Rust、Web、contracts、policy-scan、packages 五类门禁；GitHub Actions 固定到检索时最新的精确 release version，actionlint 与本地 parity 已通过，尚无 hosted-run 签收证据 | `ROLE_BUILD` | 2026-07-29 | G0-001/012/013 |
| Nightly shared-runner smoke | `ENV-PROPOSED` | `.github/workflows/nightly.yml` 每日 `03:17 UTC` 运行 shared-runner smoke；只提供回归信号，不是目标 OS、物理实验室、reboot 或 G0 证据 | `ROLE_BUILD` | 2026-07-29 | G0-001/012/013 |

## 11. 冻结流程

1. Owner 在截止日前以目标环境或供应链证据更新对应行。
2. 架构影响项必须新增 ADR，并列出受影响的 Probe/G0。
3. 逾期项保持 `ENV-UNFROZEN`，同时在 `docs/gates/g0-checklist.md` 标记 `BLOCKED-INPUT`。
4. 本文件不记录 Probe/G0 的 PASS/FAIL；证据由后续 probe report 和 evidence index 承载。
