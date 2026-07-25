<!-- GENERATED FILE: DO NOT EDIT DIRECTLY -->
<!-- Source: docs/verification/registry.json; regenerate with node docs/verification/render.mjs --write -->

# Phase 0 需求与追踪

> Registry 更新：2026-07-24  
> Phase 窗口：2026-07-23 至 2026-08-12  
> 当前所有 requirement 状态以 registry 为准；文档生成不代表实现或验收完成。

权威规则：

- [Verification Registry](../verification/README.md)
- [架构](../architecture.md)
- [契约](../contracts.md)
- [安全不变量](../security-recovery.md)
- [G0 检查清单](../gates/g0-checklist.md)

## 状态定义

`OPEN`、`IN-PROGRESS`、`BLOCKED-INPUT`、`SATISFIED`、`FAILED`、`RETIRED`

## P0.1

| ID | 需求 | Probe | Gate | 不变量 | 状态 | Evidence |
|---|---|---|---|---|---|---|
| `REQ-P0-001` | clean checkout 可运行真实 Rust、Web、契约和打包检查，无占位任务或缺工具跳过 | `CI` | `G0-001` | — | `OPEN` | 未产生 |
| `REQ-P0-002` | Cargo 拥有 Rust graph/lockfile；pnpm 只拥有 Web graph/lockfile；just 只分发；nFPM 只映射产物 | `CI` | `G0-001` | — | `OPEN` | 未产生 |
| `REQ-P0-003` | Rust、Node、pnpm、nFPM、Mermaid、Caddy、protoc 均有可审计 pin | `C`、`F` | `G0-001`、`G0-012`、`G0-013` | — | `OPEN` | 未产生 |
| `REQ-P0-004` | ADR、requirement ID、Gate evidence 流程存在且可追踪 | `DOC` | `G0-014`、`G0-015` | — | `OPEN` | 未产生 |

## P0.2

| ID | 需求 | Probe | Gate | 不变量 | 状态 | Evidence |
|---|---|---|---|---|---|---|
| `REQ-P0-010` | PR 执行 Rust fmt、Clippy、unit/doc test、cargo-deny/advisory | `CI` | `G0-001` | — | `OPEN` | 未产生 |
| `REQ-P0-011` | PR 执行 pnpm frozen install、format、lint、typecheck、unit、build和 Phase 0 Playwright | `CI` | `G0-001` | — | `OPEN` | 未产生 |
| `REQ-P0-012` | PR 执行 OpenAPI/TS、Proto descriptor、D-Bus、Mermaid、SQL migration 契约检查 | `CI` | `G0-001`、`G0-005` | `INV-CERT-01` | `OPEN` | 未产生 |
| `REQ-P0-013` | CI 扫描 secret、private key、systemd credentials、Identity Guard、第一方 anyhow/thiserror、XLSX/ODS 与协议禁用项 | `B`、`F` | `G0-001`、`G0-005`、`G0-013` | `INV-PRIVILEGE-01`、`INV-SECRET-01` | `OPEN` | 未产生 |
| `REQ-P0-014` | Nightly 在目标 OS VM 执行 package lifecycle、reboot/fault 与依赖扫描 | `F` | `G0-012` | — | `OPEN` | 未产生 |

## P0.3

| ID | 需求 | Probe | Gate | 不变量 | 状态 | Evidence |
|---|---|---|---|---|---|---|
| `REQ-P0-020` | 建立跨 HTTP、Protobuf、D-Bus、CommandStatus 的稳定 ErrorCode registry | `B` | `G0-001`、`G0-007`、`G0-008` | — | `OPEN` | 未产生 |
| `REQ-P0-021` | HTTP 使用 Problem Details 显式映射稳定码 | `CI` | `G0-001` | — | `OPEN` | 未产生 |
| `REQ-P0-022` | Protobuf ProtocolError 与 D-Bus error 使用显式稳定码 | `B`、`E` | `G0-004`、`G0-007`、`G0-010` | — | `OPEN` | 未产生 |
| `REQ-P0-023` | 禁止解析 Display 文案判断业务；secret/path/source-chain 必须脱敏 | `CI` | `G0-001`、`G0-013` | `INV-SECRET-01` | `OPEN` | 未产生 |

## P0.4

