# 术语表

本文件只定义术语，不定义行为。行为以对应规范文档为准。

2026-08-19 版本规则：当前 authority 仍是 Token/Bearer；ADR-0038 的单一 split Proto、generated messages、strict crypto 与 Prost typed canonicalization foundation 已存在，但标注“ADR-0038 目标”的词条不表示对应 runtime authority 已接线。

| 术语 | 定义 |
|---|---|
| **Server truth** | Server 业务数据库中已经提交的权威领域事实；业务数据只表达当前事实，必要历史证据由 AuditEvent 保存。 |
| **Target** | Server 从已提交事实计算出的、面向某台 Device 的非秘密期望状态。Target 本身不产生远端副作用。 |
| **Observed snapshot** | Device 报告的实际可观察状态。它是设备应用状态的唯一业务来源。 |
| **Drift** | Target 与最新有效 Observed snapshot 的差异。 |
| **current-fact（当前事实）** | 业务表所保存的当前权威事实；不可替代的历史仅限 redacted `AuditEvent`，以及在所属表段落显式声明了保留理由与消费者的终态行。 |
| **Identifier（surrogate 标识符）** | Server 或 Panel 生成、对外可见且不承载业务自然语义的标识符；本契约中的 `device_pk`、`operator_id`、`account_id`、`audit_event_id`、`vault_record_id`、`correlation_id`、`group_correlation_id`、`command_id` 与 `enrollment_request_id` 均以 canonical lowercase hyphenated UUIDv7 表示。`seat_id` 与 `machine_hardware_id` 是业务自然键，不属于该术语。 |
| **Command** | 面向单台 Device 的持久化、可重试远端意图。`commands` 只保存一个 current row；相同 ID 的重投递不会变成第二个业务执行。 |
| **Command ID / `command_id`** | Panel 在提交前生成的 canonical lowercase hyphenated UUIDv7。相同 ID 原样贯穿 HTTP、Server Command、WSS、Device journal、CommandStatus 和 per-Command audit correlation。 |
| **Canonical request fingerprint** | Server 对经 JCS（RFC 8785）规范化、versioned 且 domain-separated 的 canonical Command request 计算的 SHA-256，并持久化为 `request_fingerprint_version` 与 `request_fingerprint_sha256`；算法由[契约](contracts.md#command-request-fingerprint-v1)的「Command request fingerprint v1」小节冻结。它区分同一 `command_id` 的 replay 与冲突；不包含 frozen timestamps、actor、session 或 retry time。 |
| **domain-separated** | 哈希输入使用固定、版本化的域分隔前缀，与其他用途的输入空间隔离的约定。 |
| **`frozen_payload_json`** | `commands` 的 typed object column，保存通过 per-kind schema 验证的 payload 的 JCS（RFC 8785）规范形，并与 `payload_version` 一起构成每 Command 的 frozen payload；它替代多个 frozen top-level columns 或 dispatcher metadata。 |
| **Group correlation ID** | 可选、仅用于批量 Command 的查询和审计分组的关联值；不定义顺序、原子性、重试或跨 Device lifecycle。 |
| **Device** | 一台受管理工作站的业务实体；`devices` row 保存 `device_pk`、unique `machine_hardware_id`、`hardware_identity_quality`、`state` 与 transitional nullable `control_authority_revision`。Token-era row 保持 NULL；只有 flag-day 后的 control-key authority 才使用正 revision。 |
| **Device Token（当前实现）** | Enrollment 时 Server 生成的 32 字节不透明随机凭据；`device_tokens` row 以 `device_pk` 为键，保存唯一 `enrollment_request_id` 和 32-byte `token_hash`。ADR-0038 flag day 将删除它；dormant control foundation 不改变其当前 authority。 |
| **Control key（ADR-0038 目标）** | Device 专用 Ed25519 control signing key，与 Gateway TLS key 分离；protocol/PKCS#8 foundation 可以存在，但在 flag day 前不参与 Enrollment/WSS authority。Private key 只存在于 daemon-owned PKCS#8 文件，Server 只接收 public key。 |
| **ServerChallenge（ADR-0038 目标）** | HTTP 101 后由 connection-local PreAuthSession 发送的一次性随机 challenge；不持久化、不跨连接查询。 |
| **ClientProof（ADR-0038 目标）** | Device 对 Challenge、intent、control public key、Machine Hardware ID、attempt ID 与 exact ClientInit hash 的 Ed25519 proof。 |
| **ClientInit（ADR-0038 目标）** | Client 在 Proof 前构造并哈希、Proof 后逐字节发送的 standalone 帧；不是 active envelope。 |
| **CredentialBundle（ADR-0038 目标）** | Server 持久化并逐字节重放的 immutable public credential bundle；client durable CredentialAck 后才可激活对应 authority。 |
| **DeviceActor（ADR-0038 目标）** | 动态 registry 按需创建、进程内不淘汰的单 Device 排序点；Machine Hardware ID 只负责 shard routing，DeviceId 与 persisted key IDs 是 aliases。 |
| **Provisioning window** | `provisioning_window` 的当前 singleton 开关：`state`、`revision`、`last_audit_event_id`。开启期间 Enrollment 可签发 Device Token 与 Gateway certificate；关闭后不存在任何签发路径。恢复只会将已打开状态 close-once，不会自动打开。 |
| **close-once** | provisioning 窗口恢复语义：只关闭一次观察到的 `open` current-fact，后续 `closed` observation 不形成第二次恢复事件。 |
| **Gateway certificate** | provisioning 窗口内经 Enrollment 签发、供本机 Caddy loopback HTTPS 使用的证书；`gateway_certificates` 保存 `certificate_id`、`device_pk`、`enrollment_request_id`、serial、SPKI hash、not-after、status，而不保存 certificate body；私钥在 Client 本地生成且不离机。 |
| **Machine Hardware ID** | 按固定多源配方（ADR-0032）规范化并派生的稳定机器标识；不是认证凭据。 |
| **Fleet namespace UUID** | 站点级公开且不可变的 UUID，用于确定性派生 Machine Hardware ID。 |
| **Binding** | Seat 与 Device 的当前业务关联。`device_bindings` row 有 `seat_id`、unique `device_pk` 与正的 `binding_revision`；解绑通过删除关系表达。 |
| **Binding revision** | `device_bindings.binding_revision` 行级 stamp，供 `SYNC_SECRET` / Target / Observed 使用。仅 bind / unbind / rebind 铸造或更新受影响行；未变化 Binding 不重写。Import 不写入 Binding、不铸造该 stamp。它不表示 Seat→Account mapping。不存在全局 binding-set clock。 |
| **Seat→Account mapping** | `account_mappings` 中当前 confirmed contest configuration 内 Seat 与 Account 的一对一关系；没有 mapping row 表示 Seat 当前无 Account。由 Import Commit 写入，不铸造 Binding stamp。 |
| **Confirmed contest configuration** | Server 当前权威的 Seat 集合、Seat→Account mapping 和 credential metadata。没有永久冻结的 Seat universe、generic instance state 或可查询的历史 mapping/credential 版本；只能通过完整 candidate 的显式 Import Commit 被替换。 |
| **Contest configuration revision** | 已废止。不存在 `revision_counters` 或全局 configuration clock；Import 不以任何 revision CAS 保护 Seat/mapping。preview→commit 串行化只靠 singleton `pending_import_candidate`。 |
| **Credential revision** | `accounts.credential_revision`：某 Account 当前秘密的单调修订。任何已提交的 Import Commit 都无条件替换该 Account 的当前 ciphertext 并推进此修订，不做明文比较。每个 Account 只有其关联的 current vault ciphertext，不保留可寻址的旧密码版本。 |
| **Candidate import** | 单次严格 CSV upload 的完整、非秘密 pending 草稿；全局同一时刻最多一个。`pending_import_candidate` singleton 只保存 candidate ID、expiry、preview-token hash、nonsecret fingerprint 和 redacted preview；不保存密码或 encrypted CSV。终止时只删除该 row，只保留 redacted audit lineage。 |
| **Import preview / import diff** | Server 对 candidate 与 confirmed baseline 的 redacted 结构化比较结果。Server 是 classification 的唯一权威。 |
| **Import Commit** | Operator 对 candidate 的显式二次确认动作；请求体再次提交同一 CSV，`preview_token` 经 header `Natsume-Preview-Token`。不对任何 revision 做 CAS；先常量时间比对非秘密 fingerprint，不一致则 `IMPORT_CANDIDATE_MISMATCH` 且零写入、candidate 保留。原子应用 Seat 集合、Seat→Account mapping 与 credential。不写入 Binding、不铸造 Binding stamp；将删座位仍绑定则拒绝。no-op 仅 lineage/redacted audit。material/no-op 只在非秘密维度上由 seats/mappings diff 定义：任何已提交的 commit 都推进每个 Account 的 credential revision。不自动产生 Device Command。 |
| **Preview token** | Server 签发的 opaque 证据；`pending_import_candidate` 只保存其 `preview_token_hash`，并保存 candidate identity、非秘密 fingerprint、redacted preview 与 expiry。commit 时作为 HTTP header `Natsume-Preview-Token` 回传，不得放入 query string。 |
| **Import Discard** | Operator 显式终止尚未提交的 candidate：在同一事务删除 `pending_import_candidate`，并保留 redacted audit；不触及 vault，不改变 confirmed configuration、binding、revision 或 Target。 |
| **Session epoch** | 当前受管桌面会话的身份代际；会话操作必须绑定该 epoch。 |
| **Home epoch** | 每次 `HOME_RESET` 由 Server 分配的严格单调身份代际；不得跨 epoch 复用未证明安全的结果。 |
| **Client 凭据文件** | Device 本地 service-user-owned 权限文件：Device Token、Seat 凭据、identity record、dormant `control-key-1.pk8` 与 control manifest 为 `0600 natsume:natsume`，Gateway key/leaf 与含凭据 Caddy 配置为 `0640 natsume:natsume-gateway`；全部原子写，无应用层加密（ADR-0032）。 |
| **Server vault** | `server_vault_records` 中的应用层加密秘密存储，只保存当前 Account 的 DOMjudge 密码。每个 `account_id` 只有一个 row；row 只有 `vault_record_id`、`account_id`、nonce 和 ciphertext，不含 `record_type`/`subject_id`、format/key/AAD version 或 timestamp。 |
| **AuditEvent** | 具有 `audit_event_id`、由 audited guarded operation 自行插入并与敏感领域 mutation 原子提交的 redacted 证据；fresh ID 可作为 typed operation input，但已持久化的同 ID 或预插入 audit row 不能重放为新 mutation 的依据。其 envelope 只有 occurred-at、actor、action kind、resource type/optional ID、result、optional reason code、correlation ID、optional group correlation ID 和 typed `redacted_detail_json`；revision/count 等 event-specific detail 只在该 JSON 内。 |
| **guarded operation** | 具有显式 guard，并在一个 transaction 内原子提交敏感领域 mutation 与其 typed `AuditEvent` 的领域操作。 |
| **`system:recovery`** | [审计词汇注册表](contracts.md#当前-auditevent-词汇注册表)中表示启动或恢复路径的系统 actor 值。 |
| **`system:password-reset`** | [审计词汇注册表](contracts.md#当前-auditevent-词汇注册表)中表示离线 operator password reset 路径的系统 actor 值。 |
| **LKG** | Last Known Good，本地最后一次已验证可用的配置或证书集合。 |
| **Caddy BLOCKED** | Caddy 仅提供有限本地状态页，不代理 DOMjudge 的 fail-closed 状态。 |
| **Caddy READY** | Gateway 证书、配置和目标 upstream 都通过验证后激活的数据面状态。 |
| **Enrollment** | provisioning 窗口内的 server-auth HTTPS 注册流程。`enrollment_requests` 记录硬件、CSR/SPKI、client/protocol/source-IP、state、可选 resolution/resolved device/issuance audit 与 created-at；`device_tokens` 和 `gateway_certificates` 各自以唯一 Enrollment request 关联签发结果。重复注册为受审计的替换。 |
| **xheaders 自动登录** | DOMjudge 官方 X-Headers 认证：Caddy 仅在 `/login` 路由注入 `X-DOMjudge-Login` 与 base64 的 `X-DOMjudge-Pass`（ADR-0034）。 |
| **镜像升级重验清单** | 每次赛事镜像 bump 后必须重跑的桌面 capability 验证集（ADR-0035），维护于 supported-platform.md。 |
| **typed contract** | 输入集合封闭、字段和枚举明确、能够被穷举校验的接口契约。 |
| **evidence locator** | 能定位到可复现实验、CI、artifact、日志或签署记录的稳定引用。 |
| **fail closed** | 无法证明安全或一致时停止敏感动作，而不是猜测、降级或自动重建身份。 |
