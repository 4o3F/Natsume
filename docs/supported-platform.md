# Natsume V2 支持平台与环境冻结

> 状态：`DRAFT-STEP0`
> 最后复核：2026-08-01
> Phase 0 窗口：2026-07-23 至 2026-08-19（v2.8 重基线）
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
| Client data plane | package-pinned Caddy，loopback HTTPS，`/login` xheaders 注入（[ADR-0034](adr/0034-state-execution-and-data-plane-boundary.md)） |
| Device control | WSS（server-auth TLS + Device Token）+ Protobuf，单 TCP 端口（[ADR-0033](adr/0033-enrollment-and-device-control-boundary.md)） |
| 签发模型 | provisioning 窗口内 Enrollment 签发 Token + Gateway cert（[ADR-0033](adr/0033-enrollment-and-device-control-boundary.md)） |
| Session Agent launch | system-level XDG Autostart，直接 resident process |
| Session Agent GUI | build-time Slint，winit backend + Skia renderer |
| Desktop 策略 | 每赛事周期单镜像/单桌面 + 镜像升级重验清单（[ADR-0035](adr/0035-session-home-and-desktop-cycle.md)）；当前周期 X11 |
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

全部 `ENV-UNFROZEN`。当前无目标环境或物理硬件 evidence。

| 项目 | 需要冻结的内容 |
|---|---|
| Server OS | **Ubuntu 26（官方 server 镜像）**（`ENV-PROPOSED`，见 §4.2，变更自 24.04 提案）；仍需精确 release/kernel/glibc/systemd 值与 clean install、reboot、包生命周期 evidence |
| Client OS（ICPC 派生镜像） | **Ubuntu 24.04.3 LTS** 派生镜像（`ENV-PROPOSED`，精确值见 §4.2）；镜像标识按 image build 日期；仍需 Deb 安装、Caddy 执行、D-Bus/logind、当期桌面 evidence |
| Server 网络 | Server 地址由**部署时配置**，不是仓库冻结的 IP literal：Server 自身监听 `0.0.0.0:8443`（包内配置），Client 端 endpoint 经 debconf 配置并在 `postinstall` 规范化校验；地址在同一部署内必须稳定，变更需重新签发带新 IP-SAN 的 TLS leaf 并重配 Client。Client 为 DHCP 短租期（ADR-0030 F3） |
| 时间同步 | Server 与全部 500 台工作站的时钟纪律：竞赛 LAN 上的 NTP source（或等效机制）与最大容许 skew（`ENV-UNFROZEN`，待部署证据）。静默依赖时钟的消费者：Command `deadline_at` 与 freshness 判定、Gateway 与 Server 证书的有效期窗口、TLS 有效期校验、UUIDv7 的时间序 |
| Operator 浏览器 | family/version、OS、分辨率/缩放、中文输入、安全 policy、更新窗口 |
| DOMjudge | 部署 snapshot 标识、upstream scheme/host/port/path；**`auth_methods` 含 `xheaders` 且登录契约验证**；**web server brotli 启用**；**upstream（至少 `/login`）TLS，材料由自签 origin CA 签发**；无健康检查端点，upstream 健康探测为被动（见 §4.2） |
| Desktop（当期单环境） | **Xfce + X11**（`ENV-PROPOSED`，见 §4.2，变更自 GNOME 提案）；仍需镜像升级重验清单全项（见下） |
| Slint runtime closure | 精确版本、features、动态链接库、font/IME、package size、cold start |
| Home backend | OverlayFS 与 staged-copy 二选一（均为 `ENV-PROPOSED`），按 safety、recovery 与 performance evidence 限时定案 |
| 物理硬件 fixture | v1 事故机器与代表性异构硬件的匿名化 fixture；全量实地验证在首次 provisioning 完成（[ADR-0032](adr/0032-device-identity-and-local-credential-lifecycle.md)，不再作为 G0 阻塞门禁） |

