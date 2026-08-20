# Natsume V2 边界契约

> 状态：`NORMATIVE`  
> 适用范围：HTTP、Enrollment、WSS Device control、Protobuf、D-Bus、CommandStatus 和公开错误  
> 机器结构权威源：OpenAPI、Protobuf descriptor、D-Bus introspection、SQL migration 和生成代码

## 0. 当前实现与已接受目标

**当前 authority**继续使用公开 HTTPS Enrollment、Device Token 与 Bearer-before-101。Production Proto 已按预发布 BC 原位拆为六个文件，单一 package 为 `natsume.device.control`，双端 subprotocol 为 `natsume.control`；当前 socket 仍只消费既有 `ControlEnvelope`。Migration 已包含 dormant control-key/bundle/transitional columns，但 Token application 不读写它们。

**已接受目标**由 [ADR-0038](adr/0038-unified-ordinary-wss-device-control-authority.md) 定义：普通 WSS 内的 standalone Challenge/Proof/ClientInit、Ed25519 control key、动态 DeviceActor 与同 socket CredentialAck activation。对应 generated messages、Prost decode/semantic validation、typed canonical re-encoding 与 transcript foundation 已存在，但尚无 runtime authority consumer。

项目预发布阶段允许原位 breaking change，不建立 `control_v2`、第二 descriptor 或旧/新 package 兼容层。只有 atomic authority flag day 可同时删除 Token/public Enrollment HTTP、启用新 Server/daemon 状态机并收紧本文件现行认证条款；在此之前禁止混合 Token/key authority。

本文件定义稳定语义，不复制完整 schema。字段名、编号和路由必须从机器 schema 生成或验证。未实现行为的完整字段表与 wire 结构延迟到对应 Phase 实现时由机器 schema 定义。

## 1. 契约原则

1. 每个边界使用封闭 typed contract。
2. 输入必须先完成认证、大小限制、版本和结构校验，再进入 application。
3. **网络输入不得直接成为任意命令、路径、UID、unit、环境、upstream 或配置片段。**
4. 公开边界返回稳定 `ErrorCode`；领域内部保留 typed error。
5. **password 明文、private key 与 Device Token 值不进入通用 API、Observed、日志、指标或普通 audit。**
6. 兼容性只在明确的 wire/schema version 范围内提供。
7. 未知 enum/oneof、超限 frame、重复非法字段或版本不匹配必须显式失败。
8. 人类叙述状态不能偷偷成为 wire 字段。
9. Command replay 由 Panel-owned `command_id` 以及 `request_fingerprint_version` / `request_fingerprint_sha256` 定义；持久化 payload 使用 `payload_version` 与 `frozen_payload_json`。

### 1.1 Identifier

以下 Server/Panel 生成的 surrogate public identifier 必须是 canonical lowercase hyphenated UUIDv7：`device_pk`、`operator_id`、`account_id`、`audit_event_id`、`vault_record_id`、`correlation_id`、`group_correlation_id`、`command_id` 与 `enrollment_request_id`。其中 `command_id` 由 Panel 生成，其余由 Server 生成。`device_pk` 在 wire 上的名字是 `device_id`（对应关系见 §3.6.1）。

业务自然键明确不属于该 Identifier 契约：`seat_id` 是 seat code，`machine_hardware_id` 是按固定配方派生的 hash。该约束的 guard 位于 HTTP/WSS 边界；SQL 列继续使用 `TEXT`，schema tests 会有意插入非 UUID 值。

## 2. 契约所有权与入口拓扑

每个边界的机器权威源（OpenAPI、Protobuf descriptor、D-Bus introspection、SQL migration）是其结构的唯一来源；语义由对应模块拥有。**不得手工维护与生成契约并行的“第二份完整字段表”。**

Operator HTTP、Enrollment 与 Device WSS **合并到同一 TCP 端口**（[ADR-0033](adr/0033-enrollment-and-device-control-boundary.md)），各自使用独立路由、授权与限流；防火墙面为一个 TCP 端口。

Server TLS 的 ALPN protocol-ID 集合固定且仅包含 `http/1.1`；不得宣告 `h2`；Operator HTTP、Enrollment 与 Device WSS 均经同一 HTTP/1.1 listener，Device WSS 使用 RFC 6455 Upgrade。

Server TLS 只启用 TLS 1.3；不得启用 TLS 1.2 或更早版本。

Server TLS leaf 与私钥从 Server 私有状态目录读取，分别固定为 X.509 DER 与 PKCS#8 DER；Server 不接受 PEM 或其他编码。

**2026-08-16 修订（Phase 3 WP2b）**：package-owned `/etc/natsume-server/config.toml` 的 `[site]` section 固定包含 `config = "/etc/natsume/site.toml"`、`control_root = "/etc/natsume/trust/control-ca.crt"` 与 `local_origin_root = "/etc/natsume/trust/local-origin-ca.crt"`；三者均为绝对路径。共享 `/etc/natsume/site.toml` 为 Gateway 签发新增三个必填顶层 key：`gateway_hostname`（canonical lowercase DNS hostname，不允许 IP literal、尾随点、空 label 或非 LDH label）、`gateway_not_after`（RFC 3339 UTC、尾随 `Z`）与 `contest_end`（RFC 3339 UTC、尾随 `Z`）。Origin CA issuing material 只从 Server 私有 keys 目录的固定文件 `/var/lib/natsume-server/keys/origin-ca.der`（X.509 DER）和 `/var/lib/natsume-server/keys/origin-ca-key.pk8`（PKCS#8 DER）读取；`serve` 在 bind 前校验证书、私钥及二者匹配，missing/malformed/mismatch 均 fail closed，`bootstrap` 与 `reset-operator-password` 绝不创建、修改或校验这两个文件。Gateway leaf 每次签发复检 `gateway_not_after` 至少晚于当前时间硬编码 liveness floor `GATEWAY_MINIMUM_REMAINING_VALIDITY_SECONDS = 300`，且 `gateway_not_after >= contest_end + GATEWAY_VALIDITY_MARGIN_SECONDS`，其中 coverage margin 固定为 `GATEWAY_VALIDITY_MARGIN_SECONDS = 86_400`；site startup preflight 校验同一 coverage 关系，两个常量均不进入配置。

**2026-08-16 修订（Phase 3 WP2b Origin CA equality preflight）**：`serve` 还必须把 `[site].local_origin_root` 的 packaged PEM certificate 解码为 DER，并与 `/var/lib/natsume-server/keys/origin-ca.der` 逐字节比较；二者必须是同一 CA certificate，mismatch 以独立静态 startup cause fail closed，且发生在 bind 前。

- `GET /api/v2/health` 是无需认证的进程存活检查，固定返回 HTTP 200 与 JSON `{"status":"ok"}`，不查询数据库，且不表示 readiness 或依赖健康状态。

### 2.1 Server 运行模式

单一 runtime binary `natsume-server` 使用 clap derive 只分派三个必选 subcommand：`serve`、`bootstrap` 与 `reset-operator-password`。三者均无自定义参数与自定义 flag；argv 不承载配置、路径或秘密，唯一配置源保持 package-owned 固定文件 `/etc/natsume-server/config.toml`。缺少或未知 subcommand、或出现额外参数时，必须在接触文件系统前 fail closed。

`natsume-server serve` 的固定启动序列为：加载固定配置与共享 site issuance policy；以 `create_if_missing = false` 打开**已经存在**的数据库，缺失即失败；运行 migration 与 provisioning close-once recovery；只读取并校验**已经存在**的 vault 主密钥，缺失即失败且绝不创建；校验 Origin CA issuing material，再校验 TLS identity；最后才 bind 并 serve。`serve` 不创建数据库、vault 主密钥、Origin CA material 或 operator account，也不提示输入。

`natsume-server bootstrap` 的固定离线序列为：加载固定配置；以 `create_if_missing = true` 创建或打开并 migrate 数据库；vault 主密钥缺失时创建，已存在时只读取并校验；从 TTY 读取 login name，并以不回显方式读取两次 password；仅当 `operator_accounts` 为空时，把唯一 first admin 与其 typed audit row 在同一事务内创建，然后退出。它不做 TLS preflight、不 bind、不启动 listener。重复执行 `bootstrap` 必须保持零业务写入并以非零状态退出。

`natsume-server reset-operator-password` 是 first admin 密码遗失时唯一的非破坏性恢复路径（[ADR-0037](adr/0037-operator-identity-and-server-runtime-secrets.md)）。其固定离线序列为：加载固定配置；以 `create_if_missing = false` 打开**已经存在**的数据库，缺失即失败；运行 migration；从 TTY 读取目标 login name，并以不回显方式读取两次新 password，使用与 §3.6.3 完全相同的 Argon2id profile。随后在**同一事务**内更新该 operator 的 PHC string、删除该 operator 当前全部 session row，并插入 actor 为 `system:password-reset` 的 typed audit row。login name 未知时以非零状态退出且零写入。它不做 TLS preflight、不 bind、不启动 listener；**它绝不创建账户，也绝不接触 vault 主密钥。** **2026-08-15 修订**：已实现；审计词汇（actor `system:password-reset`、action `reset_operator_password`、reason `credential_recovery`、detail `removed_session_count`）已在 §3.6.4 注册表登记。

`postinstall` 不得调用需要交互式 TTY 的 `bootstrap` 或 `reset-operator-password`，安装期不得处理 operator secret；operator 必须在 TTY 上以 `natsume-server` 用户手工运行它们。

