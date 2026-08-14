# Natsume V2 领域模型

> 状态：`NORMATIVE`  
> 适用范围：Server 业务状态、Target/Observed 语义和本地运行时身份  
> 不包含：数据库列级 schema、Protobuf 字段编号、HTTP 路由

数据库 migration 是物理 schema 的权威来源；本文件定义稳定的业务含义、聚合边界和安全不变量。未实现行为的具体字段、状态枚举与事务编排延迟到对应 Phase 实现时定义。

## 1. 建模原则

1. 一个实例只建模当前一场竞赛，不创建 `Event` 聚合。
2. Confirmed contest configuration 只能通过完整 candidate 的显式 Import Commit 被替换。
3. 业务表只保存 current-fact（当前事实）；不可替代的历史只限 redacted `AuditEvent`，以及在其所属表段落显式声明了保留理由与消费者的终态行；不创建 generic instance state 或可恢复的业务 snapshot。
4. 内部主键与外部/硬件标识分离。
5. **密码是秘密值，不是普通实体属性。**
6. Target、Observed、Drift 和 Command 含义相互独立。
7. **远端副作用不在普通领域事务中“假装完成”。**
8. **删除、重置和替换必须显式，不能通过 identity fallback 隐式发生。**
9. **所有陈旧性判断使用单调 revision/epoch，而不是时间戳猜测。**

### 1.1 权威业务表注册表

| 表名 | 职责 |
|---|---|
| `site_identity` | 站点身份（fleet namespace UUID） |
| `revision_counters` | 全局修订计数器 singleton |
| `server_vault_records` | 加密 vault 密文 |
| `seats` | 座位 |
| `devices` | 设备 |
| `audit_events` | 审计事件（唯一证据历史） |
| `operator_accounts` | 操作员账户 |
| `operator_sessions` | 操作员会话 |
| `accounts` | 竞赛账户 |
| `account_mappings` | Seat→Account 映射 |
| `device_bindings` | Seat↔Device 绑定 |
| `observed_device_states` | 设备观测状态 |
| `provisioning_window` | 发放窗口 singleton |
| `enrollment_requests` | 设备注册请求 |
| `device_tokens` | 设备令牌 |
| `gateway_certificates` | 网关证书 |
| `pending_import_candidate` | 唯一待定 CSV 导入候选 |
| `commands` | 命令 current row |

本清单与 schema tests（`integration-tests/tests/schema_contract.rs`、`server/src/db.rs`）相互锁定：新增或删除任何表都必须同步更新两处测试与本清单。

## 2. 标识和值对象

领域使用一组稳定值对象区分业务身份与硬件标识。共享修订只由 `revision_counters` singleton 持有：`configuration_revision` 表示 confirmed Seat 集合与 Seat→Account mapping 的修订，也是 import baseline CAS token，密码内容不参与该修订；`binding_revision`（`BindingRevision`）表示**全局** Seat↔Device 当前 Binding 集合的 CAS 修订，不表示 Seat→Account mapping。`accounts.credential_revision` 是每个 Account 当前秘密的修订；`SessionEpoch` / `HomeEpoch` 是本地运行时代际。

每次 Binding 集合实际变化时，`BindingRevision` 在该事务内最多递增一次。受影响的新增/变更 `device_bindings` row 记录该全局值；未变化 Binding 不重写，因此不因无关绑定变更而成为 secret-sync stale。Account mapping 或密码变化而 Binding 保持时，不得推进它。

wire 上的 `binding_revision`（`TargetAssignment` / `SyncSecret` / `BindingResult`）是行级 stamp：该 Binding 行创建或变更时记录的全局值，不是当前全局计数器本身。`observed_device_states.installed_binding_revision = 0` 是合法哨兵，表示尚未安装任何 Binding；列域 `>= 0` 与 Binding 行要求 `> 0` 的差异即来源于此。

面向单台 Device 的非秘密 generation 是从 current Server truth 派生的 contract artifact，不创建独立 counter 或通用 version system；`observed_device_states` 只记录 Device 报告的 `received_generation`、`applied_generation`，以及可选 `gateway_configuration_revision`。

