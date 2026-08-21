# Phase 5 执行计划：State, Data Plane & Secrets

> 状态：`DRAFT-PLAN`（2026-08-16 起草）
> 适用：Phase 5 启动时提升为 `docs/gates/phase-5-status.md` 的启动分解基线，届时按最新事实修订
> 权威来源：[路线图](../roadmap.md) §Phase 5 与 G5 覆盖、[契约](../contracts.md) §7/§8/§9/§11/§12、[状态与执行模型](../state-and-execution.md) §3–§6、[ADR-0034](../adr/0034-state-execution-and-data-plane-boundary.md)
> 前置：Phase 4 全部 WP 关闭；并先关闭 ADR-0038 flag-day sequencing blocker（见 E4）

**2026-08-19 blocker**：本计划按当前 Token Enrollment、daemon credential paths 与 Bearer WSS authority 起草；[ADR-0038](../adr/0038-unified-ordinary-wss-device-control-authority.md) 的原位 Proto/crypto/schema foundation 已存在，但 runtime cutover 尚未发生。Owner 必须先决定 Phase 5 位于 atomic authority cutover 前还是后，并据此重基线 credential/session inputs；禁止实现混合 Token/control-key compatibility。

**2026-08-20 修订（Command 投递二分）**：`SYNC_STATE` / `SYNC_SECRET` 是 Converge 命令（键分别为 `canonical_hash` vs `applied_hash`、`credential_revision` vs `installed_credential_revision`）。Device 无 command journal；D12 journal GC 关闭为不再适用。Observed 为 slim snapshot（`credential_state`，无 `secret_state` / `STALE`）。`SyncState` 字段为 `canonical_hash`、`binding_id`、`seat_code`、`domjudge_username`，无 `generation`。`open_binding_prompt` 空 body，无 TTL，无 `prompt_message_id`；Seat 在 `BindingRequest.seat_code`（Phase 6 消费）。

本文件是计划，不是完成声明。遵守 [路线图](../roadmap.md) §1 原则 5：细目在 Phase 启动时冻结，本文件提供该冻结的候选基线与决策清单。

## 1. 阶段目标与边界

**结果**：operator 显式触发的 `SYNC_STATE` 把 Server truth 派生的 Target 安全落到 Device 的 Caddy 数据面（BLOCKED → READY），`SYNC_SECRET` 把 Seat 凭据安全落盘并驱动 `/login` 注入配置重渲染，Drift 视图让 operator 看到 Target 与 Observed 的纯比较结果。

**非目标**：Session Agent、Home 事务、桌面 lock（Phase 6）；发布、备份、审计导出（Phase 7）；任何自动触发的 Command（[状态与执行模型](../state-and-execution.md) §3/§5 明令二者只能由人显式触发，不得由 Drift 自动触发）。

## 2. 入场检查（Phase 5 启动前）

| # | 检查项 | 依据 | 阻塞范围 |
|---|---|---|---|
| E1 | DOMjudge lab 实访复核：snapshot 标识、`auth_methods` 实含 xheaders、brotli 已启用、upstream `/login` TLS 且链可验、该版本仍执行 password verification | G0 条目 9 的 owner 裁定把实访移出 G0 并指定为 Phase 5 入场检查项 | 阻塞 WP5；WP1–WP4 可先行 |
| E2 | Phase 4 WP5（Device WSS client / Observed）与 WP6（缩比容量探针）关闭 | Device 侧无投递通道则 WP4/WP6 无从执行 | 阻塞全部 Device 侧 WP |
| E3 | 时钟 skew 容差冻结（当前 `ENV-UNFROZEN`） | [支持平台](../supported-platform.md) 时钟纪律；Target freshness 与证书窗口静默依赖 | 阻塞 WP4 freshness 判定 |
| E4 | 冻结 ADR-0038 atomic cutover 与 Phase 5 的先后；禁止 Token/key 或 old/new registry 混合路径 | ADR-0038 flag-day约束 | 阻塞本计划提升为Gate status及WP1/WP4/WP6编码 |

## 3. 已冻结事实（不得重新设计）

