# 术语表

本文件只定义术语，不定义行为。行为以对应规范文档为准。

2026-08-19 版本规则：当前 authority 仍是 Token/Bearer；ADR-0038 的单一 split Proto、定向 handshake/Active envelopes、strict crypto 与 Prost typed canonicalization foundation 已存在，但标注“ADR-0038 目标”的词条不表示对应 runtime authority 已接线。无 `ClientInit`、无 `ControlEnvelope`、无 Hello。

2026-08-20：Identifier 与持久化主键统一为 `device_id`（同一 UUIDv7 surrogate），不再使用 `device_pk`。`devices` 无 `control_authority_revision`；当前 control key 由 `device_control_keys.status = 'active'`（partial unique one-active-key）表达。持久化瞬间为 INTEGER UTC epoch milliseconds、列后缀 `_unix_ms`；HTTP JSON 已冻结时刻字段仍为 RFC 3339 UTC。`binding_id` UUID occupancy、vault `account_id` PK、无 `revision_counters`、无 `import_payload` 均保持。

2026-08-24：ADR-0038 目标新增统一人工审核 Enrollment 状态机和 `EnrollmentReviewStatus`，移除 transaction expiry 与 challenge ID；Command 双端由有界 single-consumer channel 严格串行；binding prompt/request 用每 active session 单 in-flight 关联；credential convergence 使用完整 `{binding_id,account_id,credential_revision}`。