Server logging 只写 stderr，由 systemd/journald 收集；配置只来自 package-owned 固定配置文件的可选 `[log]` section。`level` 是封闭枚举且只允许 `error`、`warn`、`info`、`debug`、`trace`，section 或 field 缺失时固定默认为 `info`；未知值必须在 startup fail closed，且不得回显原值。不得从环境变量或 argv 读取 log filter，也不提供 per-module / per-target directive。日志必须遵守 [安全与恢复 §10](security-recovery.md#10-日志和指标脱敏) 的 allowlist 与默认禁止项，尤其不得记录 secret、operator credential、payload dump、完整 filesystem path 或 error source chain。

## 3. Operator HTTP

### 3.1 基本要求

- 使用 HTTPS；operator session 与两级固定角色（`admin` / `viewer`，[ADR-0034](adr/0034-state-execution-and-data-plane-boundary.md)）在 Server 边界执行；
- mutation 必须有 correlation ID，返回“领域已提交”或“Command 已创建”，**不虚构远端完成**；需要 audit 的 mutation 由其 guarded operation 在同一 transaction 内自行插入 audit row；fresh `audit_event_id` 可作为 typed input，但已持久化的同 ID 或预插入 audit row 不能重放为新 mutation 的依据；
- destructive / high-impact mutation 要求明确确认语义；contest configuration 的 **Import Commit** 本身即二次确认动作，不新增独立 confirmation resource；
- 非 Command mutation 使用其领域的 CAS/revision 或明确的 repeat-safe 语义；不得用浏览器重试猜测副作用边界。

### 3.2 HTTP 错误响应

每个进入 Axum 的请求由 Server 生成新的 UUIDv7 correlation ID；client-supplied correlation ID 既不接受也不传播。每个正常响应与每个错误响应都返回 `X-Correlation-Id`；mutation 的 audit row 使用同一个 ID。

HTTP 错误响应的 media type 固定为 `application/json`。wire body 只包含 `title`、`status`、`code`、`correlation_id`，不接受其他字段；`code` 来自稳定 ErrorCode registry，调用方不得解析 `title` 判断业务。

### 3.3 Secret API 约束

允许：上传 CSV 做 redacted preview（密码只存在于请求体解析期内，preview 不写 vault）；人工触发 `SYNC_SECRET`；展示 credential revision 和 redacted result；展示 opaque `preview_token`、redacted import summary 与 binding impacts。preview evidence 只有非秘密 diff 与受影响 account 计数，**不包含密码内容是否变化的任何分类或布尔证据**（[ADR-0031](adr/0031-contest-import-and-secret-evidence.md)）。

**2026-08-16 修订**：Phase 2 已挂载以下 import route：

| Method | Path | 角色 | 语义 |
|---|---|---|---|
| `GET` | `/api/v2/imports` | `admin` | 返回 singleton pending candidate 的 redacted summary；不返回 preview token，读取时 lazy-expire |
| `POST` | `/api/v2/imports` | `admin` | 严格解析 CSV、计算 redacted diff 与非秘密 fingerprint、持久化 singleton 非秘密草稿；零 confirmed 写入、零 vault 写入 |
| `POST` | `/api/v2/imports/{import_id}/actions/commit` | `admin` | 同一 CSV + `Natsume-Preview-Token`；校验 canonical ID、token 与 fingerprint 后原子替换 confirmed configuration；不对任何 revision 做 CAS |
| `POST` | `/api/v2/imports/{import_id}/actions/discard` | `admin` | 删除 pending 草稿行，不改变 confirmed truth、不触及 vault |

**2026-08-16 修订（Phase 3 WP2a）**：挂载以下 provisioning-window operator route：

| Method | Path | 角色 | 语义 |
|---|---|---|---|
| `POST` | `/api/v2/provisioning-window/actions/open` | `admin` | 按 [ADR-0033](adr/0033-enrollment-and-device-control-boundary.md) 打开当前 provisioning window；目标已满足时 repeat-safe `noop` |
| `POST` | `/api/v2/provisioning-window/actions/close` | `admin` | 按 [ADR-0033](adr/0033-enrollment-and-device-control-boundary.md) 关闭当前 provisioning window；目标已满足时 repeat-safe `noop` |
| `GET` | `/api/v2/provisioning-window` | `admin` / `viewer` | 按 [ADR-0033](adr/0033-enrollment-and-device-control-boundary.md) 返回当前 `{state, revision}` fact |

**2026-08-16 修订（Phase 3 WP2c）**：挂载以下 Enrollment review operator route；list 为 current-fact read，绝不返回 CSR bytes：

| Method | Path | 角色 | 语义 |
|---|---|---|---|
| `GET` | `/api/v2/enrollment-requests` | `admin` / `viewer` | 按 `created_at`、`enrollment_request_id` 返回 live `pending` / `approved` request 的 redacted review facts |
| `POST` | `/api/v2/enrollment-requests/{request_id}/actions/approve` | `admin` | 审批 pending replacement；不签发凭据，Device 下次 claim POST 才签发 |
| `POST` | `/api/v2/enrollment-requests/{request_id}/actions/reject` | `admin` | 拒绝 pending replacement；Device polling 观察 `ENROLLMENT_REQUEST_REJECTED` |

**禁止**：

- 返回 password 值；
- 提供通用 secret read endpoint；
- 把 password 明文放入 audit diff、metric 或普通 log；
- 在错误 `detail` 中包含 CSV 行原文或密码；
- 将浏览器 local/session storage 作为秘密存储。

### 3.4 Operator import 边界

Import 是对 confirmed contest configuration 的高影响路径。稳定语义（领域规则以 [领域模型](domain-model.md) 为准，并发模型见 [ADR-0031](adr/0031-contest-import-and-secret-evidence.md)）：

- **全局同一时刻最多一个非秘密 pending candidate**；`pending_import_candidate` singleton row 存在即为 pending，新 upload 前需显式终止现有 candidate；
- candidate 只在严格解析成功后持久化，row 只包含 candidate ID、expiry、`preview_token_hash`、`nonsecret_fingerprint_version`、`nonsecret_fingerprint_sha256` 和 `redacted_preview_json`；普通 surface 使用 `candidate_id` 和 opaque `preview_token`，不使用 import state/history、configuration/Binding baseline、encrypted CSV 或可见 password-bearing snapshot；preview 零 vault 写入；
- **Server 是 diff classification 的唯一权威**；client/UI 只渲染结构化结果，不得本地重算分类；
- preview 绑定 candidate identity、redacted diff、非秘密 fingerprint 与过期时间；数据库只保存 opaque token 的 hash 与 fingerprint；不返回 baseline configuration/binding revision；密码在解析完成后从内存丢弃，commit 时再次提交；
- Commit 不对任何 revision 做 CAS。`seats` / `account_mappings` / Account 密码的唯一写入方是 Import Commit 本身；存在 singleton pending 时第二次 upload 被拒绝；single-lifetime reset 删除 candidate。Import **零 Binding 写入**，不铸造 Binding stamp；将删除且 commit 时仍绑定的座位返回 `IMPORT_SEATS_STILL_BOUND`，零 confirmed 变更；非秘密 fingerprint 不一致返回 `IMPORT_CANDIDATE_MISMATCH`，零写入且 candidate 保留；
- Commit、discard 和 expiry 在各自事务中删除 pending 草稿行，仅留下 redacted audit lineage；不删除 vault payload row。重复请求不能借由保留 terminal candidate state 取得新的业务结果；
- Import Commit 不创建 Command，不自动 `SYNC_STATE`/`SYNC_SECRET`，**不产生 Device I/O**，也不表示 Device 已同步；
- 任何 invalid、expiry、discard、authorization failure、非秘密 fingerprint 不一致、将删座位仍绑定或 transaction failure **均不得改变 confirmed truth、binding 或相关 revision**；
- 清空 confirmed configuration 只能通过独立 single-lifetime reset，不得由 import 隐式完成。

**2026-08-15 修订（Phase 2 启动冻结：import HTTP 面与 preview evidence 字段）**：

- 传输：`POST /api/v2/imports`，`Content-Type: text/csv`（UTF-8，允许 BOM），`admin` only。request body 是硬编码 route 级安全常量 `CSV_IMPORT_BODY_LIMIT_BYTES = 4_194_304`（4 MiB，约为 500 席位 CSV 的 80 倍余量），施加于 upload 与 `commitCsvImport` 的 `MethodRouter`，不入 `config.toml`；超限在解析与任何数据库访问前返回 `413`。不存在 `IMPORT_COMMIT_BODY_LIMIT_BYTES`。
- 字段长度上限为硬编码常量：seat ≤ 64、account ≤ 64、password ≤ 512 字节；数据行数上限为硬编码常量 `MAX_IMPORT_ROWS = 10_000`（约为 500 席位假设的 20 倍）；超限拒绝归类为 `IMPORT_CANDIDATE_INVALID`；均不入 `config.toml`。
- 上传同步完成严格解析、diff 分类、非秘密 fingerprint 计算与 candidate 落库，成功返回 `201` 与 `ImportPreviewResponse`：`candidate_id`（canonical UUIDv7）、`preview_token`（opaque，仅在本响应呈现一次）、`expires_at`（RFC 3339 UTC）、`diff`。不返回 baseline configuration/binding revision。零 confirmed 写入、零 vault 写入。TTL 冻结为常量 `IMPORT_CANDIDATE_TTL_SECONDS = 1_800`（30 分钟），不入 `config.toml`。密码在解析完成后从内存丢弃。
- `preview_token` 为 32 字节 CSPRNG，以无填充 URL-safe base64 呈现；数据库只存其 SHA-256；比较必须常量时间。commit 时作为 HTTP header `Natsume-Preview-Token` 回传，不得放入 query string（会进日志）。
- Import nonsecret fingerprint v1：`nonsecret_fingerprint_version = 1` 是以下字节序列的 SHA-256：ASCII domain separator `natsume:import-nonsecret:v1`，随后一个 NUL byte（`0x00`），随后 JSON 数组的 JCS（RFC 8785）序列化。数组元素为对象 `{seat_code, domjudge_username}`，按 `seat_code` 升序。password 列不进入该哈希。字段集合或序列化的任何变更都必须使用 `nonsecret_fingerprint_version + 1`。
- `diff`（redacted，Server 唯一权威）字段冻结为：`seats_added[]`（seat_code）、`seats_removed[]`（seat_code）、`mappings_changed[]`（`{seat_code, current_domjudge_username|null, candidate_domjudge_username}`，只含存续 Seat）、`unchanged_count`（存续且 mapping 不变的 Seat 数）、`affected_account_count`（candidate 配置内全部 Account 数——commit 后其 `credential_revision` 无条件推进的对象）、`binding_impacts[]`（`{seat_code, device_id}`，被移除且**当前**有 Binding 的 Seat）。`binding_impacts` 是 commit blocker 预告，不是解绑计划；非空时 commit 必须失败。全部列表按 `seat_code` 升序，保证 golden 可比。密码内容是否变化不分类、不出现（既有冻结）。
- Commit：`POST /api/v2/imports/{import_id}/actions/commit`，`Content-Type: text/csv`，body 为**同一 CSV**（同一 `CSV_IMPORT_BODY_LIMIT_BYTES`），`admin` only；`preview_token` 仅经 header `Natsume-Preview-Token` 传递，不使用 `{preview_token}` JSON body。`import_id` 必须与 `candidate_id` canonical 逐字符相等。成功 `200` 返回 `{}`（无 `configuration_revision` / `binding_revision`）。commit 在同一 `BEGIN IMMEDIATE` 事务内：校验 token、重解析 CSV、重算 fingerprint 并与所存哈希常量时间比较；不一致则 `409 IMPORT_CANDIDATE_MISMATCH` 且零写入、candidate 保留。然后再读取将删座位的当前 Binding：任一仍绑定则 `409 IMPORT_SEATS_STILL_BOUND` 且零写入。通过后应用 seats/mappings/Account vault ciphertext，删除 pending 行并审计。
- Discard：`POST /api/v2/imports/{import_id}/actions/discard`，无 body，`admin` only；成功 `204`。**不要求 `preview_token`**——token 只存在于浏览器内存，页面刷新即丢失；若 discard 也要求 token，operator 将被锁死至过期而无法重传。discard 是零业务变更操作，admin session 已是足够授权；commit 保持 token 必需（第二次显式确认，并把提交绑定到已审阅的 preview）。discard 只删除 pending 草稿行，不触及 vault。
- 错误映射（复用 error-code registry 既有冻结码，并登记 `IMPORT_SEATS_STILL_BOUND` 与 `IMPORT_CANDIDATE_MISMATCH`；随 route 挂载登入 §3.6.5 表）：解析失败、结构错误、candidate 内重复 account、空或仅 header → `400 IMPORT_CANDIDATE_INVALID`；存在 pending 时的再次 upload → `409 IMPORT_CANDIDATE_PENDING`；未知、已过期或已 discard 的 `import_id`，以及 **`preview_token` 不匹配** → `404 IMPORT_CANDIDATE_UNAVAILABLE`（token 不匹配与 candidate 不存在必须不可区分，不给未持 token 方提供 candidate 存在性 oracle；audit 内部以 `reason_code` 区分真相）；commit 重解析后非秘密部分与所存 fingerprint 不一致 → `409 IMPORT_CANDIDATE_MISMATCH`（candidate 保留至 discard/expiry/成功 commit，operator 可用原文件重试；**不得**折叠为 `IMPORT_CANDIDATE_UNAVAILABLE`）；将删除座位在 commit 时仍有 Binding → `409 IMPORT_SEATS_STILL_BOUND`（preview 已列出的 impacts 与空闲期内新绑上的座位同一码；operator 解绑后必须重新 preview，因为座位集合可能已变）。不存在 `IMPORT_PREVIEW_STALE`。
- 过期为 lazy 清理（与 expired session row 同原则）：首个观察到 expired candidate 的 import surface 请求在同一事务内删除 pending 草稿行并审计一次，不运行 background cleaner，也不删除 vault row。
- 审计词汇已按 §3.6.4「先注册后写入器」纪律登记；四个 import 写入器与 HTTP 层 rejected-upload 的 `candidate_invalid` 写入器均已落地（已实现）。

**2026-08-16 修订（Phase 2 补全：pending 读取面）**：

- Pending read：`GET /api/v2/imports`，`admin` only；成功 `200` 返回 `ImportPendingResponse { pending: ImportPendingSummary | null }`，其中 summary 恰含 `candidate_id`（canonical UUIDv7）、`expires_at` 与 `diff`。该 surface **绝不返回 `preview_token`**，也不返回 baseline configuration/binding revision；页面刷新后 operator 可查看并 discard，但必须重新上传才能 commit。读取时观察到 expired candidate 必须在同一事务执行 tolerant lazy expiry 与 `expire_import_candidate` 审计，并返回 `pending: null`；不存在 candidate 时同样返回 null 且零写入。

**2026-08-20 修订（Import 不修改 Binding，且取消 revision CAS）**：Import Commit 零 `device_bindings` 写入、不铸造 Binding stamp。已删除 `revision_counters`；candidate / preview / pending read 删除 `baseline_configuration_revision` 与 `baseline_binding_revision`；commit 成功体删除 `configuration_revision` 与 `binding_revision`，返回 `{}`。Commit 不对任何 revision 做 CAS；preview→commit 锁是 singleton pending candidate。将删座位在 commit 时仍绑定 → `409 IMPORT_SEATS_STILL_BOUND`。从 live catalog 与 HTTP mapping 删除 `IMPORT_PREVIEW_STALE`（预发布原位 BC；G0 已关闭，但可移除未使用的 import CAS 码）。`binding_impacts[]` 保留为 blocker 预告。实现与 OpenAPI / schema 须同批收口；在此之前不得把旧 unbind-and-replace 或 import revision CAS 行为当作规范。

**2026-08-20 修订（preview 不持久化密码）**：删除 `import_payload` vault type 与 `IMPORT_COMMIT_BODY_LIMIT_BYTES`。preview 只持久化非秘密 pending 草稿并返回 preview，零 vault 写入。commit 请求体改为同一 CSV，`preview_token` 仅经 `Natsume-Preview-Token` header。新增稳定码 `IMPORT_CANDIDATE_MISMATCH` / `409`（audit `reason_code` 为 `nonsecret_mismatch`）；不得折叠为 `IMPORT_CANDIDATE_UNAVAILABLE`。实现与 OpenAPI / schema 须同批收口；在此之前不得把 encrypted whole-CSV staging 或 `{preview_token}` JSON commit body 当作规范。

### 3.5 Direct Command creation

Command create/replay 的唯一 HTTP 资源契约为：

```text
PUT /api/v2/commands/{command_id}
```

- **Panel 在发起请求前生成 `command_id`**。它必须是 canonical lowercase hyphenated UUIDv7：可解析、version 为 7，且其 canonical string 与 path 输入逐字符相同。Server 不生成、重写或为同一意图替换这个 ID。
- request 的持久化 target 是 `device_pk`，并使用封闭 `kind`、`payload_version`、`payload`、可选 `reason_code` 与可选 `group_correlation_id`；持久化 row 只为 `group_correlation_id` 保留对应 top-level column，`reason_code` 不另设 top-level column。
- Server 对 canonical request 计算 versioned、domain-separated SHA-256 fingerprint，并持久化为 `request_fingerprint_version` 和 `request_fingerprint_sha256`；算法由下述 v1 小节冻结。
- **只有当前 state 为 `enrolled` 的 Device 才有资格首次持久化 Command。** 精确顺序为：事务外完成 request 校验；进入 `Database::write` / `BEGIN IMMEDIATE`；以 typed `DeviceState` 查找 target（不存在即 `DeviceNotFound`）；再查同 ID fingerprint（相同即 replay，不同即 conflict，均忽略当前 Device state）；仅在同 ID 尚不存在时检查 state，非 `enrolled` 即 `DeviceNotEnrolled` 且零 Command、零 audit、零 notifier；最后才为 `enrolled` target 在同一事务插入 created audit 与 Command。Command PUT 与 lifecycle mutation 共用 `BEGIN IMMEDIATE`，因此 disable/revoke 与新 ID PUT 的竞态只能得到以上两个串行结果，不能产生 `DeviceNotEnrolled` 与新 Command row 同时成立的结果。
- 没有该 ID 时，Server 原子持久化 Command 与 `created_audit_event_id` 指向的创建 audit，返回 `201`。已有该 ID 且 fingerprint 相同，返回同一个已持久化 Command，返回 `200`，不再写 audit 或重复任何副作用。已有该 ID 且 fingerprint 不同，返回 `409` / `COMMAND_REQUEST_CONFLICT`。
- Device 在 Command 创建后被 disable 或 revoke 不改变该 ID 的幂等事实：same-ID/same-fingerprint 仍返回 `200` 且零写入，same-ID/different-fingerprint 仍返回 `409` 并写既有的独立 conflict audit。只有首次持久化受 `enrolled` gate 约束。
- `409` conflict 路径写一行 audit：`resource_id` 为 `command_id`、`result` 为 rejected、`reason_code` 为 `COMMAND_REQUEST_CONFLICT`；`redacted_detail_json` 可含 fingerprint **version** 与计数，**绝不含 fingerprint 值或 request 回显**。conflict 是低频异常信号（client 缺陷或攻击），与既有 import stale-reject 审计同一原则。same-ID/same-fingerprint replay 仍然零 audit 写入。
- 非 canonical UUIDv7 返回 `400` / `COMMAND_ID_INVALID`。错误不得回显原始 request、fingerprint、secret 或未脱敏诊断。
- 每个 bulk target 使用一个独立 `command_id`。可选 `group_correlation_id` 只用于查询和审计分组；即使它参与 fingerprint，也不表达顺序、原子性、重试或跨 Device lifecycle。
- 本节冻结 OpenAPI 和后续实现必须遵守的语义；**不声明 Phase 0 已经提供 HTTP listener、授权 handler、Command repository、dispatcher 或实际 Panel mutation。**

#### Command request fingerprint v1

wire `payload` / `payload_version` 即列 `frozen_payload_json` / `payload_version` 的输入；`payload` 经 per-kind schema 验证后，其 JCS（RFC 8785）规范形原样存入 `frozen_payload_json`，验证后的规范形即存储形。fingerprint 覆盖同一 JCS 规范形；因此同 `command_id` 同 fingerprint 蕴含同 frozen payload。反向不成立：仅 `reason_code` 或 `group_correlation_id` 不同的请求 fingerprint 不同而 frozen payload 相同——server 谓词严格更细且权威。

`request_fingerprint_version = 1` 的 fingerprint 是以下字节序列的 SHA-256：ASCII domain separator `natsume:command-request:v1`，随后一个 NUL byte（`0x00`），随后对象 `{device_id, kind, payload_version, payload, reason_code, group_correlation_id}` 的 JCS（RFC 8785）序列化。`reason_code` 与 `group_correlation_id` 缺失时必须完全省略对应 key；全部输入均使用通过 schema 验证的 HTTP request 值。Server 派生的 frozen timestamps、actor、session 与 retry time 绝不参与。

字段集合或序列化的任何变更都必须使用 `request_fingerprint_version + 1`；旧版本永久有效，每行由 `request_fingerprint_version` 记录其版本。

### 3.6 Operator session 与 Phase 1 operator API

Operator 身份与会话是 Server 持久化事实（[ADR-0037](adr/0037-operator-identity-and-server-runtime-secrets.md)；领域语义见 [领域模型](domain-model.md)）。

#### 3.6.1 冻结的 Phase 1 operator API contract

下表是 Stage 0 冻结、供 Phase 1 / Stage 5 实现遵守的规范 surface，**不是当前 route 已全部挂载的声明**：

| Method | Path | 角色 | 语义 |
|---|---|---|---|
| `POST` | `/api/v2/session` | anonymous | 建立 operator session |
| `GET` | `/api/v2/session` | `admin` / `viewer` | 返回当前 operator identity 与 role |
| `DELETE` | `/api/v2/session` | `admin` / `viewer` | 终止当前 operator session |
| `GET` | `/api/v2/seats` | `admin` / `viewer` | 返回当前 Seat 集合 |
| `GET` | `/api/v2/accounts` | `admin` / `viewer` | 返回当前 Account identity 与 credential revision，不含任何 password evidence |
| `GET` | `/api/v2/devices` | `admin` / `viewer` | 返回当前 Device 集合与 state |
| `GET` | `/api/v2/bindings` | `admin` / `viewer` | 返回当前 Seat↔Device Binding 集合；响应行 = `{seat_id, device_id, binding_revision}`，其中 `binding_revision` 为行级 stamp，供 Panel 做变更感知 |
| `POST` | `/api/v2/devices/{device_id}/actions/revoke` | `admin` | Device 转为 `revoked`、移除当前 Device Token，并将关联 Gateway certificate 状态行转为 `revoked` 后保留 |
| `POST` | `/api/v2/devices/{device_id}/actions/disable` | `admin` | Device 转为 `disabled`；保留当前 Device Token row 与 active Gateway certificate row |

两个 Device lifecycle action 是 **repeat-safe** 的 current-fact mutation：目标 state 已达到时零业务写入，写一行 `result = 'noop'` 的 audit，并返回 HTTP `200`；首次实际生效同样返回 HTTP `200`，audit 为 `result = 'succeeded'`。

transition matrix 固定如下：`revoke` 可从任意当前 state 应用；从 `enrolled` 或 `disabled` 执行完整 revoke effect，从 `revoked` 进入 convergence 检查。只有 `devices.state = 'revoked'`、不存在当前 `device_tokens` row，且该 Device 不存在任何状态不为 `revoked` 的保留 `gateway_certificates` row 时，`revoke` 才是 `noop`；任一部分尚未收敛都必须补全剩余 effect，并记录 `result = 'succeeded'`。

`disable` 从 `enrolled` 转为 `disabled`，保留当前 Device Token row 与 active Gateway certificate row；保留是 lifecycle 与审计证据语义，**不授权 Device WSS**，因为 WSS 还要求 resolved Device state 为 `enrolled`。对已经 `disabled` 的 Device 是 repeat-safe `noop`。对 `revoked` Device 同样是零业务写入的 `noop`：`revoked` 已包含比 `disabled` 更强的限制，并额外移除了 Device Token、将关联 certificate 状态行转为 `revoked` 后保留；不得把它降级回 `disabled` 而静默削弱安全状态，也不为此引入新的 stable ErrorCode。

二者都不创建 Command、不产生 Device I/O、不改变 Binding 集合，也不推进任何 revision。解绑是独立的 Binding-set mutation，绝不由 Device state transition 隐含触发。

HTTP 边界的 `device_id` 与持久化的 `devices.device_pk` 是同一值的两个名字；不存在 mapping table，也不做 format conversion。

read route 返回 bounded 集合；Phase 1 不提供任意 filter、sort、query language 或分页。

#### 3.6.2 当前 Stage 5B mounted subset

Stage 5B 当前挂载 §2 的 `GET /api/v2/health`，以及 §3.6.1 表中的全部九个 Phase 1 operator operation；每个 operation 都有真实 handler，不提供 placeholder handler 或 placeholder schema。

**2026-08-16 修订**：Phase 2 在上述 Stage 术语下新增挂载 `getCsvImport`、`createCsvImport`、`commitCsvImport` 与 `discardCsvImport`，四者均为真实 handler。OpenAPI 除已挂载 surface 外，现只声明但不挂载 `approveEnrollment`、`putCommand`；`info.description` 必须列出这一 declared-but-unmounted 集合，防止 schema 声明被误读为可调用 route。

**2026-08-16 修订（Phase 3 WP2a）**：新增挂载 `getProvisioningWindow`、`openProvisioningWindow` 与 `closeProvisioningWindow`，三者均为真实 handler；前者允许 `admin` / `viewer`，后两者仅允许 `admin`。

**2026-08-16 修订（Phase 3 WP2b）**：新增挂载无需 operator session 的 device operation `createEnrollmentRequest`（`POST /api/v2/enrollment-requests`）；`approveEnrollment` 仍只声明、不挂载，operator approve/reject HTTP 均归 WP2c。`info.description` 的 mounted 与 declared-but-unmounted 集合必须同步反映该边界。

**2026-08-16 修订（Phase 3 WP2c）**：新增挂载 `listEnrollmentRequests`、`approveEnrollment` 与 `rejectEnrollment`；list 允许 `admin` / `viewer`，两个 action 仅允许 `admin`。OpenAPI declared-but-unmounted 集合现在只剩 `putCommand`。

**2026-08-16 修订（Phase 4 WP1）**：新增挂载 `putCommand`（§3.5 的 `PUT /api/v2/commands/{command_id}`，真实 handler，仅允许 `admin`）。OpenAPI declared-but-unmounted 集合现为空，`info.description` 同步反映。

#### 3.6.3 建立与查询

- `POST /api/v2/session` 只接受恰好含 `login_name` 与 `password` 的封闭 JSON object，未知字段必须拒绝。成功同时发送 session cookie 并返回 `200`，响应体只含 `operator_id` 与 `role`。
- `GET /api/v2/session` 对有效会话返回 `200`，响应体同样只含 `operator_id` 与 `role`。它不续期、不更新 expiry，也不重新发送 cookie。
- session cookie 名固定为 `__Secure-natsume_session`；属性固定为 `Path=/api/v2`、`Secure`、`HttpOnly`、`SameSite=Strict`、`Max-Age=57600`，不发送 `Expires`。不使用 `__Host-` 前缀，因为该前缀要求 `Path=/`，与冻结的 API-prefix cookie scope 冲突。
- session credential 是 OS CSPRNG 生成的 32 bytes，在 cookie 中以 lowercase hex 传输；数据库的 `session_credential_hash` 只保存 raw credential 的 32-byte SHA-256。cookie 值不进入日志、指标、audit、HTTP 错误响应或 OpenAPI example。
- session 从创建起绝对有效 16 小时；不存在 sliding renewal。
- operator password 使用 Argon2id version 19，参数固定为 `m=19456 KiB`、`t=2`、`p=1`，salt 为 OS CSPRNG 生成的 16 bytes，持久化为 PHC string；测试不得使用弱化 profile。
- unknown login、错误 password 与持久化 PHC malformed 都执行一次同一 profile 的 verification，并统一返回 `401` / `AUTHENTICATION_FAILED`；unknown-login 路径使用固定、非秘密的 dummy PHC，不形成账户存在性或 hash-format oracle。
- Operator session 不授予任何 Device 控制面身份：它不能取得 Device Token、Gateway certificate 或 WSS 连接（`INV-CERT-01`）。

#### 3.6.4 终止、过期与 audit

- `DELETE /api/v2/session` 对 credential state 的结果保持 repeat-safe：有效 session 只删除并审计一次；missing、malformed、unknown 或已删除 credential 是零写入 no-op；这些成功或 no-op 结果都返回 `204`。无论结果为 `204` 或下述 `500`，响应都发送同名、同 `Path=/api/v2` scope 与同安全属性的 clearing cookie。
- 真正的 persistence 或 infrastructure failure 返回 `500` / `INTERNAL_ERROR`，不得以 `204` 掩盖仍可能存活的 session。这不构成可利用的 credential-state oracle：termination path 对 live session 与 no-row path 都先开启 `BEGIN IMMEDIATE` transaction、读取并 commit，因此连接获取、事务开启与读取阶段的失败不与 session 是否存在相关联。写执行阶段的失败只可能出现在 live path（no-row path 零写入），但到达任一分支都必须先呈递结构合法的 32-byte credential，其猜测代价使该差异不可利用。
- expired session 只做 lazy cleanup：首个观察到仍存在 expired row 的请求在同一事务内删除并审计一次，后续请求为零写入；不运行 background cleaner。不做 GC 的前提是累积量已被这些事实限死：单实例只服务单场 contest、operator 账户是个位数、session 绝对 TTL 为 16 小时，且[领域模型](domain-model.md) §14 的 single-lifetime reset 会按场次清空业务状态。
- 只审计 bootstrap first-admin creation、离线 operator password reset、session established、首次 session termination、首次 observed expiry 与 §3.6.5 的失败登录限流触发。单次失败登录本身不审计（flood control）；但限流阈值被跨越时，每个 limiter window 写一条 audit row：actor 是非人类 system actor，`redacted_detail_json` 以 typed field 承载失败计数与来源 IP。**尝试使用的 login name 绝不进入 audit 或日志**——它可能是误输入到该字段的密码。

##### 当前 AuditEvent 词汇注册表

以下四张注册表只收录当前生产写入器已冻结的词汇；测试夹具中用于造数的 actor/action 不属于生产契约。任何新的审计行形状都必须先在这里注册其 `actor`、`action_kind`、`reason_code` 与对应 action 的 `redacted_detail_json` 键，之后才能增加写入器与测试。本契约另已声明但尚未实现一个审计行形状——§3.6.4 的失败登录限流行——其完整词汇必须在写入器落地前先在此注册（§3.5 的 `COMMAND_REQUEST_CONFLICT` 冲突行已随 Phase 4 WP1 注册并实现）。

| `actor` | 状态 |
|---|---|
| `device:enrollment` | 已实现（device-initiated intake / claim writers） |
| `device:control` | 已注册（Phase 4 WP4 的 Device-reported Command 终态写入器；2026-08-16） |
| `operator:self` | 已实现 |
| `system:bootstrap` | 已实现 |
| `system:expiry` | 已实现 |
| `system:recovery` | 已实现 |
| `system:password-reset` | 已实现 |

| `action_kind` |
|---|
| `create_enrollment_request`（**已实现**；`device:enrollment`） |
| `issue_device_credentials`（**已实现**；`device:enrollment`） |
| `approve_enrollment_request`（**已实现（writer 与 HTTP 已挂载）**；`operator:self`） |
| `reject_enrollment_request`（**已实现（writer 与 HTTP 已挂载）**；`operator:self`） |
| `expire_enrollment_requests`（**已实现**；`operator:self` close 与 `system:recovery` close-once） |
| `open_provisioning_window`（**已实现**；`operator:self` operator action） |
| `close_provisioning_window`（**已实现**；`operator:self` operator action 与 `system:recovery` startup recovery） |
| `create_first_admin` |
| `reset_operator_password` |
| `establish_session` |
| `terminate_session` |
| `expire_session` |
| `revoke_device` |
| `disable_device` |
| `create_import_candidate`（**已实现**，§3.4 Phase 2） |
| `commit_import`（**已实现**，§3.4 Phase 2） |
| `discard_import_candidate`（**已实现**，§3.4 Phase 2） |
| `expire_import_candidate`（**已实现**，§3.4 Phase 2） |
| `command_create`（**已实现**，§3.5 Phase 4 WP1；`operator:self`；`succeeded` 创建与 `rejected` conflict 共用此 action_kind） |
| `command_terminal`（**已注册**，§6 Phase 4 WP4；`device:control`；Device 上报的 Command 终态，每个 `command_id` 至多一行——重复终态合并为零写入） |

| `reason_code` | 当前使用处 |
|---|---|
| `first_enrollment` | `issue_device_credentials` 的 `create_device` 同步签发 |
| `credential_replacement` | `create_enrollment_request` 与经审批的 `issue_device_credentials` 替换签发 |
| `same_spki_retry` | `issue_device_credentials` 的 same-SPKI 自动批准重试 |
| `window_closed` | `expire_enrollment_requests` |
| `startup_recovery` | `close_provisioning_window` |
| `initial_provisioning` | `create_first_admin` |
| `credential_recovery` | `reset_operator_password` |
| `credentials_verified` | `establish_session` |
| `operator_requested` | `terminate_session`；`revoke_device` / `disable_device`、`open_provisioning_window` / `close_provisioning_window`、`approve_enrollment_request` / `reject_enrollment_request`、`create_import_candidate`、`commit_import`、`discard_import_candidate` 与 `command_create` 的 `succeeded` 结果 |
| `absolute_expiry_observed` | `expire_session`；`expire_import_candidate`（已实现） |
| `target_already_satisfied` | `revoke_device` / `disable_device`、`open_provisioning_window` / `close_provisioning_window` 与 `approve_enrollment_request` / `reject_enrollment_request` 的 `noop` 结果 |
| `candidate_invalid` | `create_import_candidate` 的 `rejected` 结果（已实现） |
| `seats_still_bound` | `commit_import` 的 `rejected` 结果（将删座位 commit 时仍有 Binding） |
| `preview_token_mismatch` | `commit_import` 的 `rejected` 结果（对外折叠为 `IMPORT_CANDIDATE_UNAVAILABLE`，见 §3.4） |
| `nonsecret_mismatch` | `commit_import` 的 `rejected` 结果（对外 `IMPORT_CANDIDATE_MISMATCH`，见 §3.4） |
| `COMMAND_REQUEST_CONFLICT` | `command_create` 的 `rejected` 结果（§3.5 固定该 reason_code 值） |
| `device_reported` | `command_terminal` 的全部结果（终态由 Device 上报，Server 不自行推断） |

| `action_kind` | `redacted_detail_json` keys |
|---|---|
| `create_enrollment_request` | `resolution`、`state`、`gateway_spki_sha256` |
| `issue_device_credentials` | `resolution`、`certificate_serial`、`gateway_spki_sha256`、`previous_device_state`；首次创建为 `null`，既有 Device 的替换只可为 `enrolled`，serial 与 SPKI digest 均为 certificate-public evidence，禁止 Device Token。**2026-08-16 修订（Phase 4 WP3）**：新增 `evicted_live_connection`（bool）——replacement 签发时该 Device 存在 live WSS 连接即为 `true`（旧凭据连接被驱逐的 anomaly evidence，Phase 3 移交项） |
| `approve_enrollment_request` | 无（`{}`） |
| `reject_enrollment_request` | 无（`{}`） |
| `expire_enrollment_requests` | `expired_count` |
| `open_provisioning_window` | `previous_revision`、`new_revision` |
| `close_provisioning_window` | `previous_revision`、`new_revision` |
| `create_first_admin` | `role` |
| `reset_operator_password` | `removed_session_count` |
| `establish_session` | `role` |
| `terminate_session` | 无（`{}`） |
| `expire_session` | 无（`{}`） |
| `create_import_candidate`（已实现） | `succeeded`：`seats_added_count`、`seats_removed_count`、`mappings_changed_count`、`binding_impact_count`；`rejected`：无（`{}`） |
| `commit_import`（已实现） | `succeeded`：`seats_added_count`、`seats_removed_count`、`mappings_changed_count`、`credential_revision_advanced_count`（无 `configuration_revision_advanced` / `binding_revision_advanced`、无 Binding 写入）；`rejected`：`seats_still_bound` 时可含 `binding_impact_count`，`nonsecret_mismatch` 与其余为 `{}` |
| `discard_import_candidate`（已实现） | 无（`{}`） |
| `expire_import_candidate`（已实现） | 无（`{}`） |
| `revoke_device` | `resulting_state`、`removed_token_count`、`revoked_certificate_count` |
| `disable_device` | `resulting_state`、`removed_token_count`、`revoked_certificate_count` |
| `command_create`（已实现） | `succeeded`：`kind`、`payload_version`、`request_fingerprint_version`；`rejected`：`request_fingerprint_version`（绝不含 fingerprint 值或 request 回显，§3.5） |
| `command_terminal`（已注册） | `kind`、`terminal_state`（`succeeded`/`failed`/`cancelled`/`expired`/`manual_intervention_required` 之一）、可选 `terminal_error_code`（稳定码字符串）；**不含 payload、result body、frame bytes 或未脱敏诊断**。audit `result` 取 `succeeded`（终态为 `succeeded`）、`noop`（终态为 `cancelled`/`expired`）或 `failed`（其余） |

#### 3.6.5 HTTP adapter、CSRF 与 ingress capacity

下表冻结已挂载 operator route 与 `createEnrollmentRequest` device route、以及 declared-only future operator route 在真实 handler 中唯一允许构造的 typed cause → stable code / status 组合；列出 declared-only mapping 不表示对应 route 已在当前 Stage 挂载：

| 稳定码 | HTTP | 触发 |
|---|---|---|
| `AUTHENTICATION_FAILED` | `401` | 凭证错误、无 session、session 已过期或已失效 |
| `AUTHORIZATION_DENIED` | `403` | `viewer` 请求 `admin` action |
| `INVALID_REQUEST` | `400` | 封闭结构或参数校验失败，包含非 canonical `device_id` |
| `IMPORT_CANDIDATE_INVALID` | `400` | import CSV 解析、结构或安全上限校验失败 |
| `RESOURCE_NOT_FOUND` | `404` | request 结构合法，但目标 Device 不存在；`putCommand` 的目标存在但 state 不是 `enrolled` 时也使用完全相同的公开 body |
| `IMPORT_CANDIDATE_UNAVAILABLE` | `404` | import candidate 未知、已过期、已删除或 preview token 不匹配 |
| `IMPORT_CANDIDATE_PENDING` | `409` | singleton pending import candidate 已存在 |
| `IMPORT_SEATS_STILL_BOUND` | `409` | CSV 将删除的座位在 commit 时仍有 Binding；须先 unbind 再重新 preview |
| `IMPORT_CANDIDATE_MISMATCH` | `409` | commit 重解析 CSV 的非秘密部分与 preview 所存 fingerprint 不一致；candidate 保留，可用原文件重试 |
| `ENROLLMENT_REQUEST_INVALID` | `400` | `createEnrollmentRequest` 的 closed request / CSR / raw SPKI / protocol 无效，或 live Enrollment 全局 capacity 已满；approve/reject 的 request ID 非 canonical UUIDv7，或 request 未知、terminal、处于相反 decision state 时也使用此码 |
| `PROVISIONING_WINDOW_CLOSED` | `409` | `createEnrollmentRequest` 观察到窗口非 `open` |
| `ENROLLMENT_REQUEST_REJECTED` | `409` | `createEnrollmentRequest` 的 hardware ID 最新 request 在当前窗口已被 operator reject |
| `DEVICE_IDENTITY_CONFLICT` | `409` | `createEnrollmentRequest` 的 live different-SPKI、hardware identity facts 冲突，或 resolved Device state 不是 `enrolled` |
| `COMMAND_ID_INVALID` | `400` | `putCommand` 的 path `command_id` 非 canonical lowercase UUIDv7（§3.5；Phase 4 WP1 挂载后生效） |
| `COMMAND_REQUEST_CONFLICT` | `409` | `putCommand` 的 same-ID/different-fingerprint conflict（§3.5） |
| — | `413` | `POST /api/v2/imports`、`commitCsvImport`、`createEnrollmentRequest` 或 `putCommand` request body 超过对应 route 级上限，由 transport 层拒绝 |
| `INTERNAL_ERROR` | `500` | 穷举 mapping 后仍没有更安全公开分类的内部失败 |

该 `413` 是 transport-level rejection：响应携带 `X-Correlation-Id` header，但**不**使用 §3.2 的 JSON error body（沿用既有 session precedent）。

输入分类的冻结规则是：**malformed 或非 canonical 输入 → `400` / `INVALID_REQUEST`（Enrollment 使用其更具体的 `ENROLLMENT_REQUEST_INVALID`）；结构良好但引用了不存在的当前事实 → `404` / `RESOURCE_NOT_FOUND`。** `putCommand` 的 typed `DeviceNotEnrolled` 使用内部 cause `command_device_not_enrolled`，但其公开 status、stable code 与四字段 body schema 必须与 `DeviceNotFound` 完全相同；不新增稳定码。映射必须在 adapter 中按 typed cause 逐项显式构造；不得提供全局 `ErrorCode -> StatusCode` 函数，不得 catch-all，不得根据 `Display` 或 source chain 分支，也不得回显非法输入。运行时该表只覆盖已挂载 route：当前 mounted subset 以 §3.6.2 为准；上表 device-route rows 已随 `createEnrollmentRequest` 挂载生效。新增已挂载 route 时只加入真实可达的组合，不为未挂载 operation 预设 status。

当前是 same-origin JSON API：不启用 CORS，也不增加 CSRF token 或 CSRF framework；当前防护明确依赖 `Secure` + `HttpOnly` + `SameSite=Strict`。若部署拓扑变为 cross-site，必须在开放 CORS 或 cookie 跨站传递前重开 CSRF 决策。

session request body 是硬编码 security limit：Rust 常量 `SESSION_REQUEST_BODY_LIMIT_BYTES = 4096`，只通过 axum `DefaultBodyLimit::max` 应用于 session `MethodRouter`，不得成为全局 router default，也不得进入 `config.toml`。`putCommand` 按同一纪律声明自己的 route 级常量 `COMMAND_REQUEST_BODY_LIMIT_BYTES = 16_384`（2026-08-16，Phase 4 WP1；payload 族均为数百字节级 typed 对象，16 KiB 提供充分余量）。真实 login body 约 100 bytes，4 KiB 提供约 40 倍余量；超限必须在 Argon2 verification 与任何数据库访问前返回 `413`。未来 CSV import 等 route 必须声明自己的 limit，不继承此值；把 security limit 配置化会产生例如放宽到 1 GiB 的 fail-open surface。

header count/size 与 slow-header protection 在 Stage 4 **仍未关闭**。已核实 axum `Serve` 只暴露 `local_addr` 与 `with_graceful_shutdown`，不能取得 hyper HTTP/1 builder；`max_headers`、`max_buf_size`、`header_read_timeout` 只存在于 `hyper::server::conn::http1::Builder`。设置它们需要自建 accept loop、graceful shutdown，并直接增加 `hyper` / `hyper-util`；Stage 4 保留已有 evidence 的 `axum::serve` listener/shutdown 路径。hyper 当前的 transport implementation property 是 `max_headers` 默认 100、`max_buf_size` 约 400 KB、超限返回 `431`，但 hyper 明示这些默认值不稳定，因此它们**不是**冻结的 Natsume contract；slow-header read timeout 同属此 gap。该 gap 不得无限期携带：header count/size、slow-header timeout 与 connection capacity 必须在 **Phase 4 显式定案**——或以 `hyper::server::conn::http1::Builder` 的 limit 自建 accept loop，或记录一份带部署证据的、经评审的接受结论。

connection count 是 availability capacity，不是可脱离部署证据硬编码的 security constant。同一 TCP port 后续还承载 Device WSS（[ADR-0033](adr/0033-enrollment-and-device-control-boundary.md)）；按约 3 个 operator browser 选择的值会在 Device fleet 接入后过小。它依赖 S0-4 device-fleet evidence，保持 `ENV-UNFROZEN`；Stage 4 不为 connection capacity 增加 rate limiter、worker pool 或 connection manager。

password-verification 并发是独立资源，不属于上述 capacity：Device Enrollment 与 Device WSS 都不执行 password verification（见 [架构 §5](architecture.md)），故 device fleet 规模不改变其上界；它由已冻结常量完全确定（§3.6.3 的 `m=19456 KiB` 与本节记述的约 3 个 operator browser）。因此它与 session body limit 同级，是硬编码 security limit：Rust 常量 `PASSWORD_VERIFICATION_CONCURRENCY = 4`，以进程内 semaphore 施加于 Argon2 verification，不进入 `config.toml`。

因此 Gate 4 不能标记为完整 `PASS`：session body limit 与 password-verification 并发已关闭，header/slow-header 与 connection capacity 仍开放。

**2026-08-16 修订（Phase 4 WP2 定案）**：上段 gap 以选项一关闭——Server 以 `hyper::server::conn::http1::Builder` 自建 accept loop（保留既有 TLS listener 与 graceful shutdown 语义，`with_upgrades` 为 Device WSS 预留同一 loop）。冻结的硬编码常量：`HTTP_MAX_HEADER_COUNT = 64`、`HTTP_HEADER_READ_TIMEOUT_SECONDS = 10`（slow-header 防护）、`HTTP_MAX_BUF_SIZE_BYTES = 65_536`（request-line/header 缓冲上限）、`MAX_CONCURRENT_CONNECTIONS = 2_048`（accept 级 semaphore，permit 先于 accept——满载时停止 accept 而非 accept 后关闭）。四者按本节既有纪律不进 `config.toml`。行为边界：超 header 数与超 header 缓冲均由 hyper 返回 `431` 后关闭连接；header 读超时为 transport 级连接关闭——三者与 oversized WSS frame 同类，不进稳定 ErrorCode 表。派生语义：hyper 对每个 message head 武装该读超时，因此它同时构成约 10 秒的 keep-alive idle 超时（客户端连接池自愈；WSS upgrade 完成后脱离 HTTP/1 连接语义，不受影响）。排空无上界（与先前 `axum::serve` 语义对等），进程级兜底为 systemd `TimeoutStopSec`。容量值由 500 台 fleet + 重连风暴 + 约 3 operator browser 推得（约 4× 余量）；缩比验证归 G4 容量探针，完整 500 台验证仍在 G7。至此本节五项 ingress 决策（session body limit、password-verification 并发、header count/size、slow-header timeout、connection capacity）全部关闭，上段「Gate 4 不能标记为完整 PASS」的旧结论随本修订作废。

与 §12 oversized WSS frame 的规则相同，oversized session body 是 transport ingress resource-limit failure：body-limit layer 在 HTTP adapter 前直接返回 `413`，不进入 stable ErrorCode mapping table。

失败登录按 source IP 限流，遵循同一 transport ingress resource-limit 模式：limiter 在 Argon2 verification **之前**生效，超限直接返回 `429`，同样不进入 stable ErrorCode mapping table。阈值与 limiter window 是硬编码 Rust security 常量，理由与 `SESSION_REQUEST_BODY_LIMIT_BYTES` 相同——把 security limit 配置化会产生 fail-open surface，因此它们不进入 `config.toml`；具体数值在实现时选定，并按同一「文档化常量」纪律记入本节。跨越阈值时按 §3.6.4 每个 limiter window 写一条 audit row，其余失败登录零 audit 写入。

## 4. Enrollment

Enrollment 使用 server-auth HTTPS：Client 必须验证预配置 Server trust 和 IP-SAN/endpoint；**不使用 TOFU 或 dangerous verifier**；请求有严格大小和速率限制；与 operator API 共享进程但使用独立路由、授权和限流。

**窗口门禁**：仅在 `provisioning_window.state = 'open'` 时受理（[ADR-0033](adr/0033-enrollment-and-device-control-boundary.md)）；singleton 只有 `state`、`revision` 和 `last_audit_event_id`。窗口关闭时以稳定 ErrorCode 拒绝且零状态变更；restart/restore 的 open→closed recovery 是 close-once audited CAS，不是历史状态 replay。

**请求**包含 `enrollment_requests` 所需的 `machine_hardware_id`、`hardware_identity_quality`、`gateway_csr_der`、`gateway_spki_sha256`、`client_version` 和 `protocol_version`；Server 记录 `source_ip`、state、可选 resolution/resolved device/issuance audit 和 `created_at`。**不得包含**：Caddy config、DOMjudge password、任意 certificate profile、任意路径/unit/shell。

**响应**可携带 `device_pk`、Device Token、Gateway leaf + chain；持久化只记录 `devices`、`enrollment_requests`、`device_tokens`、`gateway_certificates` 与 `audit_events` 的 migration-defined facts。`gateway_certificates` 不保存 leaf/chain bytes，失败无半成品。

**签发与审批的非对称语义**（[ADR-0033](adr/0033-enrollment-and-device-control-boundary.md)）：`resolution = 'create_device'`（未知 `machine_hardware_id` 的首次 Enrollment）在窗口内以同一事务同步签发并直接返回结果，**不需要 operator 审批**。`resolution = 'replace_device_credentials'` 只适用于当前 state 为 `enrolled` 的既有 Device，且 different-SPKI 必须经 operator 显式审批；唯一例外是请求的 `gateway_spki_sha256` 与该 Device 当前 `issued` request 的 SPKI 相同——此时自动批准，因为持有同一 private key 证明是同一台机器在 finalization 失败后重试。窗口关闭与 hardware ID 最新 request 已 rejected 的门禁保持既有优先级；通过这两项后，`disabled` / `revoked` Device 的 same-SPKI、different-SPKI 新 intake、live replay 与 approved claim 均返回 `409` / `DEVICE_IDENTITY_CONFLICT`，零写入、零签名、零审计，且 state 不变；Enrollment 不再隐式恢复 Device。

**替换语义**：经批准的替换只可关联 state 为 `enrolled` 的既有 `device_pk`；替换 `device_tokens.token_hash` 并签发新的 certificate metadata，作为 re-enrollment 审计；`issue_device_credentials` audit 的 `previous_device_state` 只可为 `enrolled`（首次创建为 `null`）。若旧连接仍存活，记录异常审计事件。不存在 Enrollment-owned enable/reactivate API 或 state restore 分支。

**审批与 claim**：需要审批的 Enrollment 请求以 HTTP `202` 与 typed 非错误 body（request identity 与 state）应答，不携带任何签发结果。Device 通过幂等重投**同一** request 轮询：相同 `machine_hardware_id` + `gateway_spki_sha256` 返回同一条 live request，不新增 row。operator 批准前重新读取 request resolved Device 的当前 state；非 `enrolled` 时在 audit/CAS 之前返回现有 `RequestNotPending` 分类（HTTP `400` / `ENROLLMENT_REQUEST_INVALID`），零写入。operator reject 不读取 Device state，残留 `pending` request 仍可被拒绝。成功批准把 persisted state 置为 `approved`，受审计且零签发；该 state 不作为 wire value 返回——观察到 `approved` 的下一次重投仅在 Device 仍为 `enrolled` 时同步执行签发事务并返回 `201`，否则按上段返回 identity conflict。只要 rejected row 仍是此 hardware ID 的最新 request，同一窗口内该 hardware ID 的任何 SPKI（包括新 key）都返回 `ENROLLMENT_REQUEST_REJECTED` 且零写入。窗口关闭把 `pending` / `approved` / `rejected` 转为 `expired`，因此下一次开窗不再受旧 rejection 阻断；这是无 window-ID column 时冻结的 current-window scoping rule。claim 响应的 Token 与 Gateway leaf 明文只存在于那一次响应，数据库始终只保存 hash。claim 前必须重新校验窗口仍为 `open`（`INV-CERT-01`：窗口外不存在签发路径）。claim 响应丢失时，仍为 `enrolled` 的 Device 重试落入 same-SPKI 自动批准路径重新签发。live `pending`/`approved` request 存在期间，同一 hardware ID 上 SPKI **不同**的提交以稳定 ErrorCode 拒绝；operator 必须先拒绝现有请求。

**2026-08-16 修订（Phase 3 WP2c operator decision precision）**：approve/reject path 的 `request_id` 必须是 canonical lowercase hyphenated UUIDv7。`pending → approved` 与 `pending → rejected` 写 `result = 'succeeded'` / `reason_code = 'operator_requested'` audit；已为目标 state 的 re-approve / re-reject 是零业务写入的 repeat-safe `noop`，仍写一行 `reason_code = 'target_already_satisfied'` audit 并返回 `200 {enrollment_request_id,state}`。`approved → reject`、`rejected → approve`、任意 terminal request、未知 request 或非 canonical ID 都不是 noop，统一返回 `400` / `ENROLLMENT_REQUEST_INVALID` 且零写入，避免 actionability / existence oracle。operator list 只返回 `pending` / `approved`，字段固定为 request ID、hardware ID/quality、SPKI digest、client/protocol、state、nullable resolution/resolved Device、created-at 与 source IP；不得返回 `gateway_csr_der`。

**2026-08-16 修订（Phase 3 WP2b intake capacity）**：全局 live Enrollment request（persisted state 为 `pending` / `approved`）上限固定为硬编码 `MAX_LIVE_ENROLLMENT_REQUESTS = 600`，对应 500-seat fleet 加运维余量，不进入配置。检查位于 `BEGIN IMMEDIATE` intake transaction 内、发生在任何新 request row 写入前；达到上限后新的 intake 返回 `400` / `ENROLLMENT_REQUEST_INVALID` 且零写入，已有 live request 的幂等 replay 与 approved claim 仍可返回或排空既有 row。

**Client 收尾**：leaf 与本地私钥 SPKI 匹配、chain 通到预置 origin CA、SAN 等于配置 hostname、本地持久化原子完成后才提交结果；**中途失败不得留下“看似已 Enrollment”的半状态**（重试自然落入替换语义）。

**2026-08-16 修订（Phase 3 WP2b wire precision）**：device intake 固定为 `POST /api/v2/enrollment-requests`、`Content-Type: application/json`，无需 operator session；route 级硬编码上限为 `ENROLLMENT_REQUEST_BODY_LIMIT_BYTES = 65_536`，只作用于该 `MethodRouter`，不进入 `config.toml`，超限在 JSON/CSR 解析与任何数据库访问前返回 transport-level `413`。request 是拒绝未知字段的 closed object；`machine_hardware_id` 为 canonical lowercase hyphenated UUIDv5，`hardware_identity_quality` 只允许 `strong` / `medium` / `weak`，`gateway_csr_der` 为 RFC 4648 padded base64 的 DER CSR，`gateway_spki_sha256` 为 lowercase hex 64，`client_version` 为 1–64 bytes ASCII graphic（`0x21`–`0x7e`），`protocol_version` 当前只允许 `1`。Server 必须验证 CSR signature/structure、直接从 parsed CSR 的 raw `SubjectPublicKeyInfo` DER bytes 重算 SHA-256（不得重编码 SPKI）并与 claimed SPKI 常数内容比较；mismatch 或 forged signature 返回 `400` / `ENROLLMENT_REQUEST_INVALID` 且零写入。

**2026-08-16 修订（Phase 3 WP4 device quality claim）**：device 报告的 `hardware_identity_quality` 冻结为 sanitized claim 中全部 present 槽 quality 的最小值（序 `weak < medium < strong`）；该值仅供 operator review 展示，不参与任何服务端门控（与 ADR-0032 R4 的 per-slot quality 及 claim 层「quality 不参与判定」一致）。

成功同步签发返回 `201` 与 closed body `{enrollment_request_id, state:"issued", device_id, device_token, gateway_leaf_der, gateway_chain_der}`：两个 ID 均为 canonical UUIDv7；32-byte Device Token 使用 43-character unpadded base64url，仅该响应携带；leaf 是 RFC 4648 padded base64 DER，chain array 以相同编码从 leaf 的直接 issuer 排到 root，因此与独立 leaf 合并后的验证顺序固定为 leaf→root。待审批或 pending 轮询返回 `202` 与 closed body `{enrollment_request_id, state:"pending"}` 且不携带签发材料；persisted `approved` 由下一次同 request POST 直接消费为 `201`，不是可达 wire value。公开失败映射固定为：无效 request/CSR/SPKI/protocol、live capacity 满或已存在但不可 action 的 decision target → `400` / `ENROLLMENT_REQUEST_INVALID`；窗口非 open → `409` / `PROVISIONING_WINDOW_CLOSED`；hardware ID 当前窗口已拒绝 → `409` / `ENROLLMENT_REQUEST_REJECTED`；同 hardware ID 的 live different-SPKI、其他 identity facts 冲突或 resolved Device state 非 `enrolled` → `409` / `DEVICE_IDENTITY_CONFLICT`；未分类内部/持久化/签发失败 → `500` / `INTERNAL_ERROR`。所有结果含 Server-owned canonical UUIDv7 `X-Correlation-Id`；`201` / `202` body 也不得回显 CSR authority fields。

## 5. Device control：WSS

- **传输**：WebSocket over server-auth TLS；Protobuf 消息作为 tungstenite 重组后的 WS binary message（无自定义 length-prefix framing）；单一 `Sec-WebSocket-Protocol` 为 `natsume.control`，不匹配在 upgrade 拒绝；
- **认证**：Device Token 必须由 CSPRNG 生成 32 bytes；upgrade 时经 `Authorization: Bearer <Device Token>` 提交；Server 常数时间比对 `device_tokens.token_hash`，并在同一 typed read model 中解析 resolved `devices.state`；只有 state 为 `enrolled` 才可继续。**无 token / malformed token / 错误 token / 不再有对应 token row / token row 对应 Device state 非 `enrolled` → 同一 `401` body 与内部 cause，发生在任何 Protobuf 解码之前并计入同一 IP limiter**；非法持久化 state 是 corruption，保持 `500`；
- TLS early data（0-RTT）保持关闭；认证失败按 IP 限流；
- Frame 必须有明确最大长度、封闭 envelope kind 和 command/correlation ID；超限 frame、未知版本、非法 oneof 必须关闭连接，**不得猜测**；
- `Command.command_id` 和 `CommandStatus.command_id` 必须验证为 canonical UUIDv7，并将 HTTP path 的同一字符序列原样带入和带回；WSS/Device 不得另行生成或格式化 ID，且 validation error 不回显原 ID；
- keep-alive 使用 WS ping/pong；连接中断不改变 Server truth；重连后通过 direct durable Command 和 Observed 收敛。

## 6. Command 契约

Command current row 直接绑定 `command_id`、`device_pk`、`kind`、`state`、request fingerprint version/hash、可选 `group_correlation_id`、`payload_version`、`frozen_payload_json`、`created_at`、`deadline_at`、可选 terminal error/result 和 `created_audit_event_id`。它不保存秘密 payload copy、certificate 或 token issuance 数据；每 Command 的 frozen typed input 只在 `frozen_payload_json` 内表示。

`commands.state` 的取值集合随 Phase 4 状态机冻结；在此之前该列无 CHECK，该缺口随状态机冻结时一并关闭。

V2 业务 family 限于：`SYNC_STATE`、`SYNC_SECRET`、`OPEN_BINDING_PROMPT`、`SESSION_LOCK`/`UNLOCK`/`TERMINATE`、`HOME_RESET`（具体枚举以 `.proto` 为准；新增 family 必须证明不是任意远程管理能力）。wire/DB `kind` 值即 proto oneof 字段名（lower snake，例如 `SESSION_LOCK`↔`lock_session`、`HOME_RESET`↔`reset_home`），文档族名为其 UPPER_SNAKE 标签。`OPEN_BINDING_PROMPT` 只携带 expiry 与 message-catalog ID 两个 typed 字段，并且只驱动封闭的 binding-prompt screen，因此不构成任意远程管理能力。**Command receipt 在 Device durable 持久化前不得确认**，且只表示“已可靠接收”不表示“已成功执行”。终态由可选 `terminal_error_code` 或 `redacted_terminal_result_json` 表示（[ADR-0034](adr/0034-state-execution-and-data-plane-boundary.md)）。

Device journal 保存收到的 Command frame bytes；相同 `command_id` 且 frame bytes 相同时不重复副作用，相同 ID 但 frame bytes 不同时以 `COMMAND_PAYLOAD_CONFLICT` 拒绝。Server 必须从已存储的 frozen payload 确定性渲染给定 `command_id` 的 wire Command，使每次重新投递的 frame byte-identical。Server-side `COMMAND_REQUEST_CONFLICT` fingerprint predicate 保持权威；依照 §3.5 的 payload 单一 JCS 规范形不变量，同 fingerprint 蕴含 byte-identical frame，Device predicate 只会在 Server 不变量被破坏时触发。Server DB、WSS Command、journal、CommandStatus 和 audit correlation 的 ID 关联不可断开。

## 7. `SYNC_STATE`

`SYNC_STATE` payload 只携带非秘密、封闭的 Target snapshot 或其 typed plan。Device 必须验证 target Device、baseline revision 与派生代际、command freshness、本地 identity、payload schema/version，以及所有派生 hostname/upstream 均来自允许集合。应用失败必须保留已验证 LKG 或进入 BLOCKED（详见 [状态与执行模型](state-and-execution.md)）。

**`SYNC_STATE` 不签发、不携带、不安装任何证书或 token**；Gateway certificate 只在 Enrollment 获得（`INV-CERT-01`）。

## 8. `SYNC_SECRET`

Payload 在传输和内存中使用秘密专用类型。Device 必须在写入前重新验证当前 binding 和 revision；**陈旧 secret 不得安装**；凭据文件更新原子，不留半写；随后由 Daemon 重渲染含凭据的 Caddy `/login` 注入配置并原子激活（[ADR-0034](adr/0034-state-execution-and-data-plane-boundary.md)）。成功结果只报告已安装 credential revision、redacted status 和 audit correlation；**不得回显 password**。

## 9. Observed snapshot

Observed 使用完整或可证明合并的 typed snapshot，**不使用自由格式 status map**；每个维度独立表达状态与有限诊断码，**密码值、token 值、private key、完整路径和内部异常链不得出现**。上报节奏为**变化时上报 + 低频周期兜底**（带宽约束，ADR-0030 F2）。Server 只接受当前 authenticated Device 的 snapshot，校验单调 sequence、合法大小和 schema；**不能把 Device 自报字段直接当作授权**。

## 10. Local D-Bus

**Device Daemon ↔ Session Agent**：UI snapshot 只含展示所需数据（view kind、Seat code、binding 状态、session epoch 等），**不含 password、token、certificate private material、Server 凭据或任意 HTML**。view kind 与 action 为封闭 enum，经版本升级路径扩展（[ADR-0035](adr/0035-session-home-and-desktop-cycle.md)）。调用校验 UID/PID/logind session 和 current epoch；陈旧 epoch 重放被拒绝；Agent 退出导致 lease 过期，不授予额外权限；**lock/unlock 不调用 Caddy adapter**。

**Device Daemon ↔ Privileged Helper**：Helper 方法按 capability 命名，参数必须是封闭 enum、规范化 ID、Helper 内重新派生或 allowlist 校验的路径/UID、明确 epoch，**无 secret**。**Helper 不接受 Server/WSS request 的原始对象。**

## 11. Caddy 控制契约

Device Daemon **不发送任意 Caddyfile、不使用 Caddy Admin API**。控制路径为（[ADR-0034](adr/0034-state-execution-and-data-plane-boundary.md)）：

```text
已验证 Target + 本地证书/凭据材料
  → Daemon 渲染完整配置文件（固定 loopback listen、固定 hostname、固定 DOMjudge upstream、
     固定 TLS material 引用、固定 BLOCKED/READY route 集、仅 /login 的 header 注入）
  → caddy validate
  → 原子替换配置文件（temp + fsync + rename）
  → systemd path unit 触发 reload
  → 本地健康检查；失败回滚 LKG 配置文件
```

执行前必须验证证书/私钥匹配与 SAN/有效期；**未验证配置不得激活**。含凭据的渲染配置是 secret artifact（`0640 natsume:natsume-gateway`）。`Accept-Encoding` 保持透传，不配置 `encode`（brotli 在 upstream 完成，ADR-0030 F5）。Session lock/unlock contract 不包含任何 Caddy 字段。

## 12. Stable ErrorCode

依赖方向：`DomainError → exhaustive adapter mapping → stable ErrorCode → HTTP/Protobuf/D-Bus/CommandStatus`。**禁止 `stable ErrorCode → domain decision`。**

规则：字符串值显式定义；每个公开 adapter 映射穷举；未分类内部错误映射到有限通用码；`detail` 默认无或脱敏；新内部错误不自动成为新稳定码；删除稳定码需要兼容计划；**Web/Device 不解析 Display 文本**；同一语义跨 transport 使用同一稳定码。实现级 variant 不直接进入 registry，只有调用方可稳定处理、会跨公开边界或持久化到 `CommandStatus`/Observed 的语义才进入下表。

| 类别 | 稳定码 | 当前公开语义 |
|---|---|---|
| common | `INTERNAL_ERROR` | adapter 穷举分类后仍无法安全公开具体语义的内部失败；`detail` 默认缺失，不回显 source chain。 |
| common | `INVALID_REQUEST` | 未被更具体稳定码覆盖的闭合结构或参数校验失败。 |
| common | `RESOURCE_NOT_FOUND` | request 结构合法，但其引用的当前事实（Device、candidate、Enrollment request、Command 等）不存在。对全部资源使用同一个通用码，resource type 由 route/上下文决定；可跨 HTTP/Protobuf/D-Bus/CommandStatus 使用。 |
| common | `AUTHENTICATION_FAILED` | Operator session 或 Device Token 认证失败；不得公开区分 missing、malformed、wrong、no-row、disabled、revoked 等可形成 oracle 的原因。Device WSS upgrade 在解码前返回同体 `401`；保留 token row 的 disabled Device 也不获得 WSS authority。 |
| common | `AUTHORIZATION_DENIED` | 已识别调用方无权执行操作，包括 Operator role 与本地 Helper policy 拒绝。 |
| operator | `IMPORT_CANDIDATE_INVALID` | Import candidate 结构无效、重复 account、为空或仅含 header；不得持久化 candidate 或改变 confirmed truth。 |
| operator | `IMPORT_CANDIDATE_PENDING` | singleton pending candidate 已存在，新 upload 必须先显式终止或完成既有 candidate。 |
| operator | `IMPORT_CANDIDATE_UNAVAILABLE` | candidate 已过期、discard、删除或 preview token 不匹配；token 不匹配与不存在必须不可区分。调用方必须重新创建 candidate。不含非秘密 fingerprint 不一致。 |
| operator | `IMPORT_SEATS_STILL_BOUND` | Import Commit 将删除的座位当前仍有 Binding；拒绝、零 Binding 写入、零 confirmed-state 变更；operator 须先经 Binding API 解绑再重新 preview。 |
| operator | `IMPORT_CANDIDATE_MISMATCH` | Import Commit 重提交 CSV 的非秘密部分与 pending candidate 的 fingerprint 不一致；拒绝、零写入；candidate 保留至 discard/expiry/成功 commit。不得使用 `IMPORT_CANDIDATE_UNAVAILABLE`（存在性 oracle 折叠）。 |
| enrollment | `PROVISIONING_WINDOW_CLOSED` | provisioning window 非 open 时拒绝 Enrollment，零签发、零 Server-state 变更。 |
| enrollment | `ENROLLMENT_REQUEST_INVALID` | Enrollment 的有界 typed request、CSR/SPKI 或协议输入无效；不得留下部分 issuance。 |
| enrollment | `ENROLLMENT_REQUEST_REJECTED` | operator 显式拒绝了该 Enrollment request；Device 必须停止并等待现场人员介入，零签发。 |
| enrollment | `DEVICE_IDENTITY_CONFLICT` | 硬件身份冲突要求人工恢复；不得自动 merge、选择候选或删除凭据。 |
| control | `COMMAND_ID_INVALID` | `command_id` 不是 canonical lowercase hyphenated UUIDv7；HTTP 为 `400`，不得回显原始 ID。 |
| control | `COMMAND_REQUEST_CONFLICT` | 已有 `command_id` 与当前 versioned canonical request fingerprint 不同；HTTP 为 `409`，不得覆写既有 Command。 |
| control | `PROTOCOL_VERSION_UNSUPPORTED` | WSS subprotocol 或 typed control protocol version 不受支持；拒绝协商，不猜测兼容。 |
| control | `PROTOCOL_INVALID_ENVELOPE` | 已接收的 typed control envelope 含未知/非法 kind、oneof 或结构；关闭连接，不解析 Display 文本。 |
| control | `COMMAND_PAYLOAD_CONFLICT` | Device journal 已保存相同 `command_id` 的 Command frame bytes，但后续收到的 frame bytes 不同；拒绝且不产生第二次副作用。Server 对该 ID 的确定性重新投递必须 byte-identical。 |
| control | `COMMAND_PAYLOAD_INVALID` | Command 的 payload version、typed schema、target 或允许集合校验失败，且不属于 stale current-fact。 |
| control | `COMMAND_STALE` | 执行前或关键原子提交前发现 binding/configuration/credential 或 Command generation current fact 陈旧；拒绝且不部分应用。SessionEpoch/HomeEpoch 陈旧无论经 D-Bus、WSS 或 `CommandStatus` 暴露，始终分别映射为 `SESSION_CONTEXT_STALE` / `HOME_EPOCH_STALE`。 |
| device | `DEVICE_IDENTITY_UNAVAILABLE` | identity-bound 产物存在但当前硬件身份无法获得或有效来源不足；Device fail closed。 |
| device | `DEVICE_IDENTITY_MISMATCH` | 当前硬件身份与持久化身份不匹配；不得使用凭据或自动 re-enroll。 |
| device | `DEVICE_CREDENTIALS_UNREADABLE` | identity-bound 凭据文件损坏或不可安全读取；不得自动重建。 |
| device | `GATEWAY_CREDENTIAL_INVALID` | Gateway certificate/private key 的 SPKI、chain、SAN 或有效期验证失败；不得激活未验证配置。 |
| device | `GATEWAY_CREDENTIAL_INSTALL_FAILED` | Enrollment 收尾时 Gateway credential 无法原子持久化；不得留下“看似已 Enrollment”的半状态。 |
| device | `GATEWAY_ACTIVATION_FAILED` | Caddy candidate validate、reload、health 或 LKG recovery 无法安全完成；保留已验证 LKG 或进入 BLOCKED。 |
| device | `GATEWAY_UPSTREAM_TLS_REQUIRED` | fixed DOMjudge upstream（至少 `/login`）不满足 TLS policy；不得激活明文凭据注入。 |
| device | `SECRET_INSTALL_FAILED` | `SYNC_SECRET` 无法原子写入或激活凭据；保留旧 secret 或明确标记不可用，不留半写。 |
| session | `SESSION_CONTEXT_STALE` | SessionEpoch、Agent lease、logind/boot/session identity 或 UI action 已陈旧；拒绝控制 replacement session。 |
| session | `SESSION_UNAVAILABLE` | 当前 graphical session/Agent/display 不存在、不唯一或不满足受管操作条件。 |
| session | `SESSION_ACTION_UNSUPPORTED` | 当期冻结镜像不支持请求的 native session lock/unlock/terminate capability。 |
| session | `SESSION_STATE_CONFLICT` | 请求动作与当前 session/lock state 不一致，例如无 active lock 或 command/state 不匹配；调用方应刷新 typed state。 |
| home | `HOME_EPOCH_STALE` | `HOME_RESET` 携带的 `home_epoch` 不大于 Device 当前 epoch（非单调）；拒绝且零副作用。 |
| home | `HOME_OPERATION_FAILED` | 无法证明 mount/copy/ownership 安全，或 Home reset 无法可恢复地完成；fail closed，不启动受管 session。 |

WSS frame size 仍必须有明确上限和负向测试，但 oversized frame 是 transport ingress resource-limit failure：直接关闭连接，不进入稳定 ErrorCode registry。该 catalog 由 `natsume-error-code` crate 实现；每个 variant 使用显式 Serde rename 实现稳定字符串 `Serialize`/`Deserialize`，不得使用 `rename_all`、手写字符串 parser 或从 Rust variant 名推导 wire value。独立 registry 的治理决策已由 [ADR-0036](adr/0036-error-architecture-and-public-codes.md) 接受，实施完成度仍以 Gate evidence 为准。`RESOURCE_NOT_FOUND` 与 `ENROLLMENT_REQUEST_REJECTED` 的新增发生在 ADR-0036 所述的 coordinated pre-release baseline window 内（G0 仍为 `OPEN`），因此暂不需要 §13 的兼容性计划；该 window 关闭后，任何删除或语义变更仍受 §13 约束。

## 13. 版本和兼容

已发布的 field number、interface name、method/signal/error name、ID 和 revision 语义**不复用、不被数据迁移重写**；破坏性 wire 变化使用新 WS subprotocol 或 interface version；downgrade/rollback 通过发布 runbook 定义，不假设 schema 自动回滚。

## 14. 契约验证

CI 必须证明：生成契约 clean diff；`PUT /api/v2/commands/{command_id}` 的 `201/200/400/409`、canonical UUIDv7 正/反例、same-ID/same-fingerprint replay 与 same-ID/different-fingerprint conflict、`request_fingerprint_*`/`frozen_payload_json` 持久化、ID 在 HTTP/WSS/journal/status/audit 的一致性；WS frame size/version/unknown enum 测试；窗口关闭时 Enrollment 拒绝且零变更；open-window recovery close-once；无 token upgrade 在解码前 401；D-Bus XML/Rust/policy 一致；ErrorCode 映射穷举；secret/path/source-chain redaction；`/login` 之外路由无注入头；Session lock contract 无 Caddy 字段；禁止通用执行能力。具体检查随对应 Phase 实现补全。