| ID | 需求 | Probe | Gate | 不变量 | 状态 | Evidence |
|---|---|---|---|---|---|---|
| `REQ-P0-030` | Protobuf 生成 descriptor golden，并验证 framing、max-size、enum/oneof | `B` | `G0-001`、`G0-004` | `INV-INPUT-01` | `OPEN` | 未产生 |
| `REQ-P0-031` | Enrollment schema、DB fixture、runtime request/response 只包含 Device CSR/leaf/chain | `B` | `G0-005`、`G0-006` | `INV-CERT-01` | `OPEN` | 未产生 |
| `REQ-P0-032` | Gateway request/result 是 authenticated QUIC 下绑定 SYNC_STATE 的专用协议 | `B` | `G0-006`、`G0-007`、`G0-008`、`G0-009` | `INV-CERT-01`、`INV-CERT-02` | `OPEN` | 未产生 |
| `REQ-P0-033` | OpenAPI 从 Rust schema 生成，TypeScript snapshot clean-diff，Enrollment 无 Gateway 字段 | `B`、`CI` | `G0-001`、`G0-005` | `INV-CERT-01` | `OPEN` | 未产生 |
| `REQ-P0-034` | D-Bus XML、Rust value types 和 policy 一致；Session lock contract 不拥有 Caddy state | `E` | `G0-010` | `INV-SESSION-01` | `OPEN` | 未产生 |
| `REQ-P0-035` | SQL migration 真实执行；Enrollment 表无 Gateway 列 | `B`、`CI` | `G0-005` | `INV-CERT-01` | `OPEN` | 未产生 |
| `REQ-P0-036` | 协议和 CI 禁止 CertificateIssueRequest 与 INSTALL_CERTIFICATE | `B`、`CI` | `G0-005` | `INV-CERT-01`、`INV-INPUT-01` | `OPEN` | 未产生 |
| `REQ-P0-037` | 文档、OpenAPI 和 UI 区分 Device Identity 与 Gateway certificate 就绪；Enrollment 成功不得描述为 Gateway 已准备 | `B`、`CI`、`DOC` | `G0-005`、`G0-006` | `INV-CERT-01` | `OPEN` | 未产生 |
| `REQ-P0-038` | Session Agent 只由系统级 XDG Autostart 直接启动；无 user unit、无环境转交文件；resident + hidden，typed snapshot 后懒创建窗口 | `E`、`F`、`CI` | `G0-010`、`G0-012`、`G0-013` | `INV-PRIVILEGE-01`、`INV-SESSION-01` | `OPEN` | 未产生 |
| `REQ-P0-039` | Phase 6 GUI 固定 build-time Slint winit + Skia；无手拼 GUI/runtime helper；双桌面目标环境实测 | `E`、`F` | `G0-010`、`G0-012`、`G0-014` | `INV-PRIVILEGE-01`、`INV-SESSION-01` | `OPEN` | 未产生 |

## P0.5

| ID | 需求 | Probe | Gate | 不变量 | 状态 | Evidence |
|---|---|---|---|---|---|---|
| `REQ-P0-040` | 可构建并安装空壳 natsume-server 与 natsume-client Deb | `F` | `G0-012` | — | `OPEN` | 未产生 |
| `REQ-P0-041` | debconf/preseed 只收集 Server IP literal 与 port，并由 daemon 验证 | `A`、`F` | `G0-002` | `INV-INPUT-01` | `OPEN` | 未产生 |
| `REQ-P0-042` | upgrade/reinstall 保留有效 endpoint，仅 explicit reconfigure/override 重写 | `A`、`F` | `G0-002` | — | `OPEN` | 未产生 |
| `REQ-P0-043` | 用户、目录、mode、sysusers、tmpfiles、D-Bus、Caddy topology 与 XDG entry 符合设计；无 Agent user unit | `F` | `G0-010`、`G0-012`、`G0-013` | `INV-PRIVILEGE-01` | `OPEN` | 未产生 |
| `REQ-P0-044` | postinst 不下载组件、不生成 token/CA/private key | `F` | `G0-013` | `INV-PRIVILEGE-01`、`INV-SECRET-01` | `OPEN` | 未产生 |
| `REQ-P0-045` | 不存在 Identity Guard service/unit | `F` | `G0-013` | `INV-IDENTITY-02` | `OPEN` | 未产生 |
| `REQ-P0-046` | 不使用 LoadCredential/SetCredential 传递产品秘密 | `F`、`CI` | `G0-013` | `INV-SECRET-01` | `OPEN` | 未产生 |

## P0.6