内部主键（`devices.device_pk`，TEXT）是 Server 生成、遵守[契约 Identifier](contracts.md#11-identifier)的 canonical lowercase hyphenated UUIDv7，且不得从硬件数据推导；硬件身份（`machine_hardware_id`，派生配方见 [ADR-0032](adr/0032-device-identity-and-local-credential-lifecycle.md)）不是认证凭据。

值对象必须在进入 domain 前完成结构校验。**Domain 不解析自由格式路径、URL、shell、证书文本或 UI 文案。**

## 3. ContestConfiguration import

每个 `seat,account,password` CSV 都是完整的 contest configuration candidate，不是增量 patch。边界规则（并发模型见 [ADR-0031](adr/0031-contest-import-and-secret-evidence.md)）：

- 只接受固定三列 UTF-8 CSV（可带 BOM）；不接受额外列、XLSX/ODS、公式、列映射或自动猜测；
- **全局同一时刻最多一个 encrypted pending candidate**；`pending_import_candidate` singleton row 存在即为 pending，严格解析失败不落库；
- pending candidate 只保留 `candidate_id`、`expires_at`、`baseline_configuration_revision`、`baseline_binding_revision`、`preview_token_hash`、`payload_vault_record_id` 与 `redacted_preview_json`，不使用 import state/history；
- `password` 只进入加密 staging 和 secret commit path，明文不进任何普通 surface；
- Commit、discard 与 expiry 在其事务中删除 candidate 和 `payload_vault_record_id` 所引用的 `server_vault_records` row，只留下 redacted audit lineage；这不承诺 SQLite/WAL/backup 的取证级物理擦除；
- Commit 校验为双 CAS：baseline `configuration_revision` + `binding_revision`；任一失配 → 拒绝并重新 preview，**不改变 confirmed configuration、binding、Target truth 或相关 revision**；
- `configuration_revision` 只在 Seat 集合或 Seat→Account mapping 实际改变时推进——二者都是非秘密事实，无需接触秘密即可比较；`BindingRevision` 只在 Binding 集合实际改变时推进，包括 import 删除已绑定 Seat；
- **已提交的 Import Commit 无条件替换新确认配置中每个 Account 的 vault ciphertext（新 nonce）并推进其 `credential_revision`**；不做任何明文比较，preview 也不分类或展示密码内容是否变化。因此每次成功 import 之后全部已绑定 Device 的已安装 credential revision 都是陈旧的，操作员必须显式发起批量 `SYNC_SECRET`（N 个独立 Command）；import 本身仍然零 Command、零 Device I/O（`INV-SECRET-02` 不变）；
- 任何 `INVALID`（结构性错误、candidate 内重复 account、空或仅 header candidate）、expiry 或 discard 同样不改变 confirmed configuration、binding、Target 或 revision；
- Import Commit 不创建 Command，不自动执行 `SYNC_STATE` 或 `SYNC_SECRET`，不产生 Device I/O；
- 清空 confirmed configuration 只能通过独立的 single-lifetime reset，不得由 import 隐式完成。

Confirmed configuration 只表示现在：Seat collection 不冻结，Seat code rename 表示 `REMOVED + ADDED`，没有 generic instance state、历史 Seat universe 或 history-based rollback。完整 import lifecycle、preview evidence 与 diff taxonomy 的具体字段在 Phase 2 实现时定义。

## 4. Device 与 provisioning

- `devices.machine_hardware_id` 是 unique，因此一个存储的 `machine_hardware_id` 最多对应一个 `devices` row；该 row 只有 `device_pk`、硬件 ID、`hardware_identity_quality`（`strong`/`medium`/`weak`）与 `state`（`enrolled`/`revoked`/`disabled`）。
- 凭据签发或替换要求其绑定的 Device 当前为 `enrolled`；`revoked` 或 `disabled` Device 必须拒绝签发，恢复使用须经过显式、受审计的 lifecycle 动作，不得由 Enrollment 隐式复活。
- provisioning window 由 `provisioning_window` current singleton 表示，只有 `state`、`revision` 和 `last_audit_event_id`；不保留逐次 provisioning revision 状态行，历史由审计提供。restart/restore 只会对观察到的 `open` 状态执行一次 audited CAS close；已 `closed` 时零写入。
- provisioning 窗口内同一 `machine_hardware_id` 的 Enrollment 由 `enrollment_requests` 的 `resolution` 与 `resolved_device_pk` 表达受审计的 create/credential-replacement 结果；关联的 `device_tokens` 和 `gateway_certificates` row 提供当前 token/certificate 事实。`enrollment_requests` 的终态行（`rejected` / `expired` / `conflict` 等）予以保留；其消费者是 same-SPKI 重试自动批准判定、同硬件不同 SPKI 稳定拒绝与审计回溯。不设清理规则，赛后导出与清理归 Phase 7。若旧连接仍存活，记录异常审计事件。
- `gateway_certificates` 的 `revoked` / `retired` 状态行予以保留；其消费者是单活跃证书唯一性判定（partial unique index 只约束 `status = 'active'`）、Enrollment 替换路径对旧证书的处置与审计回溯。
- 窗口外的硬件身份冲突：**不自动合并、不选择“最近上线者”、不删除凭据**，停止敏感进展并返回稳定错误，要求人工执行恢复 runbook。
- Device 删除/替换不复用 `device_pk` 或凭据文件；替换走窗口重开 + 新 Enrollment 的受审计生命周期，而不是 merge。

## 5. 当前 Seat→Account mapping 与 Binding

- `seats` 只表示当前 Seat 身份；`accounts` 只表示当前账号身份。
- `account_mappings` 是 confirmed contest configuration 内的一对一当前 Seat→Account mapping：`seat_id` 是主键、`account_id` 是 unique；没有 mapping row 表示 Seat 当前无 Account。它属于 `configuration_revision`，不保存 superseded/unassigned/history 行。
- `device_bindings` 是 Seat↔Device 的一对一当前关系：`seat_id` 是主键、`device_pk` 是 unique、每 row 有正的 `binding_revision`；解绑通过删除当前关系表达。
- `device_bindings` 只以 foreign key 关联当前 confirmed contest configuration 中的 Seat 与 `devices.device_pk`；允许何种 `devices.state` 的绑定由 domain policy 校验，不另建 schema constraint。
- bind、unbind、rebind 或 import 删除已绑定 Seat 是 Binding-set mutation，必须以全局 `BindingRevision` CAS 原子提交；保留的 Seat code 上的 Binding 在 Account/password 变化时保持不变。
- **binding 修改只改变 Server truth 和 Target，不自动同步 Device；** secret sync 必须绑定发起时的 Seat、Device 和 BindingRevision。

## 6. Credential

密码明文不作为普通 `Account` 字段暴露。`accounts` 只通过 unique `credential_vault_record_id` 关联一个 `server_vault_records` row，并保存 `credential_revision`；vault row 只有 `vault_record_id`、`record_type`、`subject_id`、`nonce` 与 `ciphertext`，没有 format/key/AAD version 或 timestamp。已提交的 Import Commit 无条件替换当前密文（新 nonce）并推进该 Account 的 `credential_revision`，不做明文比较，也不建立 history credential/vault row。

读取密码的 application use case 必须：

1. 通过 operator authorization；
2. 绑定明确 `SYNC_SECRET`；
3. 读取当前 assignment 和 credential revision；
4. 在最短生命周期内解密并发送；
5. 不进入普通结构、日志或事件；
6. 只记录 redacted 审计元数据。

## 7. DeviceTarget

Target 是根据已提交 Server truth 为某台 Device 计算的**非秘密**期望状态。Target **不包含** password、private key、token、任意 shell/路径/UID/unit/环境或自由格式 Caddy fragment。Target 生成是可重放的纯计算（`Server truth + frozen policy → DeviceTarget`），其代际由输入 revision 派生。**Target 变化不自动创建 Command**；操作员必须显式创建 `SYNC_STATE`。

## 8. DeviceObserved

Observed snapshot 是 Device 对自身实际状态的 typed 报告。物理 current row 是按 `device_pk` 唯一的 `observed_device_states`：它保存 sequence/boot、received/applied generation 与可选 hash/error、installed binding/credential revision、secret/gateway 状态与可选 gateway configuration/certificate facts、session/lock/agent/UI/desktop capability facts、`home_state` 和 `observed_at`；没有第二张 current-facts 或 history 表。**Observed 不得携带秘密；Device 自报的属性不构成授权。** Observed 可能陈旧；Server 保留接收时间和 freshness 语义，不得用单个 `READY` 覆盖全部维度。上报节奏为变化时上报 + 低频周期兜底。

## 9. Drift

Drift 是纯比较结果（`compare(Target, latest valid Observed)`），不持有独立业务真相，可从 Target 和 Observed 重算。“无 Drift”不等于设备在线，也不等于全部安全证据有效。

## 10. Command

**Command** 是面向单台 Device 的 direct durable intent。Panel 在创建前生成 canonical lowercase hyphenated UUIDv7 `command_id`，并使用 `PUT /api/v2/commands/{command_id}`；Server 以 `request_fingerprint_version` 和 `request_fingerprint_sha256` 判断 replay：同 ID+同 request 返回既有 Command，同 ID+不同 request 是稳定 conflict。首次 `201`、replay `200`、invalid ID `400`、conflict `409` 的完整 HTTP 语义见 [契约](contracts.md)。

`commands` current row 只有 `command_id`、`device_pk`、`kind`、`state`、两个 request fingerprint field、可选 `group_correlation_id`、`payload_version`、typed object `frozen_payload_json`、`created_at`、`deadline_at`、可选 `terminal_error_code`/`redacted_terminal_result_json` 与 `created_audit_event_id`。**Command receipt 在 Device 持久化前不得确认；相同 Command ID 不得重复副作用。** 每 Command 的 frozen content 只由 `frozen_payload_json` 表达，而不是许多 frozen top-level columns 或 dispatcher metadata；陈旧时用稳定错误拒绝，不“尽量兼容”地部分应用。相同 ID 必须原样贯穿 Server、WSS、Device journal、CommandStatus 与审计 correlation。终态行（`terminal_error_code` / `redacted_terminal_result_json`）予以保留；其消费者是 Panel 状态查询与审计回溯，清理与赛后导出归 Phase 7。

批量操作 = 批量创建 Command，进度视图只由查询聚合；可选 `group_correlation_id` 只用于查询和审计分组，不定义跨 Device 顺序、原子性或 lifecycle。Command 的具体状态机与字段在 Phase 4 实现时定义；本规范不声明 dispatcher/journal/Panel mutation 已完成。

## 11. AuditEvent

**AuditEvent** 是唯一通用历史/证据记录。每个 audited guarded operation 在同一 transaction 内自行插入 audit row 和敏感业务 mutation；fresh `audit_event_id` 可作为 typed operation input，但已持久化的同 ID 或预插入 audit row 不能重放为新 mutation 的依据，audit 写失败则整个事务回滚。`audit_events` 的 envelope 只有 `audit_event_id`、`occurred_at`、`actor`、`action_kind`、`resource_type`、可选 `resource_id`、`result`、可选 `reason_code`、`correlation_id`、可选 `group_correlation_id` 与 typed object `redacted_detail_json`。适用的 configuration/binding/credential/provisioning revision、计数和其他 event-specific redacted evidence 都在该 JSON 内，而不是 nullable top-level columns。

敏感变更必须有 redacted audit；**不得记录密码、private key、Device Token 值、任意上传原文或未脱敏错误链。** Web Panel 以轮询读取权威状态（[ADR-0034](adr/0034-state-execution-and-data-plane-boundary.md)）。

## 12. Operator 身份与会话

**Operator** 是控制面人类操作者的当前身份事实，不是配置项（[ADR-0037](adr/0037-operator-identity-and-server-runtime-secrets.md)）。

- 一个 Operator 持有稳定内部主键、唯一登录标识和固定角色（`admin` / `viewer`）。角色是封闭枚举，**不建立 role/permission 关联表，也不表示权限位集合**。
- **密码是秘密值，不是普通实体属性**：只保存具备工作因子的密码哈希，明文不进入领域结构、日志、审计或响应。
- **Session** 是 Operator 的当前会话事实，只保存会话凭证哈希、所属 Operator 与绝对过期时间；不保存来源 IP、User-Agent、最近活动时间或会话历史，也不做滑动续期。
- 会话终止（登出、过期、撤销）通过删除当前会话行表达，不保留 terminal 会话状态；重复终止是 repeat-safe 的零写入。
- Operator 是 `audit_events.actor` 的来源之一；`system:recovery` 等非人类 actor 不对应 Operator 行。
- Operator 身份**不参与 Device 认证**：不能取得 Device Token、Gateway certificate 或 WSS 控制面身份（`INV-CERT-01`）。

Device lifecycle 的 operator 动作（转 `revoked` / `disabled`，以及移除当前 `device_tokens` row、将关联 `gateway_certificates` 状态行转为 `revoked` 后保留）是 repeat-safe 的当前事实 mutation：目标状态已达成时零业务写入，只记录 `result = 'noop'` 的 audit。它们**不创建 Command、不产生 Device I/O、不改变 Binding 集合，也不推进任何 revision**——解绑是独立的 Binding-set mutation（§5），不由 Device state 转移隐含触发。完整的 Device 删除/替换生命周期见 §14。

## 13. 本地运行时领域

- **Machine identity startup**：identity 检查先于一切 identity-bound 产物使用。决策是封闭枚举（如 `FIRST_BOOT_ALLOWED`、`IDENTITY_MATCH`、`IDENTITY_UNAVAILABLE_FAIL_CLOSED`、`IDENTITY_MISMATCH_FAIL_CLOSED`、`CREDENTIALS_UNREADABLE_FAIL_CLOSED`、`RECOVERY_REQUIRED`）。**不能输出“猜测最可能是同一台机器并继续”。**
- **Session**：所有 lock/unlock/terminate/UI action 都必须校验当前 `SessionEpoch`；陈旧 Agent 或陈旧 UI action 被拒绝。
- **Home**：每次 `HOME_RESET` 建立新的、由 Server 分配且严格单调的 `HomeEpoch`；reset 完成前不启动受管 session；中断的 reset 必须可恢复；**无法证明 mount/copy/ownership 安全时 fail closed；不静默切换 backend**。重置仍是操作员在场的受控事件，实现为状态文件 + 可安全重复运行的步骤；本地 `PrepareHomeInstance` / `ActivateHomeInstance` / `RecoverHomeInstance` / `GarbageCollectHomeInstance` 分解只属实现面，D-Bus surface 保持不变。

## 14. 删除、重置和替换

- **Device 删除**：Server 显式授权、停止新 Command、更新 `devices.state`、移除或替换当前 `device_tokens` row、更新关联 `gateway_certificates.status`、解除 binding、记录审计；不复用旧 `device_pk`。
- **Device 替换**：原 lifecycle 结束、重开 provisioning 窗口、新硬件独立 Enrollment、人工重建 binding、显式 `SYNC_STATE`/`SYNC_SECRET`；不复制凭据文件。
- **单生命周期竞赛重置**：通过破坏性 runbook 清理业务状态和秘密；重置后 `revision_counters.configuration_revision = 0`，下一次 import 走普通 first-import lifecycle。
- **candidate/credential 替换**：终止 pending candidate 删除其 encrypted payload；密码替换删除可寻址的旧 current ciphertext。两者的 redacted audit 保留，但不承诺存储介质的物理擦除。

## 15. 领域测试最低要求

每个聚合至少覆盖：value object 边界、正向状态转移、陈旧 revision/epoch 拒绝、事务回滚、audit envelope 原子性、secret redaction 与 adapter 错误穷举映射。当前基线还必须覆盖 BindingRevision 的全局 Binding-set CAS、candidate 终态删除、provisioning close-once recovery、`frozen_payload_json` 验证以及 canonical UUIDv7 Command ID 的 replay/conflict；具体场景随对应 Phase 实现补全。