网络必须验证：正确 IP-SAN 通过、错误 IP/错误 CA/过期证书失败、单 TCP 端口防火墙与 NAT、DNS 不作为必需 fallback、preseed/upgrade 保留 endpoint、不使用 TOFU 或 dangerous verifier、Server 与工作站之间的 clock skew 在冻结容差之内。

**镜像升级重验清单**（每次镜像 bump 重跑，[ADR-0035](adr/0035-session-home-and-desktop-cycle.md)）：XDG Autostart 直接启动同一 binary、初始 resident + hidden、typed trigger 后 lazy Slint window、current logind session 识别、owner-only singleton、中文/IME、HiDPI、focus result 可观察、lock/unlock、terminate/replacement、display lost 与 crash recovery、无 systemd user unit、lock/unlock 的 Caddy 调用数为 0。核心应用依赖 capability，不直接依赖桌面名称；无法保证 focus 时报告 `VISIBLE_UNFOCUSED`，不加入 desktop-specific 强制聚焦 hack。

硬件 fixture 只保存匿名化候选、typed result 和 derived ID，不得保存原始 serial、private key、真实 password 或完整 Machine Hardware ID。fixture 必须覆盖 placeholder/缺失/permission denied/重复 source 与 configured-disk copy。

Natsume 不应在核心状态机中加入 DOMjudge 版本特例；适配差异留在 data-plane adapter 并通过 contract test 固定。

WSL、普通开发机、虚拟硬件 serial 或 reference scaffold 不得充当目标环境 evidence。

## 4.1 已提供的环境输入（2026-08-08）

以下由项目所有者提供，状态为 `ENV-PROPOSED`：已有具体候选，但仍缺目标环境上的可复现 evidence，因此不足以标记 `ENV-FROZEN`。

| 输入 | 提供值 | 仍需的 evidence |
|---|---|---|
| Server / Client OS | Ubuntu 24.04 | 精确 point release、architecture、kernel、glibc、systemd 版本；clean install、reboot、Deb install/upgrade/remove/purge |
| 当期桌面 | GNOME + X11（**已被 §4.2 变更为 Xfce + X11**） | 桌面版本、会话类型确认为 X11、镜像升级重验清单全项 |
| Server 地址 | **不是固定 literal，按部署配置** | 部署实际 endpoint、TLS leaf 的 IP-SAN 与之匹配、错误 IP/错误 CA/过期证书的负向结果 |
| Caddy | 由本项目自由选择，当前选定 2.11.4 标准发行版 | 目标 OS 上的执行与 package lifecycle（仓库层已验证，见下） |
| Operator 浏览器 TLS | 任意现代浏览器均支持 TLS 1.3 | UI 相关的 browser 事实随 Web Panel 阶段验证，非 Server 阻塞项（见下） |
| PKI | **两个根都自签**，最终部署同样使用自签 | 部署期实际签发与分发（见下） |

### Caddy：仓库层已固定并验证

`packaging/client/` 已固定并由 `just ci-packages` 实际下载校验：

| 项目 | 值 |
|---|---|
| version | `2.11.4` |
| source | Caddy 官方 GitHub release |
| archive SHA-256 | 见 `caddy.archive.sha256` |
| binary SHA-256 | 见 `caddy.sha256` |
| module closure | `caddy.modules` 的 12 项标准模块 |
| 非标准模块 | 断言为零（`list-modules --skip-standard` 必须为空） |

模块闭包**不含 `encode`**：`Accept-Encoding` 透传，brotli 在 upstream 完成（[ADR-0030](adr/0030-foundation-deployment-and-delivery-baseline.md) F5）。

因此 `G0-IN-003` 在仓库层已满足且可复现。剩余的只是在 Ubuntu 24.04 上执行，属目标环境验证而非输入缺口。

### Operator 浏览器：TLS 1.3 互操作不是阻塞项

