# Phase 0 需求与追踪

> 状态：`DRAFT-STEP0`  
> 范围：P0.1–P0.7，Gate G0
> 规则：稳定 ASCII ID；本文件创建时全部需求为 `OPEN`，不代表实现或验收完成。

## 状态定义

`OPEN`、`IN-PROGRESS`、`BLOCKED-INPUT`、`SATISFIED`、`FAILED`。

证书边界、匿名 QUIC、无 Identity Guard、无 systemd credentials、无 runtime download、无 TOFU 均不可豁免。

## 1. 非协商证书阶梯

```text
HTTPS Enrollment（server-auth）
→ 只提交 Device Identity CSR
→ 只返回 Device Identity leaf/chain
→ READY-DEVICE-ID

mandatory-mTLS QUIC（Device certificate）
→ mock/active SYNC_STATE
→ Gateway CSR（command/generation/configuration/SPKI 绑定）
→ Server 从 target 派生 SAN/profile
→ READY-GATEWAY-CERT
```

`READY-DEVICE-ID` 与 `READY-GATEWAY-CERT` 仅是 Gate/需求文档中的叙述别名，不是新的 wire、API、数据库或运行时状态字段。它们分别概括 Device Identity certificate 已签发并可用于 mTLS，以及 Gateway certificate 仅在 authenticated `SYNC_STATE` 子协议成功后就绪。Enrollment 成功不得被解释为 Gateway 已准备；实现仍以权威设计中的 Enrollment、Gateway state 和 `waiting_for_gateway_certificate` 等状态为准。

## 2. P0.1 Monorepo 与工具链

| ID | 需求 | Probe | G0 | 状态 |
|---|---|---|---|---|
| `REQ-P0-001` | clean checkout 可运行真实 Rust、Web、契约和打包检查，无占位任务或缺工具跳过 | CI | G0-001 | `OPEN` |
| `REQ-P0-002` | Cargo 拥有 Rust graph/lockfile；pnpm 只拥有 Web graph/lockfile；just 只分发；nFPM 只映射产物 | CI | G0-001 | `OPEN` |
| `REQ-P0-003` | Rust、Node、pnpm、nFPM、Mermaid、Caddy、protoc 均有可审计 pin | C/F | G0-001/012/013 | `OPEN` |
| `REQ-P0-004` | ADR、requirement-ID、Gate evidence 流程存在且可追踪 | DOC | G0-014/015 | `OPEN` |

## 3. P0.2 真实 CI

| ID | 需求 | Probe | G0 | 状态 |
|---|---|---|---|---|
| `REQ-P0-010` | PR 执行 Rust fmt、Clippy、unit/doc test、cargo-deny/advisory | CI | G0-001 | `OPEN` |
| `REQ-P0-011` | PR 执行 pnpm frozen install、format、lint、typecheck、unit、build、Phase 0 Playwright | CI | G0-001 | `OPEN` |
| `REQ-P0-012` | PR 执行 OpenAPI/TS、Proto descriptor、D-Bus、Mermaid、SQL migration 契约检查 | CI | G0-001/005 | `OPEN` |
| `REQ-P0-013` | CI 扫描 secret、private key、systemd credentials、Identity Guard、第一方 anyhow/thiserror、XLSX/ODS 与协议禁用项 | B/F | G0-001/005/013 | `OPEN` |
| `REQ-P0-014` | Nightly 在目标 OS VM 执行 package lifecycle、reboot/fault 与依赖扫描 | F | G0-012 | `OPEN` |

## 4. P0.3 SNAFU 与稳定错误码

| ID | 需求 | Probe | G0 | 状态 |
|---|---|---|---|---|
| `REQ-P0-020` | 建立跨 HTTP、Protobuf、D-Bus、CommandStatus 的稳定 ErrorCode registry | B | G0-001/007/008 | `OPEN` |
| `REQ-P0-021` | HTTP 使用 Problem Details 显式映射稳定码 | CI | G0-001 | `OPEN` |
| `REQ-P0-022` | Protobuf `ProtocolError` 与 D-Bus error 使用显式稳定码 | B/E | G0-004/007/010 | `OPEN` |
| `REQ-P0-023` | 禁止解析 Display 文案判断业务；secret/path/source-chain 必须脱敏 | CI | G0-001/013 | `OPEN` |

