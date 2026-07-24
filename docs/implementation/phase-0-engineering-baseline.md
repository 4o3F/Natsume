# Natsume V2 Phase 0 详细实施计划：工程基线与目标环境探针

> 架构基线：`Natsume_V2_Design_v2.7.md`  
> Roadmap 基线：`Natsume_V2_Implementation_Roadmap_v1.4.md`  
> 计划版本：Phase Plan v1.1  
> 基准窗口：W1–W3  
> Gate：G0  
> 前置依赖：无

---

## 1. 阶段使命与边界

Phase 0 把 Blueprint 转换为真实可构建、可测试、可安装的工程，并用最小探针验证会影响整体架构的环境假设。本阶段不实现完整业务功能，但所有探针都必须使用最终计划采用的技术边界，不能以 mock 掩盖关键风险。

本阶段必须特别证明证书生命周期的两步边界：

```text
server-auth HTTPS Enrollment
→ 只签发 Device Identity certificate
→ 使用该 certificate 建立 mandatory-mTLS QUIC
→ 在 mock SYNC_STATE 上下文中通过 QUIC 提交 Gateway CSR
→ Server 按命令快照签发 Gateway certificate
```

Enrollment 请求、数据库 fixture 和响应中不得出现 Gateway CSR、Gateway SPKI 或 Gateway leaf/chain。

### 不在本阶段实现

- 完整领域 CRUD；
- 生产 CSV 导入；
- 真实 Command executor；
- 生产 Caddy runtime 配置；
- Session/Home 完整状态机；
- 2,000 Device 正式容量签收。

---

## 2. 输入与冻结项

W1 结束前冻结：

| 类别 | 必须冻结的内容 |
|---|---|
| Server | OS、architecture、systemd、SQLite runtime、监听地址与 TCP/UDP 端口 |
| Client | OS、kernel、filesystem、systemd/logind、目标用户与 UID/GID |
| Desktop | Display Manager、Desktop/Kiosk shell、autologin、lock/unlock API |
| Browser | Firefox 或 Chromium 的唯一主支持版本、managed policy 与 trust store |
| DOMjudge | 唯一主支持版本、X-Headers、Cookie、CSRF、redirect、submission contract |
| Caddy | 固定版本、模块集合、构建方式与 SHA-256 管理方式 |
| Hardware | 至少 6 台、至少 2 个 OEM/主板系列、SATA/NVMe 覆盖 |
| Home | OverlayFS 默认；失败时部署期固定 staged-copy fallback |
| PKI | Control Root、Local Origin Root、Server IP-SAN leaf、Device Issuing CA、Origin Intermediate 的 ownership |
| 安装 | Server IP/port debconf/preseed、站点 namespace 与公开 root 注入方式 |

任何未冻结项都必须记录 owner、决策期限和对后续 Gate 的阻塞影响。

---

## 3. 详细工作包

### P0.1 Monorepo 与原生工具链

- 建立正式 Git monorepo、branch protection、CODEOWNERS、signed tag 规则；
- 生成并提交真实 `Cargo.lock`、`pnpm-lock.yaml`；
- 固定 Rust toolchain、Node、pnpm、nFPM、Mermaid CLI、Caddy 版本；
- 验证 Cargo virtual workspace、pnpm workspace、根 `justfile` 的 ownership；
- 建立 version/changelog/ADR/requirement-ID 流程；
- 禁止 `common/utils/helpers` 垃圾桶模块和第二套 build graph。

### P0.2 真实 CI

PR 必跑：

- Rust fmt、Clippy、unit/doc tests、cargo-deny、RustSec；
- Web frozen install、lint、typecheck、unit、build；
- OpenAPI、Protobuf descriptor、D-Bus contract、Mermaid clean diff；
- SQL migration execution；
- nFPM input、package contents、systemd、D-Bus policy、shell syntax；
- secret canary、private key、`LoadCredential`/`SetCredential`、`anyhow`/`thiserror`、XLSX/ODS 依赖禁用扫描；
- Enrollment schema 禁止 Gateway CSR/certificate 字段；
- Protobuf 禁止通用 `CertificateIssueRequest` 和 `INSTALL_CERTIFICATE`。

Nightly：目标 OS VM、reboot/fault、dependency scan。Scheduled：fleet/load smoke。

### P0.3 SNAFU 与稳定错误码

- 建立 ErrorCode registry 和命名空间；
- 统一 HTTP Problem Details、Protobuf `ProtocolError`、D-Bus error、CommandStatus mapping；
- 建立 secret/path/source-chain redaction wrapper；
- binary 顶层 `snafu::Report`；
- 禁止通过 `Display` 文案解析业务错误；
- 为 Enrollment、Gateway certificate、vault、Session、Home 预留稳定错误码。

### P0.4 Contract skeleton

- `.proto` + vendored protoc + descriptor/golden fixture harness；
- OpenAPI snapshot + generated TypeScript；
- D-Bus XML/value types；
- 最小 `TargetStateSnapshot`、`ObservedStateSnapshot`、`Command`、`GatewayCertificateRequest/Result`；
- Enrollment request/result 只包含 Device CSR/leaf；
- framing、message-size、enum/oneof validation harness。