| 术语 | 定义 |
|---|---|
| **Server truth** | Server 业务数据库中已经提交的权威领域事实；业务数据只表达当前事实，必要历史证据由 AuditEvent 保存。 |
| **Persisted instant** | 数据库中的瞬间时刻：INTEGER UTC epoch milliseconds，列名后缀 `_unix_ms`。不存 RFC 3339 字符串，也不用 `strftime` CHECK。HTTP JSON 已冻结字段仍用 RFC 3339 UTC（尾随 `Z`），由 chrono 在边界转换。 |
| **Target** | Server 从已提交事实计算出的、面向某台 Device 的非秘密期望状态。Target 本身不产生远端副作用。 |
| **Observed snapshot** | Device 报告的 slim typed 实际状态：`applied_hash`、`CredentialObservation{binding_id,account_id,credential_revision,state}`、`gateway_state`、`gateway_certificate_fingerprint`、`session_state`。无 password、无 `secret_state`、无 `STALE` variant。ADR-0038 Active admission 中，SessionReady 后第一条 Active packet 必须是 Observed；旧数据库 row 只是 last-known evidence。 |
| **Drift** | Target 与最新有效 Observed snapshot 的差异。 |
| **current-fact（当前事实）** | 业务表所保存的当前权威事实；不可替代的历史仅限 redacted `AuditEvent`，以及在所属表段落显式声明了保留理由与消费者的终态行。 |
| **Identifier（surrogate 标识符）** | Server 或 Panel 生成、对外可见且不承载业务自然语义的标识符；本契约中的 `device_id`、`operator_id`、`account_id`、`audit_event_id`、`correlation_id`、`group_correlation_id`、`command_id`、`enrollment_request_id` 与 `binding_id` 均以 canonical lowercase hyphenated UUIDv7 表示。`seat_id` 与 `machine_hardware_id` 是业务自然键，不属于该术语。vault 不是独立资源，不存在 `vault_record_id`，按 `account_id` 与 `accounts` join。 |
| **Command** | 面向单台 Device 的显式意图，分 Converge（领域键幂等，Server 重推同一 payload）与 Oneshot（仅 live socket，离线丢弃）。Server DeviceActor 与 Client executor 都通过有界 single-consumer channel 严格按到达顺序执行，每连接最多一个 in-flight。`commands` current row 是 operator 审计；正确性不是 Client journal。 |
| **Command ID / `command_id`** | Panel 在提交前生成的 canonical lowercase hyphenated UUIDv7。相同 ID 原样贯穿 HTTP PUT、Server `commands` row、WSS `Command` 与 per-Command audit correlation。Device 不以该 ID 做 journal 去重。 |
| **Converge command** | `sync_state`、`sync_secret`、`reset_home`：按领域键幂等，Server 在 drift 时重推同一 payload；Device 无 command journal。 |
| **Oneshot command** | `lock_session`、`unlock_session`、`terminate_session`、`open_binding_prompt`：仅 live socket；离线丢弃；重连不重放。已产生本地副作用但断线丢失结果时，Server terminal state 为 `outcome_unknown`，不得自动重放；wire `CommandState` 仍只有 `SUCCEEDED` / `FAILED`。 |
| **OPEN_BINDING_PROMPT** | Oneshot Command，打开封闭 binding-prompt screen。空 body，无 TTL，无 `prompt_message_id`。`CommandStatus` `SUCCEEDED` 表示 screen 已打开，不是绑定已确认。Seat 在随后的 `BindingRequest.seat_code`。 |
| **BindingRequest** | Device 在 live Active socket 上由志愿者确认发起的绑定尝试，只携带 `seat_code`。不是 `OPEN_BINDING_PROMPT` 的 Command reply；每个 active session 同时最多一个 prompt/request，靠连接内顺序关联，无 `binding_request_id` 或 capability token。 |
| **BindingResult** | Server 对当前 in-flight `BindingRequest` 的应答（`state`、`error_code`）。不携带 request ID 或 occupancy `binding_id`；Device 从随后的显式 `SYNC_STATE` / `SYNC_SECRET` 得知该 stamp。 |
| **Canonical request fingerprint** | Server 对经 JCS（RFC 8785）规范化、versioned 且 domain-separated 的 canonical Command request 计算的 SHA-256，并持久化为 `request_fingerprint_version` 与 `request_fingerprint_sha256`；算法由[契约](contracts.md#command-request-fingerprint-v1)的「Command request fingerprint v1」小节冻结。它区分同一 `command_id` 的 replay 与冲突；不包含 frozen timestamps、actor、session 或 retry time。 |
| **domain-separated** | 哈希输入使用固定、版本化的域分隔前缀，与其他用途的输入空间隔离的约定。 |
| **`frozen_payload_json`** | `commands` 的 typed object column，保存通过 per-kind schema 验证的 payload 的 JCS（RFC 8785）规范形，并与 `payload_version` 一起构成每 Command 的 frozen payload；它替代多个 frozen top-level columns 或 dispatcher metadata。 |
| **Group correlation ID** | 可选、仅用于批量 Command 的查询和审计分组的关联值；不定义顺序、原子性、重试或跨 Device lifecycle。 |
| **Device** | 一台受管理工作站的业务实体；`devices` row 保存 `device_id`、unique `machine_hardware_id`、`hardware_identity_quality` 与 `state`。不存在 `devices.control_authority_revision`。当前 control key 是 `device_control_keys.status = 'active'` 的那一行（partial unique one-active-key）。 |
| **Device Token（当前实现）** | Enrollment 时 Server 生成的 32 字节不透明随机凭据；`device_tokens` row 以 `device_id` 为键，保存唯一 `enrollment_request_id` 和 32-byte `token_hash`。ADR-0038 flag day 将删除它；dormant control foundation 不改变其当前 authority。 |
| **Control key（ADR-0038 目标）** | Device 专用 Ed25519 control signing key，与 Gateway TLS key 分离。Server natural key 是 `public_key`（32-byte Ed25519），不存在 ControlKeyId；daemon manifest pins hex(public_key)。Private key 只存在于 daemon-owned PKCS#8 文件。flag day 前不参与 Enrollment/WSS authority。 |
| **ServerChallenge（ADR-0038 目标）** | HTTP 101 后由 connection-local PreAuthSession 发送的 `{protocol_version,challenge_nonce}`。Nonce 固定 32 bytes、不持久化、不跨连接查询且无需 Client 回显 challenge ID。Server 本地 timer 只约束本连接 `ClientProof` 的接收与验证；协议不携带 expires timestamp。 |
| **ClientProof（ADR-0038 目标）** | Device 对 Challenge 的 Ed25519 proof。`oneof purpose` 为 `EnrollmentAttempt` 或空 `ResumeSession`；前者必带 durable canonical UUIDv7 `enrollment_id`、candidate public key、exact CSR 与 evidence quality，后者不携带 Enrollment material。无 `ProofIntent` enum。 |
| **Enrollment transaction（ADR-0038 目标）** | Device 在首次联网前持久化 `enrollment_id`、control/Gateway keys 与 exact CSR。Server durable state 为 `pending_review`、`awaiting_credential_ack`、`active`、`denied`；每个新 transaction 必须人工审核且意图由 Server 派生。pending/denied 重放稳定 review status，awaiting/active 重放 exact Bundle，Ack 分别执行一次 activation/no-op，再返回 Activated；同 ID + 不同 material 为 conflict。transaction 不过期。 |
| **EnrollmentReviewStatus（ADR-0038 目标）** | Server→Client 的 Enrollment 业务状态包：`PENDING_REVIEW` 表示 durable 等待人工审核，`DENIED` 携带 stable `error_code`。批准不新增 status variant；不可变 `CredentialBundle` 本身即批准结果。 |
| **CredentialBundle（ADR-0038 目标）** | Server 下发的 `{enrollment_id,gateway_leaf_der}` immutable public bundle。Client durable 保存后回 `CredentialAck{enrollment_id,bundle_sha256}`；Ack 激活 Server authority。 |
| **EnrollmentActivated / Ready（ADR-0038 目标）** | Server Ack transaction commit 后发送 `{enrollment_id,device_id,bundle_sha256}` Activated；Client 原子写 Active manifest 后以 Ready 完整回显这些 facts。它们分隔 Server durable authority 与 Client durable Active，Ready 后才可签发本连接 SessionReady。 |
| **SessionReady（ADR-0038 目标）** | 双端 durable Enrollment barrier 完成后的 connection-local lease：`session_id` bytes。Active envelope 只回该值；不是 Enrollment commit receipt 或 Identifier UUID。发送后 Server actor 先进入 `AwaitingInitialObserved`，第一条 Active packet 必须是 Observed。 |
| **DeviceActor（ADR-0038 目标）** | 动态 registry 按需创建、进程内不淘汰的单 Device 排序点；Machine Hardware ID 只负责 shard routing，DeviceId 与 `public_key` 是 aliases。每个 actor 有一个有界 single-consumer mailbox；无 ControlKeyId。 |
| **Provisioning window** | `provisioning_window` 的当前 singleton 开关：`state`、`revision`、`last_audit_event_id`。ADR-0038 目标中，open 只允许新 Enrollment admission 与 operator approve/sign；关闭后 pending transaction 保留，已固化 Bundle 的 exact replay、Ack 与最终化继续。Enrollment transaction 无 expiry。恢复只会将已打开状态 close-once，不会自动打开。flag day 前的当前 Token/HTTPS 行为单独保留。 |
| **close-once** | provisioning 窗口恢复语义：只关闭一次观察到的 `open` current-fact，后续 `closed` observation 不形成第二次恢复事件。 |
| **Gateway certificate** | provisioning 窗口内经 Enrollment 签发、供本机 Caddy loopback HTTPS 使用的证书；`gateway_certificates` 保存 `certificate_id`、`device_id`、`enrollment_request_id`、serial、SPKI hash、`not_after_unix_ms`、status，而不保存 certificate body；私钥在 Client 本地生成且不离机。 |
| **Machine Hardware ID** | 按固定多源配方（ADR-0032）规范化并派生的稳定机器标识；不是认证凭据。 |
| **Fleet namespace UUID** | 站点级公开且不可变的 UUID，用于确定性派生 Machine Hardware ID。 |
| **Binding** | Seat 与 Device 的当前业务关联。`device_bindings` row 有 `seat_id` PK、unique `device_id` 与 unique `binding_id`；解绑通过删除关系表达。 |
| **Binding ID / `binding_id`** | `device_bindings.binding_id`：每次 bind 铸造的 canonical lowercase hyphenated UUIDv7 occupancy stamp，独立于 Seat，供 `SyncState` / `SyncSecret` / Observed 冻结。`BindingRequest` / `BindingResult` 都不携带该字段。unbind 删除该行；再次 bind 得到新 UUID。无 integer bump，无 `seats.binding_generation`，无 `revision_counters`。Import 不写入 Binding、不铸造该 ID。它不表示 Seat→Account mapping。不存在全局 binding-set clock。 |
| **Bound assignment** | `SyncState` 的 bound variant：完整 `{binding_id,account_id,seat_code,domjudge_username}`。无绑定使用显式 `unbound` variant，不用空字符串组合。双方从 assignment 的 canonical protobuf bytes 派生 applied hash，wire 不重复发送 hash。 |
| **Credential observation** | `SyncSecret` 与 Observed 共同使用的完整 convergence identity：`{binding_id,account_id,credential_revision}` 加 installed/unset/error state。只比较 revision 不足以证明密码安装到了正确 Account/Binding。 |
| **SecretBytes** | 专用于协议秘密 byte string 的 wrapper；其生成类型必须 redacted Debug/日志/通用展示。它缩小误泄漏面，但不替代最短生命周期、zeroize 与访问控制。 |
| **Seat→Account mapping** | `account_mappings` 中当前 confirmed contest configuration 内 Seat 与 Account 的一对一关系；没有 mapping row 表示 Seat 当前无 Account。由 Import Commit 写入，不铸造 `binding_id`。 |
| **Confirmed contest configuration** | Server 当前权威的 Seat 集合、Seat→Account mapping 和 credential metadata。没有永久冻结的 Seat universe、generic instance state 或可查询的历史 mapping/credential 版本；只能通过完整 candidate 的显式 Import Commit 被替换。 |
| **Contest configuration revision** | 已废止。不存在 `revision_counters` 或全局 configuration clock；Import 不以任何 revision CAS 保护 Seat/mapping。preview→commit 串行化只靠 singleton `pending_import_candidate`。 |
| **Credential revision** | `accounts.credential_revision`：某 Account 当前秘密的单调修订。任何已提交的 Import Commit 都无条件替换该 Account 的当前 ciphertext 并推进此修订，不做明文比较。每个 Account 只有其关联的 current vault ciphertext，不保留可寻址的旧密码版本。 |
| **Candidate import** | 单次严格 CSV upload 的完整、非秘密 pending 草稿；全局同一时刻最多一个。`pending_import_candidate` singleton 只保存 candidate ID、`expires_at_unix_ms`、preview-token hash、nonsecret fingerprint 和 redacted preview；不保存密码或 encrypted CSV。终止时只删除该 row，只保留 redacted audit lineage。 |
| **Import preview / import diff** | Server 对 candidate 与 confirmed baseline 的 redacted 结构化比较结果。Server 是 classification 的唯一权威。 |
| **Import Commit** | Operator 对 candidate 的显式二次确认动作；请求体再次提交同一 CSV，`preview_token` 经 header `Natsume-Preview-Token`。不对任何 revision 做 CAS；先常量时间比对非秘密 fingerprint，不一致则 `IMPORT_CANDIDATE_MISMATCH` 且零写入、candidate 保留。原子应用 Seat 集合、Seat→Account mapping 与 credential。不写入 Binding、不铸造 `binding_id`；将删座位仍绑定则拒绝。no-op 仅 lineage/redacted audit。material/no-op 只在非秘密维度上由 seats/mappings diff 定义：任何已提交的 commit 都推进每个 Account 的 credential revision。不自动产生 Device Command。 |
| **Preview token** | Server 签发的 opaque 证据；`pending_import_candidate` 只保存其 `preview_token_hash`，并保存 candidate identity、非秘密 fingerprint、redacted preview 与 `expires_at_unix_ms`。commit 时作为 HTTP header `Natsume-Preview-Token` 回传，不得放入 query string。HTTP preview/pending 面的 `expires_at` 仍为 RFC 3339 UTC。 |
| **Import Discard** | Operator 显式终止尚未提交的 candidate：在同一事务删除 `pending_import_candidate`，并保留 redacted audit；不触及 vault，不改变 confirmed configuration、binding、revision 或 Target。 |
| **Session epoch** | 本地 graphical session 身份（Agent lease / logind）。WSS Oneshot 命令不携带 `session_epoch` / `SessionTarget`；目标就是该 Device 唯一当前 session。Client 在动作开始时本地捕获它，并在 privileged effect 前重验；若期间被替换，返回 `SESSION_CONTEXT_STALE` 且不 retarget。 |
| **Home epoch** | `HOME_RESET` 的 Converge 键。同 epoch 可重入，已完成则为 success/no-op；`HOME_EPOCH_STALE` 仅当 epoch < 已完成 epoch；重试不得 bump epoch。 |
| **Client 凭据文件** | Device 本地 service-user-owned 权限文件：Seat 凭据、identity record、`control-key-1.pk8` 与 control manifest（pins hex(public_key)，无 ControlKeyId）为 `0600 natsume:natsume`；flag day 前还含 Device Token，control key foundation 尚未接权。Gateway key/leaf 与含凭据 Caddy 配置为 `0640 natsume:natsume-gateway`；全部原子写，无应用层加密（ADR-0032）。 |
| **Server vault** | `server_vault_records` 中的应用层加密秘密存储，只保存当前 Account 的 DOMjudge 密码。`accounts` 为父表；vault 以 `account_id` 为 PRIMARY KEY 且 `REFERENCES accounts(account_id) ON DELETE CASCADE`，每个 Account 至多一行。row 只有 `account_id`、nonce 和 ciphertext，不含 `vault_record_id`、`record_type`/`subject_id`、format/key/AAD version 或 timestamp。同一事务先插 Account 再插 vault；删除 Account 级联删除 vault。 |
| **AuditEvent** | 具有 `audit_event_id`、由 audited guarded operation 自行插入并与敏感领域 mutation 原子提交的 redacted 证据；fresh ID 可作为 typed operation input，但已持久化的同 ID 或预插入 audit row 不能重放为新 mutation 的依据。其 envelope 只有 `occurred_at_unix_ms`、actor、action kind、resource type/optional ID、result、optional reason code、correlation ID、optional group correlation ID 和 typed `redacted_detail_json`；revision/count 等 event-specific detail 只在该 JSON 内。 |
| **guarded operation** | 具有显式 guard，并在一个 transaction 内原子提交敏感领域 mutation 与其 typed `AuditEvent` 的领域操作。 |
| **`system:recovery`** | [审计词汇注册表](contracts.md#当前-auditevent-词汇注册表)中表示启动或恢复路径的系统 actor 值。 |
| **`system:password-reset`** | [审计词汇注册表](contracts.md#当前-auditevent-词汇注册表)中表示离线 operator password reset 路径的系统 actor 值。 |
| **LKG** | Last Known Good，本地最后一次已验证可用的配置或证书集合。 |
| **Caddy BLOCKED** | Caddy 仅提供有限本地状态页，不代理 DOMjudge 的 fail-closed 状态。 |
| **Caddy READY** | Gateway 证书、配置和目标 upstream 都通过验证后激活的数据面状态。 |
| **Enrollment（当前实现，flag day 前）** | provisioning 窗口内的 server-auth HTTPS 注册流程。`enrollment_requests` 记录硬件、CSR/SPKI、client/protocol/source-IP、state、可选 resolution/resolved device/issuance audit 与 `created_at_unix_ms`；`device_tokens` 和 `gateway_certificates` 各自以唯一 Enrollment request 关联签发结果。该 HTTP 模型不定义 ADR-0038 目标状态机。 |
| **Enrollment（ADR-0038 目标）** | ordinary server-auth WSS 内以 Challenge proof 开始的 durable、人工审核注册流程。窗口 gate admission 与 approve/sign；transaction 不过期，Bundle/Ack/Ready/Resume 负责断线恢复，Client 不自报 create/replacement/recovery 意图。 |
| **xheaders 自动登录** | DOMjudge 官方 X-Headers 认证：Caddy 仅在 `/login` 路由注入 `X-DOMjudge-Login` 与 base64 的 `X-DOMjudge-Pass`（ADR-0034）。 |
| **镜像升级重验清单** | 每次赛事镜像 bump 后必须重跑的桌面 capability 验证集（ADR-0035），维护于 supported-platform.md。 |
| **typed contract** | 输入集合封闭、字段和枚举明确、能够被穷举校验的接口契约。 |
| **evidence locator** | 能定位到可复现实验、CI、artifact、日志或签署记录的稳定引用。 |
| **fail closed** | 无法证明安全或一致时停止敏感动作，而不是猜测、降级或自动重建身份。 |