## 5. P0.4 契约骨架

| ID | 需求 | Probe | G0 | 状态 |
|---|---|---|---|---|
| `REQ-P0-030` | Protobuf 生成 descriptor golden，并验证 framing、max-size、enum/oneof | B | G0-001/004 | `OPEN` |
| `REQ-P0-031` | Enrollment schema、DB fixture、runtime request/response 只包含 Device CSR/leaf/chain | B | G0-005 | `OPEN` |
| `REQ-P0-032` | Gateway request/result 是 authenticated QUIC 下绑定 `SYNC_STATE` 的专用协议 | B | G0-006–009 | `OPEN` |
| `REQ-P0-033` | OpenAPI 从 Rust schema 生成，TypeScript snapshot clean-diff，Enrollment 无 Gateway 字段 | B/CI | G0-001/005 | `OPEN` |
| `REQ-P0-034` | D-Bus XML、Rust value types 和 policy 一致；Session lock contract 不拥有 Caddy state | E | G0-010 | `OPEN` |
| `REQ-P0-035` | SQL migration 真实执行；Enrollment 表无 Gateway 列 | B/CI | G0-005 | `OPEN` |
| `REQ-P0-036` | 协议和 CI 禁止 `CertificateIssueRequest` 与 `INSTALL_CERTIFICATE` | B/CI | G0-005 | `OPEN` |
| `REQ-P0-037` | 文档、OpenAPI 和 UI 文案必须区分 Device Identity 与 Gateway certificate 就绪；禁止将 Enrollment 成功描述为 Gateway 已准备 | B/DOC/CI | G0-005/006 | `OPEN` |
| `REQ-P0-038` | Session Agent 只由桌面会话通过系统级 XDG Autostart 直接启动；无 systemd user unit、无环境转交文件；启动后保持 resident + hidden，只有 typed UI snapshot 才懒创建窗口 | E/F/CI | G0-010/012/013 | `OPEN` |
| `REQ-P0-039` | Phase 6 GUI 固定采用 build-time compiled Slint（winit + Skia）；产品不得直接拼装 winit/softbuffer/tiny-skia/cosmic-text，不得依赖 runtime interpreter、外部 GUI helper、Node/Python/JVM；GNOME/GDM/Wayland 与 LightDM 启动的目标 X11 desktop 均需实测 | E/F | G0-010/012/014 | `OPEN` |

## 6. P0.5 空壳 Deb 与安装配置

| ID | 需求 | Probe | G0 | 状态 |
|---|---|---|---|---|
| `REQ-P0-040` | 可构建并安装空壳 `natsume-server` 与 `natsume-client` Deb | F | G0-012 | `OPEN` |
| `REQ-P0-041` | debconf/preseed 只收集 Server IP literal 与 port，并由 daemon 验证 | A/F | G0-002 | `OPEN` |
| `REQ-P0-042` | upgrade/reinstall 保留有效 endpoint，仅 explicit reconfigure/override 重写 | A/F | G0-002 | `OPEN` |
| `REQ-P0-043` | 最终用户、目录、mode、sysusers、tmpfiles、D-Bus、Caddy topology 与 XDG Autostart entry 符合设计；Client package 不包含 Session Agent systemd user unit | F | G0-010/012/013 | `OPEN` |
| `REQ-P0-044` | postinst 不下载组件、不生成 token/CA/private key | F | G0-013 | `OPEN` |
| `REQ-P0-045` | 不存在 Identity Guard service/unit | F | G0-013 | `OPEN` |
| `REQ-P0-046` | 不使用 `LoadCredential`/`SetCredential` | F/CI | G0-013 | `OPEN` |

## 7. P0.6 六项探针