### P0.5 空壳 Debian 包与安装配置

- 构建可安装的 `natsume-server`、`natsume-client` 空壳 Deb；
- Client debconf 交互与 preseed 保存 canonical Server IP/port；
- config upgrade/reinstall preservation；
- 最终用户、group、目录、mode、sysusers、tmpfiles、D-Bus policy；
- 仅三个产品Rust binary、Caddy service/path与system-wide XDG Autostart entry；无Session Agent user unit；
- 无 Identity Guard service；
- postinst 不下载 Caddy、CA、crate、npm package，不生成 token。

### P0.6 六项基础探针

#### A. Server IP SAN 与安装 endpoint

- IPv4/IPv6 literal；
- 同一数字端口的 TCP HTTPS 与 UDP QUIC；
- rustls IP SAN 正向、错误 IP、错误 CA、过期证书；
- debconf/preseed/noninteractive upgrade。

#### B. Enrollment → mTLS → Gateway CSR 分层探针

- 新 Client 仅生成 Device key/CSR；
- server-auth HTTPS 批准后只返回 Device leaf/chain；
- 用 Device cert 建立 QUIC mTLS；
- 服务器从 `peer_identity` 取得 Device；
- 在 mock active `SYNC_STATE` 上发送 Gateway CSR；
- Server 忽略 CSR SAN，按 mock target hostname 签发；
- 无 active command、错误 generation/configuration、匿名 QUIC、不同 SPKI 重试均拒绝；
- 0-RTT 关闭。

#### C. DOMjudge/Caddy

- loopback HTTPS/HTTP2；
- visual 503 page；
- X-Headers、Cookie、CSRF、redirect、Brotli transparent；
- Unix-socket Admin API。

#### D. Machine Identity

- 6 台物理硬件；
- placeholder、重复 serial、缺失字段、permission denied；
- configured-disk copy；
- fixtures 与预期 UUIDv5。

#### E. Session/Home

- desktop-only lock/unlock；
- 记录 Caddy Admin call count 为零；
- OverlayFS、Browser/IDE、ACL/xattr；
- staged-copy fallback；
- reboot 恢复。

#### F. Package/systemd

- clean install、upgrade、remove/purge；
- service ordering、D-Bus authorization；
- root key 文件权限；
- no runtime download。

---

### P0.7 XDG Autostart + Slint Session Agent 探针

P0.7 不创建第七份报告。它是 Probe E 的 **E1 XDG/Slint** 子部分；原有 Session lock/Home 验证作为 **E2**。在进入 Phase 6 前，必须用最小同一 Rust binary 证明：

- `/etc/xdg/autostart` entry 在 GNOME/GDM/Wayland 与 LightDM 启动的目标 X11 desktop 中直接启动唯一 `--autostart` 进程；具体 OS 版本和 X11 desktop 在 `supported-platform.md` 冻结前保持 `BLOCKED-INPUT`；
- package中不存在Session Agent systemd user unit、bootstrap/run双模式或环境descriptor；
- 同名user-level `Hidden=true`/replacement shadow在Home prepare时被固定路径清理或阻止；Agent lease超时保持Browser gated；
- Agent通过自身PID/logind识别UID/Class/Type/Active/Remote/Seat，拒绝greeter/TTY/SSH/inactive/错误用户/歧义session；
- Agent取得`$XDG_RUNTIME_DIR/natsume/session-agent.lock`，重复实例被拒绝；
- Agent进入Slint event loop时没有window/tray/splash，typed trigger才懒显示并可重新隐藏；
- `.slint`由`slint-build`编译，正式features只含backend-winit、renderer-skia、std/compat/accessibility；
- Qt backend、interpreter、live-preview、system tray、MCP/system-testing均未启用；
- 中文/ASCII、输入框、按钮、IME/paste、HiDPI、fractional scaling和multi-monitor基本行为可接受；
- Wayland拒绝focus时回报`presented_unfocused`，标准notification缺失时仍可用；
- Agent crash导致lease过期/Browser gated，受管relogin重启；Daemon不通过systemd user或伪造display环境spawn；
- 最终ELF/package不要求额外安装GTK、Qt、WebKit、Electron、Node、Python、JVM或外部GUI helper；
- Agent不能访问vault/Caddy Admin，Binding UI payload为typed message ID和参数。

探针产出明确支持矩阵，而不是笼统声明“支持LightDM”。LightDM只负责启动目标desktop；Agent与其没有IPC。

## 4. 三周实施顺序

### Week 1

- 建仓库、owner、lockfile、平台矩阵、实验室；
- 启动 package/systemd、IP-SAN、Caddy、Session、Home 探针；
- 冻结 requirement/Gate IDs；
- 建 CI skeleton 和 secret canary。

### Week 2