| ID | 需求 | Probe | Gate | 不变量 | 状态 | Evidence |
|---|---|---|---|---|---|---|
| `REQ-P0-050` | IP-SAN 匹配成功；错误 IP、错误 CA、过期证书失败 | `A` | `G0-003` | `INV-CERT-01` | `OPEN` | 未产生 |
| `REQ-P0-051` | 同一数字端口可同时提供 TCP HTTPS 与 UDP QUIC | `A` | `G0-003` | — | `OPEN` | 未产生 |
| `REQ-P0-052` | Device-only Enrollment runtime 可恢复并只返回 Device leaf/chain | `B` | `G0-005`、`G0-006` | `INV-CERT-01` | `OPEN` | 未产生 |
| `REQ-P0-053` | 匿名 QUIC 在 TLS 阶段拒绝，Protobuf decode counter 为 0，0-RTT 关闭 | `B` | `G0-004`、`G0-006` | `INV-CERT-01` | `OPEN` | 未产生 |
| `REQ-P0-054` | 只有 authenticated peer 的 active SYNC_STATE 可提交 Gateway CSR | `B` | `G0-006`、`G0-007` | `INV-CERT-01`、`INV-CERT-02` | `OPEN` | 未产生 |
| `REQ-P0-055` | 相同 request/SPKI 幂等返回同一结果；不同 SPKI conflict | `B` | `G0-008` | `INV-CERT-02`、`INV-COMMAND-01` | `OPEN` | 未产生 |
| `REQ-P0-056` | Server 忽略 CSR SAN，按 Target hostname/profile 签发 | `B` | `G0-009` | `INV-CERT-02` | `OPEN` | 未产生 |
| `REQ-P0-057` | 无 active command、错误 generation/configuration、匿名连接均稳定码拒绝且不降级 | `B` | `G0-007` | `INV-CERT-01`、`INV-CERT-02` | `OPEN` | 未产生 |
| `REQ-P0-060` | Caddy 固定版本/modules/checksum；loopback HTTPS、visual 503、Admin Unix socket 可验证 | `C` | `G0-012`、`G0-013` | `INV-DATAPLANE-01` | `OPEN` | 未产生 |
| `REQ-P0-061` | 至少 6 台物理 Machine ID fixture，覆盖至少 2 个 OEM/主板系列和 SATA/NVMe | `D` | `G0-011` | `INV-IDENTITY-01`、`INV-IDENTITY-02` | `OPEN` | 未产生 |
| `REQ-P0-062` | Machine ID 覆盖 placeholder、重复、缺失、permission denied、configured-disk copy | `D` | `G0-011` | `INV-IDENTITY-01`、`INV-IDENTITY-02` | `OPEN` | 未产生 |
| `REQ-P0-063` | desktop lock/unlock 的 Caddy Admin 调用次数、config hash 和 epoch 均不变 | `E` | `G0-010` | `INV-SESSION-01` | `OPEN` | 未产生 |
| `REQ-P0-064` | OverlayFS 在目标环境验证；失败时通过 ADR 固定 staged-copy | `E` | `G0-014` | `INV-SESSION-01` | `OPEN` | 未产生 |
| `REQ-P0-065` | Deb 在目标 OS 完成 install/reinstall/upgrade/remove/purge/reboot | `F` | `G0-012` | — | `OPEN` | 未产生 |
| `REQ-P0-066` | Caddy 状态页只接受 allowlist JSON；主页面 503；无 session_locked、secret、free-form error；动态值只用 textContent | `C`、`E` | `G0-010`、`G0-013` | `INV-DATAPLANE-01`、`INV-SECRET-01`、`INV-SESSION-01` | `OPEN` | 未产生 |
| `REQ-P0-070` | 六份 probe report 均包含环境、步骤、正反用例、结果、证据、ADR、owner 和限制 | `A`、`B`、`C`、`D`、`E`、`F` | `G0-014` | — | `OPEN` | 未产生 |

## Gate

| ID | 需求 | Probe | Gate | 不变量 | 状态 | Evidence |
|---|---|---|---|---|---|---|
| `REQ-P0-080` | 每个 G0 条目均可追踪到 requirement、测试或实验室证据 | `DOC` | `G0-001`、`G0-002`、`G0-003`、`G0-004`、`G0-005`、`G0-006`、`G0-007`、`G0-008`、`G0-009`、`G0-010`、`G0-011`、`G0-012`、`G0-013`、`G0-014`、`G0-015` | — | `OPEN` | 未产生 |
| `REQ-P0-081` | Gate decision 由架构、工程和 QA 角色签署 | `DOC` | `G0-015` | — | `OPEN` | 未产生 |
| `REQ-P0-082` | 关键平台输入未冻结时，对应需求保持 BLOCKED-INPUT，不得默许通过 | `DOC` | `G0-014`、`G0-015` | — | `OPEN` | 未产生 |

## 非目标

- 完整领域 CRUD、Auth/RBAC、SSE；
- 生产 CSV、Preparation Center 和业务 Web 页面；
- 真实 fleet Command executor 和生产 Caddy generator；
- 完整 Session/Home 状态机；
- 将 Gateway certificate 加入 Enrollment；
- 以文档或 scaffold 代替目标环境证据。

## 变更控制

1. 新 requirement 只追加 ID，不复用已发布 ID；
2. 修改 registry 后运行 renderer；
3. `SATISFIED` 必须有可定位 evidence；
4. 总体 Gate 结论只存在于独立签署 decision；
5. 证书、安全和特权边界不得通过 requirement waiver 放宽。