### 3.1 Caddy 控制路径（[契约](../contracts.md) §11、ADR-0034）

```text
已验证 Target + 本地证书/凭据材料
  → Daemon 渲染完整配置文件（固定 loopback listen、固定 hostname、固定 DOMjudge upstream、
     固定 TLS material 引用、固定 BLOCKED/READY route 集、仅 /login 的 header 注入）
  → caddy validate
  → 原子替换配置文件（temp + fsync + rename）
  → systemd path unit 触发 reload
  → 本地健康检查；失败回滚 LKG 配置文件
```

禁止：任意 Caddyfile、Daemon 使用 Caddy Admin API、未验证配置激活、本地 `encode`（brotli 属 upstream，ADR-0030 F5）。激活前必须验证证书/私钥匹配与 SAN/有效期。

### 3.2 BLOCKED / READY 二态

业务状态只有两个。BLOCKED：包内本地资源 503、不代理 DOMjudge、只显示 allowlisted typed state、严格 CSP、动态值只经 `textContent`，**不得暴露 secret / 路径 / 自由格式错误 / source chain / `session_locked`**。READY 需同时证明：当前 Target/revision、Gateway key/cert 匹配且 SAN 与有效期合格、`caddy validate` 通过、reload 成功、本地健康检查通过、固定 TLS upstream policy、LKG 写入成功或可恢复。**Enrollment 成功不得展示为数据面 ready**（证书持有与数据面状态是两个独立维度）。

### 3.3 xheaders `/login` 注入与 upstream

只在 `/login` 注入 `X-DOMjudge-Login` 与 base64 的 `X-DOMjudge-Pass`，其他 route 零注入；upstream 必须 TLS（origin CA 签发），非 TLS 时不得激活注入配置（`INV-DATAPLANE-02`）。上游语义已核实为 **password-verifying**（不是 header 隐式信任），安全依据是 ADR-0030 T3（选手不知道自己的凭据）。DOMjudge 无健康端点，Natsume 不主动探测，`upstream_unhealthy` 只由被动信号（代理错误）驱动。

### 3.4 秘密 artifact 属主（ADR-0034 的 2026-08-16 修订）

credential source `0600 natsume:natsume`，rendered Caddy secret artifact `0640 natsume:natsume-gateway`（原 `root:root`/`root:natsume-gateway` 已与 ADR-0032 同日调和为 service-user 所有）。禁入清单不变：不得进入 Target、Observed、audit diff、metrics、普通日志或 Session Agent。

### 3.5 Phase 4 已交付、本阶段直接消费

- **payload schema v1 已实现**（`server/src/application/command.rs`）：HTTP frozen payload 仍是 Phase 4 交付面。wire `SyncState` 为 `{canonical_hash, binding_id, seat_code, domjudge_username}`，无 `generation`；`SyncSecret` 为 `{binding_id, credential_revision, password}`。Converge 键是 `canonical_hash` / `credential_revision`；occupancy 是 `binding_id` UUIDv7。`generation` 不进入 `SyncState` 或 Observed。无 `TargetAssignment` / `TargetGateway` 消息。
- `sync_secret` 的 vault→wire `SecretBytes` 注入被 Phase 4 显式登记为 **Phase 5 接线 hook**（Phase 4 接受并持久化但 dispatcher 不渲染不投递）。
- proto `SecretBytes` 的 `Debug` 经 build 期 `skip_debug` 手写为 `[REDACTED]`。

### 3.6 打包既有资产