Server 为 TLS 1.3-only 且 ALPN 仅含 `http/1.1`。TLS 1.3 自 2018 年起已在 Chrome、Firefox、Safari、Edge 的主线版本中默认启用，Ubuntu 24.04 随附的浏览器均满足；仅宣告 `http/1.1` 也不影响浏览器协商。Stage 3 已有真实 TLS 客户端测试覆盖 TLS 1.3 与 ALPN。

其余 browser 事实（family/version、分辨率/缩放、中文输入、安全 policy、更新窗口）只影响 Web Panel 的 UI 验证，随该阶段冻结，不构成 Server 的 G0 阻塞项。

### PKI：两个自签根的职责与生成要求

体系里有两个互不替代的根，均可自签：

| 根 | 证书分发位置 | 私钥保管 | 签发对象 |
|---|---|---|---|
| control root | `/etc/natsume/trust/control-ca.crt` | **离线介质**，运行中的 Server 不得持有（[安全与恢复](security-recovery.md) §2） | Server 的 TLS leaf |
| local origin CA | `/etc/natsume/trust/local-origin-ca.crt` | Server 私有状态目录（Enrollment 签发需要在线使用） | 各 Client 的 Gateway 证书 |

生成要求：

- control root 自签，私钥离线保存；用它签发 Server TLS leaf，**IP-SAN 必须等于该部署的实际 Server 地址**；leaf 与私钥按 [ADR-0037](adr/0037-operator-identity-and-server-runtime-secrets.md) 分别以 X.509 DER 与 PKCS#8 DER 放入 Server 私有状态目录，权限不宽于 `0600`、目录不宽于 `0700`；Server 只读取，不自签、不降级、不生成回退。
- local origin CA 自签；其证书随 Client 包分发并进入工作站浏览器信任库，使本机 Caddy 的 loopback HTTPS 可被信任。
- **测试材料无需额外输入**：`server/src/tls.rs` 的 `test_support` 已在运行时用 `rcgen` 生成自签 CA 与 leaf，Stage 3 的真实 TLS 测试即基于此。因此 `G0-IN-006` 的「test material」部分已满足。

尚未冻结的一项：local origin CA **私钥在 Server 上的存放路径**目前不在包内配置中（`[storage] root_key` 是 vault 主密钥，不是 origin CA key）。Phase 1 的 Server 不签发证书，因此这不是当前阻塞项；它是 Phase 3 Enrollment 启动时必须冻结的输入。

### Server 地址的架构后果

Server 地址可配置这一事实改变了 `G0-IN-002` 的性质：不再需要向仓库提供一个 IP literal。现行实现已经支持：

- Server 监听地址来自包内 `/etc/natsume-server/config.toml` 的 `[listen] https`，当前为 `0.0.0.0:8443`，即绑定所有接口、端口固定；
- Client 侧 endpoint 经 debconf（`natsume-client/server-ip`、`natsume-client/server-port`）配置，并在 `postinstall` 中规范化与校验；
- Server TLS leaf 由离线流程提供（[ADR-0037](adr/0037-operator-identity-and-server-runtime-secrets.md)），因此其 IP-SAN 必须以**该部署的实际地址**为输入生成。

由此产生一条必须记录的约束：地址在同一部署内必须保持稳定。更换 Server 地址不是纯配置操作——它使既有 TLS leaf 的 IP-SAN 失效，需要重新签发 leaf 并重新配置全部 Client。这不构成对"可配置"的否定，只是说明配置时机是部署期而非运行期。

### DOMjudge xheaders：协议契约已由官方文档确认