- 完成 SNAFU、OpenAPI/Proto/D-Bus codegen；
- 构建空壳 Deb 与 debconf/preseed；
- 完成两步证书生命周期探针；
- 建 Machine ID fixture；
- 完成 Server/Client vault cryptographic spike。

### Week 3

- 把所有检查变为真实 CI；
- 完成 clean install/upgrade/remove；
- 汇总六项 Probe A–F 结论与 ADR；Probe E 同时覆盖 E1 XDG/Slint 与 E2 Session lock/Home；
- 完成 G0 evidence bundle 与 Gate review。

---

## 5. 交付物

- `supported-platform.md` 与平台冻结记录；
- 六份 Probe A–F report；跨桌面 Session Agent 矩阵与 Slint 依赖闭包记录在 Probe E，不创建 Probe G；
- real CI workflows；
- real lockfiles/toolchain pins；
- 空壳 Server/Client Deb；
- debconf/preseed fixtures；
- Protocol/OpenAPI/D-Bus/SQL validation harness；
- 第一批 ADR；
- VM/physical lab inventory 与 fault scripts；
- G0 evidence bundle。

---

## 6. 验证矩阵

| 场景 | 预期结果 |
|---|---|
| Enrollment response 含 Gateway leaf | CI/schema test 失败 |
| 匿名 QUIC 连接 | TLS handshake 阶段拒绝 |
| 已认证 QUIC、无 active SYNC_STATE 的 Gateway CSR | 稳定错误码拒绝 |
| CSR 请求 SAN 与 target 不同 | CSR SAN 被忽略，证书使用 target SAN |
| 相同 request/SPKI 重试 | 返回同一结果 |
| 相同 request 不同 SPKI | conflict |
| 错误 Server IP/CA | HTTPS/QUIC 均拒绝 |
| Session Agent XDG Autostart direct launch | GNOME Wayland与LightDM/X11直接启动；初始无窗口；greeter/remote/ambiguous拒绝；shadow受控；Wayland unfocused可观察 |
| Agent runtime dependency closure | Slint feature闭包正确；无额外GUI toolkit/VM/WebView/helper运行时 |
| desktop lock/unlock | Caddy hash/epoch/Admin call count 不变 |
| image 中预置 root key/private key | package/image scan 失败 |

---

## 7. 风险与缓解

| 风险 | 缓解 |
|---|---|
| 目标 Desktop lock API 不可靠 | Phase 0 就决定支持矩阵或替代 Desktop，不拖到 Phase 6 |
| IP-SAN 部署复杂 | 探针失败时在 ADR 中改为固定 internal DNS；不得使用 TOFU |
| Gateway QUIC 子协议被做成通用签发接口 | Schema/authorization negative tests 和专用 message 命名 |
| OEM hardware evidence 不稳定 | 扩大 fixture、降低自动批准、收紧支持硬件 |
| OverlayFS 不兼容 | 在本阶段固定 staged-copy backend |
| CI 只验证结构不运行工具 | G0 不接受 placeholder 或“工具缺失即跳过” |

---

## 8. G0 Gate 清单

- [ ] clean checkout 的真实 CI 全绿；
- [ ] Client endpoint 交互/preseed 持久化与升级保留通过；
- [ ] Server IP SAN 正反测试通过；
- [ ] Enrollment 只签 Device cert 的 schema、DB fixture 与运行探针通过；
- [ ] Device cert → mandatory-mTLS QUIC → mock Gateway CSR 的两步探针通过；
- [ ] 匿名/错误上下文 Gateway CSR 被拒绝；
- [ ] XDG/logind direct Slint Agent探针通过GNOME Wayland与LightDM/X11；
- [ ] user-level同名Autostart shadow、singleton、crash/missing-lease Browser gate探针通过；
- [ ] greeter/remote/inactive/ambiguous拒绝与logout/relogin新进程/lease防护通过；
- [ ] Slint lazy GUI/focus-denied/HiDPI/IME与feature/依赖闭包通过；
- [ ] desktop lock/unlock 无 Caddy 调用；
- [ ] Machine ID physical fixture 完成；
- [ ] 空壳 Deb 的最终 topology 通过；
- [ ] 所有高风险 probe 有 ADR/owner/结论；
- [ ] Gate decision 已签署。

## 13. 已合并的上游 Phase 0 实现基线

本整合包以 `v2@dcbefb68035ab2fb1df74f5ddafa0ce7a181820c` 为代码基线，已经包含但不等于 Gate PASS：

- 23 个最小稳定 ErrorCode 的 `crates/error-code` registry；
- HTTP/Protocol/D-Bus 显式映射、`Redacted<T>`、`CodedReport` 与 source/path/secret 脱敏测试；
- Server、Daemon、Privileged Helper、Session Agent 的 compile-time ownership 接线；
- 真实 Cargo/pnpm lockfile、PR CI、nightly smoke 与 package content 检查；
- XDG Autostart package boundary，并移除 Session Agent systemd user unit。

仍保持 OPEN：目标 OS、GNOME/Wayland、LightDM 启动的 X11 desktop、真实 Slint/IME、Caddy、nFPM、重启与物理实验室 Gate。