| 资产 | 现状 |
|---|---|
| `natsume-caddy.service` | `Type=notify`、`User=natsume-caddy` + `SupplementaryGroups=natsume-gateway`、三条 `ConditionPathExists`（`/run/natsume/gateway-tls/{ready,fullchain.pem,key.pem}`）、`ExecStart`/`ExecReload` 指向 `bootstrap.caddyfile`、`ProtectSystem=strict`、`ReadOnlyPaths` 含 gateway-tls 与两个 gateway-status 目录、`LimitCORE=0`、**无 `[Install]` 段** |
| `natsume-caddy.path` | `PathExists=/run/natsume/gateway-tls/ready` → `Unit=natsume-caddy.service`（**启动**触发，非 reload） |
| `bootstrap.caddyfile` | 仅 BLOCKED 形态：loopback bind、`auto_https off`、`admin unix//run/natsume/caddy-admin.sock`、`persist_config off`、状态 JSON route、静态资源 route、`file_server { status 503 }` 兜底、严格 CSP；**无 upstream、无 `/login` 注入、无 READY 路由集**；hostname 为占位 `contest.natsume.test` |
| BLOCKED 状态页 | **已就绪可复用**：`index.html` 无 inline script/style；`status.js` 每 2s 轮询、全部 DOM 写入经 `textContent`、state 与 action 走 `hasOwnProperty` allowlist、`fetch` 用 `no-store`+`credentials:"omit"`、非 OK 走 `renderUnavailable()`；CI 断言 `node --check`、CSP、`status 503`、`session_locked` 禁入 |
| tmpfiles | `/run/natsume` 0770 natsume:natsume-gateway；`gateway-tls` 与 `gateway-status` 均 0750 |
| Caddy 模块闭包 | `caddy.modules` 12 项标准模块、**不含 `encode`**；CI 断言 required 超集 + `list-modules --skip-standard` 为空 |

## 4. 工作包分解（候选基线）

依赖序：WP1 → WP2 → WP3 → WP4 → WP5 → WP6 → WP7；WP0 可与 WP1 并行。

### WP0：状态页数据面（gateway-status.json 生产者）

**缺口**：`status.js` 轮询 `/.well-known/natsume/gateway-status.json`，`bootstrap.caddyfile` 从 `/run/natsume/gateway-status` 提供该文件，**但仓库中没有任何进程写它**；`GatewayStatusSnapshot` 类型已在 `local-control-api` 定义却不在任何 D-Bus interface 上。

- 目标：Daemon 原子写状态 JSON，使 BLOCKED 页面显示真实 typed 状态。
- 冻结项：文件式 vs D-Bus 式（建议文件式——页面已按文件轮询设计）；`schema_version`；写入节奏（状态变化即写 + 低频兜底）；三套并行状态词表（proto `GatewayState` 7 值、`local-control-api` 的 `GatewayBlockReason`/`GatewayReasonCode`/`SuggestedAction`、`status.js` 的 6 状态 4 动作）的**权威源与映射表**（决策点 D7）。
- 测试：JSON 字段 allowlist 断言；secret/path/source-chain 零出现的字节扫描；`status.js` 的 allowlist 与生产者取值集合一致性测试（防止新状态在页面回落成 `recovery_required`）。

### WP1：Gateway 凭据物化（DER → PEM）

**缺口（Phase 5 第一必补项）**：Enrollment 落盘 DER（`/var/lib/natsume/keys/gateway-{leaf,chain}.der`、`gateway-key.pk8`），Caddy 期望 PEM（`/run/natsume/gateway-tls/{fullchain.pem,key.pem}` + `ready`）。**无任何代码物化**，而三条 `ConditionPathExists` 与 path unit 已依赖它们。

- 目标：Enrolled 驻留后与每次凭据轮换后，把 DER 材料物化为 PEM 并以 `ready` 标记收尾。
- 冻结项：写入顺序（key → fullchain → ready，`ready` 为最后的原子标记）；`fullchain.pem` = leaf 后接 chain；权限 `0640 natsume:natsume-gateway`；轮换语义（`ready` 删除→重建 vs 保持 + 触发 reload，与 D3 一并定案）；私钥零化与 `LimitCORE=0` 一致。
- 文件面：`client/device-daemon/src/gateway_material.rs`（新）、`startup.rs` 接线、复用 `atomic_write`。
- 测试：DER→PEM 与 rustls/openssl 交叉验证；leaf↔key SPKI 匹配先于写入；chain 逐字节等于 packaged origin root；权限/属主断言；`ready` 后于两个 PEM 的顺序断言（崩溃点注入）；轮换后 Caddy 仍可加载。

### WP2：Target 派生与 `SYNC_STATE` 服务端