[DOMjudge 官方手册的 Advanced configuration topics](https://www.domjudge.org/docs/manual/main/config-advanced.html) 确认了本仓库既有的冻结契约，无需实验室即可确定协议层：

- 启用方式为在 `auth_methods` 配置中包含 `xheaders`；
- 两个 header 发往 `/login`：`X-DOMjudge-Login` 携带账户名，`X-DOMjudge-Pass` 携带 **base64 编码的密码**；
- 官方给出的用例是由受控代理注入 header，使参赛者无需知道自己的登录凭据。

这与 [ADR-0034](adr/0034-state-execution-and-data-plane-boundary.md) 已冻结的「Caddy 仅在 `/login` 注入 `X-DOMjudge-Login` 与 base64 `X-DOMjudge-Pass`、其他 route 不注入、upstream 必须 TLS」逐项一致。

**已核实的认证语义**（source：DOMjudge main 分支 `webapp/src/Security/DOMJudgeXHeadersAuthenticator.php`，核实日期 2026-08-14）：xheaders **不是** header 隐式信任。该 authenticator 只在 `loginmethod=xheaders` 的 `/login` POST 上触发，要求两个 header 同时存在，对 `X-DOMjudge-Pass` 做 base64 解码后构造 `new Passport(new UserBadge($username), new PasswordCredentials($password))`——密码经 Symfony 标准 password verification 与存储哈希比对，与普通表单登录同路径。因此能直接到达 DOMjudge 的客户端要冒充某账户，仍必须知道该账户的密码；本体系此处的安全依据是 [ADR-0030](adr/0030-foundation-deployment-and-delivery-baseline.md) T3（选手不知道自己的 DOMjudge 凭据），而不是"代理路径不可绕过"。

由此产生的 lab 验证项是**防版本漂移**而非封堵伪造面：必须确认**实际部署的 DOMjudge 版本**的 xheaders 仍保持上述 password-verifying 语义，不得假定成立。残余风险须记录：可直连 DOMjudge 的客户端可以对该 endpoint 进行在线密码猜测；缓解手段是由赛事组织方生成具备足够熵的 team password，属于部署输入而非 Natsume 机制。

因此 `G0-IN-004` 的 DOMjudge 部分收缩为**部署事实**而非协议事实，仍需实验室确认：DOMjudge 版本、`auth_methods` 实际已含 `xheaders`、web server 已启用 brotli、upstream（至少 `/login`）为 TLS 且信任链可验证、以及部署版本的 xheaders 仍执行 password verification。

### 硬件 fixture 集需要的具体内容（`G0-IN-005`）

fixture 的用途是证明 [ADR-0032](adr/0032-device-identity-and-local-credential-lifecycle.md) 的 normalization、placeholder 过滤、2-of-3 判定与 UUIDv5 派生在真实 fleet 上成立。fixture **不是**机器镜像，而是每台机器一条匿名化记录。

每条记录需要三个固定 source slot 的采集结果，顺序固定：

1. DMI system UUID
2. DMI motherboard serial
3. 第一块 system disk serial

MAC 地址明确排除，不要采集。

每个 slot 需要：

| 字段 | 取值 | 说明 |
|---|---|---|
| `anchor_kind` | 固定 slot 标识 | 三者之一 |
| `status` | `present` / `unavailable` / `unsupported` / `permission_denied` / `malformed` / `rejected_placeholder` / `conflict` | 采集结果分类，对应 `EvidenceStatus` |
| `quality` | `strong` / `medium` / `weak` | 对应 `EvidenceQuality` |
| `candidate_id` | 该 slot 规范化值的 UUID 派生结果 | **不提交原始 serial** |

整机再加一个 `completeness`：`complete` / `temporarily_unavailable` / `unsupported`，对应 `CollectionCompleteness`。

**禁止提交**：原始 DMI serial、原始磁盘 serial、完整 Machine Hardware ID、private key、真实密码。只提交 slot 分类、quality 与派生候选 UUID。

fixture 集必须覆盖下列场景，每种至少一例；这是 ADR-0032 的证据要求，缺项则该项决策仍未被真实硬件证明：

| 场景 | 为什么需要 |
|---|---|
| 三个 slot 全部有效 | 正常路径 |
| 恰好两个有效（三种缺失组合各一） | 2-of-3 判定的可行性 |
| 仅一个有效 | 必须 fail closed，不得猜测 |
| 全部无效 | 必须 fail closed |
| placeholder 值（全零、全 `F`、厂商占位串） | placeholder 过滤规则 |
| 大小写/空白/分隔符不一致 | normalization 规则 |
| `permission_denied` | 非 root 采集路径的 fail-closed |
| **configured-disk copy**：同一磁盘镜像克隆到不同主板 | 必须判定为不同机器，这是 v1 事故的核心场景 |
| **v1 事故机器**：v1 曾误判为同一台的实机 | 回归证据，必须复现为正确判定 |
| 代表性异构：不同厂商/型号/固件的至少三台 | 证明规则不是只对单一批次成立 |

数量上限没有硬规定，但少于上表场景数即无法覆盖。若某场景在真实 fleet 中不存在（例如没有 `unsupported` 的机型），应显式说明"该场景在本 fleet 不可得"，而不是用构造数据填充——构造数据不能作为实地 evidence。

## 4.2 环境输入补充与变更（2026-08-14）

以下由项目所有者提供，状态为 `ENV-PROPOSED`（值已具体，仍缺目标环境上的可复现 evidence）：

| 输入 | 提供值 | 假设与后续事项 |
|---|---|---|
| Client OS 精确值 | Ubuntu 24.04.3 LTS、kernel `6.14.0-29-generic`、`x86_64`、glibc 2.39（`2.39-0ubuntu8.5`）、systemd 255（`255.4-1ubuntu8.10`） | 生命周期 evidence 待条目 12 在该镜像上执行 |
| Client 镜像标识 | 按 image build 日期锚定 | 每次镜像 bump 触发重验清单 |
| Server OS | 官方 Ubuntu 26 server 镜像（精确 release/kernel/glibc/systemd 待补），**变更自 2026-08-08 的 24.04 提案** | 所有者假设 24.04 构建产物可向前兼容运行；该假设不替代 evidence——Server 侧生命周期证据最终须在该镜像上产出。后续对齐事项：CI runner 与 packaging smoke 当前为 ubuntu-24.04 |
| 当期桌面 | **Xfce + X11**（`XDG_SESSION_TYPE=x11` 已确认），**变更自 2026-08-08 的 GNOME 提案** | lock API 待定：Xfce 下建议以 logind lock-session 为准，Phase 6 限时定案（ADR-0030 F4 已同步修订） |
| DOMjudge 版本策略 | 部署恒为最新 main 分支 snapshot | 所有者断言 xheaders 契约跨版本稳定；条目 9 lab 结论必须登记测试时的实际 snapshot 标识，更换 snapshot 触发 xheaders 语义复核。upstream 当前不可访问，lab 待其可得 |
| DOMjudge upstream TLS | **必须 TLS**，证书由自签 origin CA（`LOCAL_ORIGIN_CA`）签发 | 维持 ADR-0034 fixed-TLS-upstream 不变量与 G5「非 TLS 拒绝激活」验收，不引入可信链路豁免 |
| DOMjudge 健康检查 | 无专用端点，Natsume 不做主动探测 | `GatewayState` 的 `upstream_unhealthy` 只由被动信号（代理错误）驱动 |
| 时间同步 | Client 部署时与 Server 做 NTP 校准 | 持续 skew 容差仍 `ENV-UNFROZEN`；时钟消费者（`deadline_at`、证书有效期、UUIDv7 序）在容差冻结前不得假设长期同步 |
| Operator 浏览器 | 所有者豁免其余 freeze 字段 | 相关事实随 Web Panel 阶段验证，不作为 G0 输入 |
| 硬件 fixture | 当前不可采集，`G0-IN-005` 维持 `BLOCKED-INPUT` | 前提澄清：Machine Hardware ID 持久化为身份锚点（`devices.machine_hardware_id` UNIQUE 与 Enrollment 绑定），上线后变更派生逻辑的代价是全 fleet 重新注册；fixture 须在首次 provisioning 前完成采集 |
| Slint runtime closure（实测） | Slint `1.15.1`；features `compat-1-2`/`std`/`backend-winit-x11`/`renderer-skia`；直接 ELF NEEDED 冻结于 `packaging/client/session-agent.needed` 且 target VM `ldd` 全表吻合；二进制 11,734,952 字节；冷启动至 resident marker 59 ms（图形会话内实测） | CJK **渲染**在各屏形态正常；**当期镜像不带中文输入法，IME 输入项未验证**——镜像加装中文 IME 后复验。probe 各屏（binding_prompt/lock_presentation 等）、HiDPI 缩放与焦点观察均通过 |
| V1 残留 | 仅测试 VM 曾装过 V1；fleet 机器无 V1 部署残留 | V1 与 V2 共用 `/etc/natsume/config.toml` 路径，残留会改变 dpkg 安装分支（conffile 提示、`.dpkg-old`）；lifecycle harness 已加干净 VM 守卫，检测到残留即拒绝 |

## 5. 支持声明

在 G0 通过前：

- 不发布"支持某发行版/桌面/浏览器"的产品声明；
- 只能称为候选目标；
- repo pin 不等于环境支持；
- VM 证据不替代物理 Machine ID fixture；
- 文档 scaffold 不等于 GUI 实现；
- package build 成功不等于 lifecycle 签收。

## 6. 当前输入门禁

| ID | 输入 | 目标 | 状态 |
|---|---|---|---|
| `G0-IN-001` | Server/Client OS、architecture、systemd | P0 收尾 | `ENV-PROPOSED`：Client 精确值已提供（§4.2）；Server 变更为 Ubuntu 26 官方镜像，精确值待补；两侧 lifecycle evidence 待目标环境执行 |
| `G0-IN-002` | Server endpoint 与单 TCP 端口 | P0 收尾 | `RESOLVED`（性质变更）：地址按部署配置，不需要仓库 IP literal；端口固定 `8443`。剩余部分并入目标环境验证（§4.1） |
| `G0-IN-003` | Caddy version/modules/source/checksum | P0 收尾 | `RESOLVED`：2.11.4 标准发行版已固定并由 `just ci-packages` 校验；剩余为目标 OS 执行（§4.1） |
| `G0-IN-004` | Browser（含 TLS 1.3 互操作证据）、DOMjudge（xheaders/brotli/TLS）、当期桌面、XDG、Slint、lock API | P0 收尾 | 大部分推进：桌面 Xfce + X11（`ENV-PROPOSED`，§4.2）；xheaders 协议契约由官方文档确认、认证语义由上游源码核实为 password-verifying；Browser 由所有者豁免至 Web Panel 阶段；upstream TLS 与版本策略已定（§4.2）；Slint runtime closure 已实测（§4.2）。剩余为 DOMjudge lab（upstream 不可访问）、lock API 定案，及新发现缺口：当期镜像缺中文输入法（IME 项待镜像加装后复验） |
| `G0-IN-005` | 硬件 fixture 集（v1 事故 + 代表性异构） | P0 收尾 | `BLOCKED-INPUT`：所需字段与场景清单已在 §4.1 明确，等待实地采集 |
| `G0-IN-006` | PKI test material（control CA / origin CA）与 owner | P0 收尾 | `RESOLVED`：两根均自签；test material 由 `rcgen` 运行时生成，已被 Stage 3 TLS 测试消费。Phase 3 需另行冻结 origin CA 私钥路径（§4.1） |
| `G0-IN-007` | v2.8 文档/ID/术语签收 | Step 0 | `RESOLVED`（2026-08-14）：见 [`gates/phase-0-status.md`](gates/phase-0-status.md) 的签收记录 |

Gate 状态以 [`gates/phase-0-status.md`](gates/phase-0-status.md) 为准。