| ID | 需求 | Probe | G0 | 状态 |
|---|---|---|---|---|
| `REQ-P0-050` | IP-SAN 匹配成功；错误 IP、错误 CA、过期证书失败 | A | G0-003 | `OPEN` |
| `REQ-P0-051` | 同一数字端口可同时提供 TCP HTTPS 与 UDP QUIC | A | G0-003 | `OPEN` |
| `REQ-P0-052` | Device-only Enrollment runtime 可恢复并只返回 Device leaf/chain | B | G0-005/006 | `OPEN` |
| `REQ-P0-053` | 匿名 QUIC 在 TLS 阶段拒绝，Protobuf decode counter 为 0，0-RTT 关闭 | B | G0-004 | `OPEN` |
| `REQ-P0-054` | 只有 authenticated peer 的 active `SYNC_STATE` 可提交 Gateway CSR | B | G0-006/007 | `OPEN` |
| `REQ-P0-055` | 相同 request/SPKI 幂等返回同一结果；不同 SPKI conflict | B | G0-008 | `OPEN` |
| `REQ-P0-056` | Server 忽略 CSR SAN，按 target hostname/profile 签发 | B | G0-009 | `OPEN` |
| `REQ-P0-057` | 无 active command、错误 generation/configuration、匿名连接均稳定码拒绝且不降级 HTTPS/Enrollment | B | G0-007 | `OPEN` |
| `REQ-P0-060` | Caddy 固定版本/modules/checksum；loopback HTTPS、visual 503、Admin Unix socket 可验证 | C | G0-012/013 | `OPEN` |
| `REQ-P0-061` | 至少 6 台物理 Machine ID fixture，覆盖至少 2 个 OEM/主板系列和 SATA/NVMe | D | G0-011 | `OPEN` |
| `REQ-P0-062` | Machine ID 覆盖 placeholder、重复、缺失、permission denied、configured-disk copy | D | G0-011 | `OPEN` |
| `REQ-P0-063` | desktop lock/unlock 的 Caddy Admin 调用次数、config hash 和 epoch 均不变 | E | G0-010 | `OPEN` |
| `REQ-P0-064` | OverlayFS 在目标环境验证；失败时以 ADR 固定 staged-copy | E | G0-014 | `OPEN` |
| `REQ-P0-065` | Deb 在目标 OS 完成 install/reinstall/upgrade/remove/purge/reboot | F | G0-012 | `OPEN` |
| `REQ-P0-066` | Caddy 状态页只接受 allowlist JSON；主页面 503；禁止 `session_locked`、secret、free-form error；动态值仅通过 `textContent` 渲染 | C/E | G0-010/013 | `OPEN` |
| `REQ-P0-070` | 六份 probe report 均包含环境、步骤、正反用例、结果、证据、ADR、owner 和限制 | A–F | G0-014 | `OPEN` |

## 8. Gate 与证据

| ID | 需求 | G0 | 状态 |
|---|---|---|---|
| `REQ-P0-080` | 每个 G0 条目均可追踪到 REQ、测试或实验室证据 | G0-001–015 | `OPEN` |
| `REQ-P0-081` | Gate decision 由架构、工程和 QA 角色签署 | G0-015 | `OPEN` |
| `REQ-P0-082` | 关键平台输入未冻结时，对应需求必须保持 `BLOCKED-INPUT`，不得默许通过 | G0-014/015 | `OPEN` |

## 9. Probe 映射

| Probe | 主题 | 主要 REQ |
|---|---|---|
| A | IP-SAN 与 endpoint | REQ-P0-041/042/050/051 |
| B | Enrollment → mTLS → Gateway CSR | REQ-P0-031/032/037/052–057 |
| C | Caddy/DOMjudge | REQ-P0-003/060/066 |
| D | Machine Identity | REQ-P0-061/062 |
| E | E1: XDG/Slint；E2: Session lock/Home | REQ-P0-034/038/039/063/064/066 |
| F | Package/systemd | REQ-P0-040–046/065 |

## 10. 非目标

- `NONREQ-P0-001`：完整领域 CRUD、Auth/RBAC、SSE。
- `NONREQ-P0-002`：生产 CSV、Preparation Center 和业务 Web 页面。
- `NONREQ-P0-003`：真实 Command executor、fleet simulator、容量签收。
- `NONREQ-P0-004`：生产 Caddy runtime 配置生成器。
- `NONREQ-P0-005`：完整 Session/Home 状态机。
- `NONREQ-P0-006`：将 Gateway CSR/certificate 加入 Enrollment。

## 11. 变更控制

未连续使用的编号是工作包内的预留扩展位，不代表需求遗漏。

1. 新需求只能追加 ID，不复用已发布 ID。
2. 修改证书边界必须同步更新权威设计、ADR、契约和测试。
3. 需求只有在证据可定位时才能标记 `SATISFIED`。
4. 本文件不记录 G0 总体 PASS；总体判定只存在于后续 Gate decision。