- 目标：从 Server truth 确定性派生非秘密 assignment，产出 `canonical_hash`，作为 `sync_state` 的 frozen payload；提供 operator 触发面。wire 不携带 `generation`。
- 冻结项：派生输入闭包（Seat↔Binding、account mapping、site config；无 `revision_counters`）；`canonical_hash` 算法（建议与 fingerprint v1 同纪律：独立域分隔符 + NUL + JCS，**D1**）；`generation` 不进入 `SyncState` / Observed（**D2** 关闭为不再适用）；operator 触发面（新 route vs 直接 `putCommand`，**D3**）。
- 文件面：`server/src/application/target.rs`（新）、必要的 `db/` 读面、`http/handler/`、OpenAPI（若新增 route 需同步 §3.6.1/§3.6.2/§3.6.5 与审计词表）。
- 测试：同一 truth 多次派生逐字节一致；任一输入变化改变 hash；secret 不入 Target 的字节扫描；陈旧 baseline 拒绝。

### WP3：Caddy 渲染 / validate / reload / LKG（Device 侧）

- 目标：实现 §3.1 五步管线与 BLOCKED/READY 两形态模板、LKG 回滚。
- 冻结项（本包决策最重）：
  - **渲染产物与 LKG 的文件系统契约**：`/etc/natsume/caddy/` 经通用目录树复制落地（`bootstrap.caddyfile` **不是 conffile**），渲染产物落同目录会与包管理冲突；须定路径、权限、LKG 保留代数（**D4**）。
  - **reload 触发机制**：`PathExists` 在文件持续存在时不重复触发，无法表达 reload-on-change。候选：(i) 改 `PathChanged`/`PathModified` 指向渲染产物 + reload-only 单元；(ii) Daemon 经 privileged helper 调 `systemctl reload natsume-caddy`；(iii) `.path` 只做首次激活、reload 走 (ii)。**D5**（倾向 iii）。
  - **admin unix socket 定位**：契约禁止的是 **Daemon** 使用 Admin API，而 `ExecReload` 的 `caddy reload` 正走该 socket。冻结表述建议：socket 仅 caddy 服务用户可访问、**只被 `ExecReload` 使用**、Daemon 永不直连；需契约 §11 dated revision（**D6**）。
  - **bootstrap hostname**：占位 `contest.natsume.test` vs 按 site `gateway_hostname` 渲染（占位会导致本机浏览器证书名不匹配，**D8**）。
  - **本地健康检查定义**：[状态执行模型](../state-and-execution.md) §3 留了未解的「保持 BLOCKED **或** READY-with-health」；须定探测对象（loopback 自身 / upstream TCP·TLS 握手）、阈值、失败后状态归属（**D9**）。
  - **模块闭包补登记**：upstream TLS 校验需 `http.reverse_proxy.transport.http` 与 trusted-CA 配置，`caddy.modules` 12 项未登记 transport 模块；断言是超集故不会失败，但闭包文档会与实际使用面脱节（**D10**）。
- 文件面：`client/device-daemon/src/caddy/{render,activate,lkg}.rs`、privileged-helper 新 typed 方法（若采纳 ii/iii）、`local-control-api` wire 类型扩展。
- 测试：BLOCKED/READY 渲染 golden；validate 失败零激活；reload 失败回滚 LKG 且旧配置仍有效；证书/私钥不匹配、SAN 错误、过期证书三类拒绝；配置权限 `0640 natsume:natsume-gateway`；`encode` 缺席断言；health 失败进 BLOCKED。

### WP4：`SYNC_STATE` Device 执行

- 目标：收到 Command → 校验（target device、`canonical_hash`、freshness、本地 identity、hostname/upstream 属允许集合）→ 调 WP3 管线 → CommandStatus 与 Observed 回报。wire `SyncState` 无 `generation`。
- 冻结项：freshness 时钟容差（依赖 E3）；允许集合来源（site.toml + packaged profile ID 映射）；失败到 `COMMAND_STALE` / `COMMAND_PAYLOAD_INVALID` / `GATEWAY_CREDENTIAL_INVALID` / `GATEWAY_ACTIVATION_FAILED` / `GATEWAY_UPSTREAM_TLS_REQUIRED` 的映射表。
- 测试：全部拒绝路径零副作用；成功路径 Observed 的 `applied_hash`/`gateway_state` 精确；同一 `canonical_hash` 重推为 no-op（无 Device journal）。

### WP5：xheaders `/login` 注入与 upstream policy

- 目标：READY 配置仅 `/login` 注入；upstream 固定且必须 TLS；`Accept-Encoding` 透传。
- 冻结项：注入头名与值构造（依赖入场检查 E1 的实访结论与 snapshot 标识登记）；`/login` 之外零注入的结构性保证；upstream 非 TLS 的拒绝激活路径。
- 测试：穷举 route 集断言注入头只出现在 `/login`；upstream 明文拒绝激活；不配置 `encode` 且 brotli 响应透传；DOMjudge 契约回归（lab fixture 或录制响应）。

### WP6：`SYNC_SECRET` 全链

- 目标：operator 触发 → Server 渲染 wire Command 时从 vault 取秘密注入 `SecretBytes` → Device 重检 binding/credential revision → 原子写凭据文件 → 重渲染含凭据配置并激活 → 只报 installed revision。
- 冻结项：秘密注入发生在**渲染 wire Command 时**（绝不进 `frozen_payload_json`，Phase 4 已冻结）；Device 凭据文件路径与权限；重投递时每次重新取秘密而非缓存；零秘密断言点清单。
- 测试：password 明文不入 DB/WAL/日志/审计/Observed/metrics 的字节扫描；陈旧 revision 拒绝安装；同一 `credential_revision` 重推为 no-op；写入中断保留旧 secret 或明确标记不可用；成功后配置含凭据且权限正确。

### WP7：Drift 与 operator 视图

- 目标：`compare(Target, latest valid Observed)` 的纯比较（可重算、非独立 truth）+ Panel 呈现；Observed 陈旧/缺失呈现为 unknown 而非 READY。
- 冻结项：**新 HTTP 面须先冻结**——OpenAPI 的 declared-but-unmounted 集合现已为空，Drift/Observed/Command 查询 route 需新写入契约 §3.6.1/§3.6.2/§3.6.5 并按「先注册后写入器」补审计词表（**D11**）；Drift 维度清单；Observed freshness 阈值；前端一律 shadcn 组件。
- 测试：Drift 纯函数性质（同输入同输出、零副作用）；unknown 三态（无 Observed / 陈旧 / schema 不符）；Playwright 场景。

## 5. G5 覆盖项 → WP 映射

| G5 主题 | WP |
|---|---|
| 两段签发阶梯负向 | WP1 + 既有 `INV-CERT-01` 回归 |
| Caddy validate/reload/rollback | WP3 |
| bad cert/key/SAN 拒绝 | WP1 + WP3 |
| offline LKG | WP3 |
| upstream 非 TLS 拒绝激活 | WP5 |
| `/login` 之外无注入头 | WP5 |
| secret stale/retry/redaction | WP6 |
| DOMjudge 契约回归 | WP5（依赖 E1） |
| 故障注入 | WP3/WP4/WP6 的中断点注入 |

## 6. owner 决策点

| # | 决策 | 影响 |
|---|---|---|
| D1 | `canonical_hash` 算法（建议同 fingerprint v1 纪律，独立域串） | 跨 Server/Device 一致性 |
| D2 | `generation` 不进入 `SyncState` / Observed（已关闭） | wire 无该字段 |
| D3 | `SYNC_STATE` operator 触发面：新 route vs `putCommand` | OpenAPI 与 Panel 交互 |
| D4 | 渲染产物与 LKG 路径、权限、保留代数 | 与包管理目录冲突风险 |
| D5 | Caddy reload 触发机制三选一 | systemd 拓扑与 helper 面 |
| D6 | admin unix socket 契约定位（仅 ExecReload） | 需契约 §11 dated revision |
| D7 | 三套状态词表的权威源与映射 | 状态页与 Observed 一致性 |
| D8 | bootstrap hostname：占位 vs 站点渲染 | 浏览器证书名匹配 |
| D9 | 本地健康检查定义与 upstream 不健康时的状态归属 | READY 判据 |
| D10 | `caddy.modules` 是否补登记 transport 模块 | 闭包文档与实际使用面一致 |
| D11 | Drift/Observed 查询 route 的契约冻结 | 新 wire surface |
| D12 | Device journal GC（已关闭，见 §6.1） | 不再适用：无 Device command journal |

### 6.1 D12：journal GC（2026-08-20 关闭为不再适用）

**关闭理由**：七种 Command 不是同一套 Device journal 耐久机。Converge 按领域键幂等，Oneshot 仅 live socket；Device **不**维护 command journal，因此不存在 journal GC / 终态确认通道问题。Phase 4 删除的高水位游标（`devices.terminal_result_cursor`）保持删除，不再设计替代确认机制。

**历史背景（不再实施）**：曾设想 Device journal 保存 Command frame bytes，条目删除依赖服务端终态确认。高水位游标因乱序终态会静默丢结果而被否决。

高水位游标、按命令确认帧、首批投递完成标记均不再评估。Oneshot 离线丢弃；Converge 靠领域键重推。

## 7. 跨切风险

| 风险 | 控制 |
|---|---|
| DOMjudge lab 结论晚到导致 WP5 返工 | E1 阻塞 WP5；WP0–WP4 先行 |
| reload 机制选错导致 systemd 拓扑返工 | D5/D6 先于 WP3 编码定案 |
| 秘密泄漏面扩大（渲染配置含凭据） | 每包附字节扫描；配置权限 CI 断言 |
| Observed 与 SYNC_STATE 竞争 SQLite 单写者 | E2 缩比探针；WP4 后复跑 |
| 新增 HTTP 面绕过契约纪律 | D11 先冻结再实现；审计词表先注册 |

## 8. 已否决的备选方案（勿重新翻案）

**把 payload 校验约束放进 proto（custom options）并从描述符全量 codegen，或改用「描述符驱动的通用 walker + 字段策略表」替换手写校验结构体**——2026-08-16 评估后否决。

理由：(a) **治理周期不匹配**——`.proto` 是受 §13 兼容性规则与 golden descriptor 治理的冻结 wire 契约，而校验策略是服务端可自由调整的策略，把后者塞进前者会让每次策略微调都触碰受管契约；(b) **真源本身几乎不动**，「自动同步」的边际收益很低；(c) 代价确切——operator 输入这一安全边界从编译期类型降级为测试期断言，12 个独立校验器（单点故障只影响一个 kind）合并成通用 walker（一处出错七个 kind 全漏），并需手写严格反序列化才能补回 serde derive 白送的重复键拒绝；(d) **现有防护已足够**：`protocol_contract.rs` 的描述符 golden 使任何 proto 改动立刻红灯，`render.rs` 的穷举结构体字面量使增删字段编译失败，未覆盖的仅剩「改 proto + 同步 golden 却漏改 JSON schema」，而那一刻本就在评审一次受管 wire 变更。

维持现状：proto 管 wire 形状，服务端手写结构体管入参校验，`render.rs` 管两者间的类型化映射。

## 9. 无归属挂账认领（Phase 5 一并处置）

`apply_lifecycle_mutations` 与 Device revoke/disable 的模块迁移已由 Device-first Batch 5 闭环：lifecycle 编排及其 DB、HTTP、audit seam 均归 `device`，不再是无归属挂账。

| 项 | 来源 | 建议 |
|---|---|---|
| canonical UUIDv7 variant nibble guard | 登记「归 Phase 3+」，Phase 3 已 CLOSED | 随 WP2 的 ID 校验一并硬化 |
| `record_type` 封闭枚举的 DB 强制 | 登记「归 Phase 2」，Phase 2 已 CLOSED | **2026-08-20 关闭为不再适用**：vault 已删除 `record_type`；勿补 CHECK |
| Web 深链保留、首页落点 | Phase 1/2 登记「随 Panel 页面增多再定」 | WP7 触发，一并定案 |
| `GET /api/v2/imports` 轮询取写锁 | Phase 2 登记 | 若 operator tab 数上升，随 WP7 改先 deferred 读 |
