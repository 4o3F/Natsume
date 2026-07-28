# Natsume V2 领域模型

> 状态：`NORMATIVE`  
> 适用范围：Server 业务状态、Target/Observed 语义和本地运行时身份  
> 不包含：数据库列级 schema、Protobuf 字段编号、HTTP 路由

数据库 migration 是物理 schema 的权威来源；本文件定义稳定的业务含义、聚合边界和事务规则。

## 1. 建模原则

1. 一个实例只建模当前一场竞赛，不创建 `Event` 聚合。
2. Confirmed contest configuration 只能通过完整 candidate 的显式 Import Commit 被替换；不存在首次 commit 后永久冻结。
3. 内部主键与外部/硬件标识分离。
4. 密码是秘密值，不是普通实体属性。
5. Target、Observed、Drift、Operation 和 Command 含义相互独立。
6. 远端副作用不在普通领域事务中“假装完成”。
7. 删除、重置和替换必须显式，不能通过 identity fallback 隐式发生。
8. 所有陈旧性判断使用单调 revision/generation/epoch，而不是时间戳猜测。

## 2. 标识和值对象

| 名称 | 含义 | 规则 |
|---|---|---|
| `SeatCode` | 现场席位代码 | 在当前 confirmed contest configuration 内唯一；集合可由成功 Import Commit 完整替换；Seat code 即身份，rename 视为 `REMOVED + ADDED` |
| `AccountName` | DOMjudge 账号标识 | 不含密码；按输入契约规范化 |
| `ContestConfigurationRevision` | confirmed configuration 内容的单调 revision | `0` 表示空 baseline；仅内容实际变化时递增；不得与 Device `ConfigurationGeneration` 混用；用作 baseline CAS token |
| `CredentialRevision` | 账号密码版本 | 每次密码有效变更单调递增 |
| `DevicePk` | Server 内部 Device 主键 | UUIDv7；不能从硬件数据推导 |
| `MachineHardwareId` | 稳定硬件身份 | UUIDv5；站点 namespace + 规范化硬件来源 |
| `FleetNamespaceUuid` | 站点命名空间 | 公开、不可变；不是秘密 |
| `AssignmentRevision` | Seat/Device binding 修订 | 每次有效绑定变化单调递增 |
| `ConfigurationGeneration` | 非秘密设备配置代际 | 每次 Target 语义变化单调递增 |
| `CommandId` | 单设备远端命令 ID | 全局唯一；用于幂等 |
| `OperationId` | 批量/异步业务动作 ID | 只有需要聚合时创建 |
| `SessionEpoch` | 当前受管会话代际 | 会话重建后变化 |
| `HomeEpoch` | 当前 Home transaction 代际 | Home 准备重启后变化 |
| `CertificateSerial` | 签发证书唯一序列 | 由 PKI adapter 管理 |
| `SpkiFingerprint` | 公钥绑定指纹 | 用于 Gateway request 幂等和冲突判断 |

值对象必须在进入 domain 前完成结构校验。Domain 不解析自由格式路径、URL、shell、证书文本或 UI 文案。

## 3. 聚合总览

```text
ContestConfiguration
  ├─ confirmed Seat/account/credential-metadata collection
  ├─ ContestConfigurationRevision
  ├─ non-secret internal content hash
  └─ candidate / import lineage

Device
  ├─ lifecycle
  ├─ MachineHardwareId binding
  └─ certificate metadata

Binding
  ├─ SeatCode
  ├─ DevicePk
  └─ AssignmentRevision

DeviceTarget
  ├─ ConfigurationGeneration
  ├─ non-secret configuration
  └─ secret-required metadata only

DeviceObserved
  ├─ observed revision/generation
  ├─ certificate/config/data-plane/session/home status
  └─ diagnostic codes

Operation
  └─ OperationTarget
       └─ Command
            └─ Attempt
```

这是逻辑所有权，不要求每个条目对应一张表。

## 4. `ContestConfiguration`

### 4.1 职责

拥有：

- 当前 confirmed contest configuration（Seat/account/credential-metadata 集合）；
- `ContestConfigurationRevision`；
- 仅基于非秘密信息的内部 content hash（Seat、account、credential revision 等一致性证据）；
- candidate import 与 import lineage；
- credential revision 元数据。

不拥有：

- Device；
- binding；
- Device Target；
- 密码明文的普通读取接口；
- DOMjudge 运行时状态；
- 完整历史 snapshot / rollback 产品；
- 永久 freeze 标记。

### 4.2 Contest configuration import

每个 `seat,account,password` CSV 都是完整的 contest configuration candidate，不是增量 patch。

输入契约：

- 只接受一个 UTF-8 文本文件，可允许 UTF-8 BOM；
- 列必须恰好为：

```text
seat,account,password
```

- `seat` 在 candidate 内必须唯一；
- `account` 必须满足冻结的规范化规则，且在完整 candidate 映射内必须唯一；confirmed 完整映射同样不得出现重复 account；
- candidate 内重复 `account` 判为 `INVALID`，令 `commit_allowed = false`，Operator 必须修复文件并创建新 candidate；合法 account 互换（两 Seat 交换 account，完整映射仍唯一）允许；
- 空 candidate 或仅 header、无有效数据行的 candidate 显式 `INVALID`，`commit_allowed = false`；不得通过 CSV import 清空 confirmed contest configuration；清空仅可通过独立的 single-lifetime reset 机制，不得由 import 隐式完成；
- `password` 只能进入加密 staging 和 secret commit path；
- 不接受额外列、公式、sheet、列映射、XLSX/ODS 或自动猜测；
- 不得向普通 API、Browser、audit、log、metric、SSE 或 outbox 暴露 raw CSV hash、password fingerprint、password length 或其他 password-derived digest。

统一 lifecycle（首次与后续相同）：

```text
upload
  -> encrypted staging
  -> strict parse
  -> immutable candidate import
  -> server-computed redacted diff
  -> explicit Import Commit 或 Import Discard
```

Import Commit 在通过幂等预检后的分支：

```text
idempotency preflight (step 0)
  -> [material] live validation
       -> baseline compare-and-swap
       -> atomic unbind-and-replace（仅当已授权 binding impact 非空）
       -> replace confirmed configuration / revision bumps / content outbox（仅实际变化）
  -> [no-op] live validation
       -> lineage + redacted audit only
       （before_revision == after_revision；无 unbind-and-replace、无内容 ChangeEvent/outbox、无 Target churn）
```

术语：

- **Confirmed contest configuration**：Server 当前权威的 Seat/account/credential-metadata 集合。
- **Contest configuration revision**：confirmed configuration 内容的单调 revision（`ContestConfigurationRevision`）；`0` 表示空 baseline；仅内容实际变化时递增；不得与 Device `ConfigurationGeneration` 混用；用作 baseline CAS token。
- **Candidate import**：单次 CSV upload 的不可变解析结果；外部以 `import_id` 标识；Server 内部 candidate digest/revision 仅存在于 encrypted staging / secret-safe persistence。
- **Import preview / import diff**：Server 对 candidate 与 confirmed baseline 的 redacted 结构化比较结果；Server 是 diff classification 的唯一权威；client/UI 只渲染结构化结果。
- **Import Commit**：二次确认动作；不新增独立 confirmation resource。须区分 **material** 与 **no-op**（见下）。
- **Import Discard**：Operator 对尚未提交的 candidate 的显式放弃动作；按 `import_id` 将 candidate 转入终端 `DISCARDED`，使对应 `preview_token`/evidence 对 commit 失效；不改变 confirmed configuration、binding、revision 或 Target。
- **preview token**：Server 签发或持久化的 opaque 证据句柄；普通 surface 只持有该 opaque token，不暴露 password-derived digest/fingerprint/length 或其他可作为离线猜测 oracle 的 candidate 内容证据。
- **preview evidence（签发时不可变）**：与 `preview_token` 对应、在 preview 签发时刻冻结的完整证据。绑定字段至少包括：
  - candidate identity（外部 `import_id` 及内部 candidate identity）；
  - baseline `ContestConfigurationRevision`；
  - 完整 redacted diff（含逐项分类与汇总）；
  - 精确 binding impact 集合，每一项为 `(SeatCode, DevicePk 或允许展示的非秘密 Device identity, AssignmentRevision, UNBIND_ON_COMMIT)`；
  - preview actor / authorization context；
  - expiry。
  签发后，stored evidence、redacted `summary`/items、binding impact 集合均不得再被原地修改；若需新结果，必须签发新 preview / 新 token。
- Commit 另需 `idempotency_key` 与 `correlation_id`。Commit 不依赖 Browser 重新计算 hash 或 diff。

Baseline：

- baseline kind：`NONE`（`ContestConfigurationRevision = 0` / 空集合）或 `CONFIRMED`（当前已确认集合）；CAS 以单调 `ContestConfigurationRevision` 为准。
- 初次 import 的 baseline 为 `NONE`；所有有效 Seat 均为 `ADDED`。
- 后续 import 允许新增、删除和修改 Seat/account/password。
- Seat code 是身份；rename 表示为 `REMOVED + ADDED`，不引入 identity mapping。
- 保留的 Seat code 即使 account/password 改变，也保持现有 binding。

Commit 前置与非变更结果：

- 任何 `INVALID` 行（含重复 account、空/仅 header candidate 等）阻止 commit（`commit_allowed = false`）；Operator 必须修复文件并创建新 candidate。
- **幂等预检（step 0，先于一切 live 校验与突变）**：在 authorization 重验、preview evidence 相等校验、baseline CAS、binding impact 重算/比较，以及任何 mutation **之前**，先按 `idempotency_key` 与 commit 语义 body 做预检：
  - 已存在 **COMMITTED** 记录且 key 与语义 body 均相同 → 直接返回已存储的业务结果，**零副作用**（不重跑 authorization/evidence/CAS/binding 重算，不解绑，不替换 configuration，不写 outbox，不提升任何 revision）；
  - 已存在 **COMMITTED** 记录但 key/body 不匹配（同 key 不同 body，或同 body 语义与已存记录冲突）→ `idempotency conflict`；
  - 已存在**非终态**（in-flight / non-terminal）同 key 记录但 body 不同 → `idempotency conflict`；
  - 仅**首次**执行或判定为尚未落定成功结果的非重放路径，才进入后续 live validation 与 mutation。
- **成功后重试接受语义**：material commit 成功后可能已提升 `ContestConfigurationRevision` / `AssignmentRevision` 并完成 unbind；此后对**相同 key + 相同语义 body** 的重试仍须在 step 0 命中已存储成功结果并原样返回，即使当前 live baseline/binding 已与该次 commit 前不同。不得因 post-success 的 revision bump 或 unbind 而误报 stale baseline / binding-stale。对**不同** key 的新 commit 才走 live CAS/binding 校验。
- Commit（首次/非重放路径）必须对 preview 签发时冻结的 evidence 做**逐字段相等**校验（candidate identity、baseline revision、完整 redacted diff、精确 binding impact 集合、preview actor/authz context、expiry 语义），并在 transaction 内**重验当前 authorization**；evidence 不相等或 token 无法解析到该不可变证据时拒绝，且不得改变 confirmed truth。
- **Binding freshness**：preview evidence 绑定签发时精确且不可变的 binding impact 集合，每一项为 `(SeatCode, DevicePk 或允许展示的非秘密 Device identity, AssignmentRevision, UNBIND_ON_COMMIT)`。Commit 必须对全部将被 `REMOVED` 的 Seat **重算当前** binding impacts，并与 evidence 中的集合做**精确相等**比较（不得多、不得少、不得改 identity/revision/动作）。任一新增、缺失或变更的 binding/revision 均视为 binding-stale / preview-mismatch，拒绝 commit 并要求重新 preview。Commit **仅可**解绑该 preview 已授权的精确集合，不得解绑集合外 Device。本 freshness 校验只适用于 step 0 之后的首次/非重放执行路径。
- Stale baseline、binding-stale、expired candidate、preview token/evidence mismatch、discard、失败 transaction、UI disconnect 均不得改变 confirmed configuration、binding、Target truth 或相关 revision。
- **`is_noop = true`** 当且仅当：不存在 `ADDED`/`REMOVED`/account 变更/password 变更，不存在任何 `INVALID`，且 binding impact 集合为空；全部有效项均为 `UNCHANGED`。No-op import 仍需二次确认并记录 import lineage/redacted audit，但**不**提升 contest configuration、credential 或 assignment revision（`before_revision == after_revision`），**不**执行 unbind-and-replace，**不**写内容变化 ChangeEvent/outbox，**不**产生 Target churn。

Import Commit 的 transaction 顺序（仅 step 0 判定为首次/非重放后执行）：

0. **幂等预检**（见上）；命中已 COMMITTED 同 key/同 body 则直接返回存储结果并结束；
1. transaction 内重验 authorization、candidate state（非 `DISCARDED`/非已终态不可 commit）、preview token 与不可变 evidence 相等性、expiry、baseline `ContestConfigurationRevision`，并重算 REMOVED Seat 的当前 binding impacts 做精确集合相等比较；
2. **分支 — material**（`is_noop = false` 且内容将实际变化）：
   - 仅当 preview 已授权的 binding impact 集合非空时，解绑该精确集合中的 Device 并提升对应 `AssignmentRevision`；
   - 完整替换 Seat/account/credential metadata 为 candidate 集合（合法 account swap 须在同一 transaction 内以 clear-then-apply 或等价顺序落实，避免非延迟唯一约束下的假冲突）；
   - 仅实际 password 变化提升 `CredentialRevision`；
   - 仅内容实际变化提升 `ContestConfigurationRevision` 并更新非秘密内部 content hash；
   - 标记 import terminal `COMMITTED`；
   - 原子写入 redacted AuditEvent；仅在内容实际变化时写入 ChangeEvent/outbox；
   - commit。
3. **分支 — no-op**（`is_noop = true`）：
   - **不**执行 unbind-and-replace，**不**替换 confirmed configuration 内容；
   - **不**提升 contest configuration、credential 或 assignment revision（响应中 `before_revision == after_revision`）；
   - 标记 import terminal `COMMITTED`（no-op 成功）；
   - 仅原子写入 import lineage 与 redacted AuditEvent（及幂等/terminal 所需非内容元数据）；
   - **不**写内容变化 ChangeEvent/outbox，**不**触发 Target churn；
   - commit。

#### Import Discard

**Import Discard** 是与 Import Commit 并列的显式领域动作，不是失败状态的附带描述。

- 输入：`import_id`（及边界层要求的 actor/auth 上下文；actor 不得由 client 自报为可信任主体）。
- 校验：重验 actor authorization；`import_id` 必须指向既有 candidate。
- 效果：
  - 将 candidate 转入终端状态 `DISCARDED`；
  - 使该 candidate 上已签发的 `preview_token`/preview evidence **对后续 Import Commit 失效**（不得再用于 commit）；
  - **不**改变 confirmed contest configuration、binding、任何相关 revision 或 Target；
  - **不**创建 Operation/Command，不产生 Device I/O，不写内容变化 ChangeEvent/outbox；
  - encrypted staging 清理遵循安全策略（discard 触发清理、expiry/cleanup job 或等价策略）；不得因 discard 把明文或 password-derived 材料泄漏到 ordinary surface。
- 幂等：对**已经** `DISCARDED` 的同一 `import_id` 再次 Discard 必须幂等成功（或等价无副作用确认），不改变 confirmed truth。
- 禁止：Discard **不得**撤销、回滚或覆盖已 `COMMITTED` 的 import；对已提交 import 的 Discard 必须拒绝（稳定冲突/非法状态），且 confirmed truth 不变。

始终：

- `AUTO_COMMAND_COUNT = 0`；
- Import Commit / Discard 均不创建 Operation/Command，不自动执行 `SYNC_STATE` 或 `SYNC_SECRET`，不产生 Device I/O；
- **Material** Import Commit 只改变 Server truth；由此产生的 Target/Drift 变化不代表 Device 已同步；
- **No-op** Import Commit 与 **Import Discard** 均不改变 confirmed configuration 内容，也不制造 Target churn。

### 4.3 Preview 分类

Server 权威 taxonomy（完整枚举）：

- `ADDED`
- `REMOVED`
- `ACCOUNT_CHANGED`
- `PASSWORD_CHANGED`
- `ACCOUNT_AND_PASSWORD_CHANGED`
- `UNCHANGED`
- `INVALID`

`INVALID` 至少覆盖：列/编码/唯一性等结构性错误、规范化失败、**candidate 内重复 account**、**空或仅 header candidate**，以及其他使 candidate 不可提交的输入错误。任一 `INVALID` 即 `commit_allowed = false`。

Preview 汇总至少包含：

- 各分类数量；
- `is_noop`（定义见 §4.2：无 `ADDED`/`REMOVED`/account/password 变更、无 `INVALID`、无 binding impact，全部有效项 `UNCHANGED`）；
- binding impacts 与 count（count 为 0 时仍须显式给出空集合）；
- `commit_allowed`；
- `blocking_reasons`。

Binding impact 每一项在 preview 签发时冻结，至少包含：

- `SeatCode`；
- `DevicePk` 或允许展示的非秘密 Device identity；
- 当前 `AssignmentRevision`；
- 动作 `UNBIND_ON_COMMIT`。

删除当前已绑定 Seat 时，preview 必须列出**全部** binding impacts，并纳入不可变 preview evidence。确认后在同一 Server transaction 中：重算并校验 impact 集合精确相等 → **仅**解绑该精确集合 → 提升对应 `AssignmentRevision` → 再替换集合。

不存在 `SEAT_SET_MISMATCH` 硬错误：Seat 集合差异通过 `ADDED`/`REMOVED` 表达。分类是纯计算结果，不产生副作用。UI 不得通过字符串比较或本地重算自行重建该分类。普通 surface 只使用 opaque `preview_token` 与 redacted 证据，不暴露 password-derived digest。

## 5. `Device`

### 5.1 职责

拥有：

- `DevicePk`；
- 生命周期；
- 当前 `MachineHardwareId`；
- Enrollment/Device certificate 元数据；
- 最后连接和最后 Observed 定位；
- 删除/替换审计关联。

不拥有：

- Seat；
- 密码；
- Gateway private key；
- Caddy runtime config；
- Session 实例。

### 5.2 生命周期

推荐逻辑状态：

```text
DISCOVERED/ENROLLING
  → ACTIVE
  → RETIRED
  → DELETED
```

实际实现可以使用更少状态，只要满足：

- Enrollment 未成功前不能建立 authenticated control；
- `ACTIVE` Device 才能成为 binding 目标；
- `RETIRED` 不接受新 Command；
- 删除不会让旧证书或本地 vault 自动成为“新 Device”；
- re-enroll 创建受审计的新生命周期，而不是 merge。

### 5.3 硬件身份冲突

同一个 `MachineHardwareId` 不得同时对应两个 active Device。

发现冲突时：

- 不自动合并；
- 不选择“最近上线者”；
- 不删除 vault；
- 停止 Enrollment/control 的敏感进展；
- 返回稳定错误并要求人工执行替换/恢复 runbook。

## 6. `Binding`

### 6.1 职责

Binding 表示：

```text
SeatCode ↔ DevicePk
```

并携带 `AssignmentRevision`。

### 6.2 不变量

- 一个 Seat 同时最多绑定一台 active Device；
- 一台 Device 同时最多绑定一个 Seat；
- 只有当前 confirmed contest configuration 中的 Seat 可处于 bound；
- 只有 active Device 可绑定；
- 每次有效变化增加 assignment revision；
- unbind 也是显式领域操作；Import Commit 可在同一 transaction 中对将被删除的 Seat 执行 atomic unbind-and-replace；
- 保留的 Seat code 上的 binding 在 account/password 变化时保持不变；
- binding 修改只改变 Server truth 和 Target，不自动同步 Device；
- secret sync 必须绑定发起时的 Seat、Device 和 assignment revision。

### 6.3 事务边界

一次 binding 变更原子提交：

- Binding；
- AssignmentRevision；
- AuditEvent；
- ChangeEvent/outbox。

Import Commit 触发的 unbind-and-replace 另见 §4.2：binding、confirmed configuration、相关 revision、redacted audit 与 outbox 必须在同一 Server transaction 中提交或全部回滚。

不在该事务中：

- 创建网络连接；
- 发送 Command；
- 更新 Observed；
- 修改 Client vault；
- reload Caddy。

## 7. Credential

密码明文不作为普通 `Account` 字段暴露。

领域只公开：

- 账号；
- `CredentialRevision`；
- 是否存在可用秘密；
- 最后更新时间；
- 受限的 secret handle 或 use-case token。

读取密码的 application use case 必须：

1. 通过 operator authorization；
2. 绑定明确 `SYNC_SECRET`；
3. 读取当前 assignment 和 credential revision；
4. 在最短生命周期内解密；
5. 不进入普通结构、日志或事件；
6. 发送后清零可清零缓冲；
7. 只记录 redacted 审计元数据。

## 8. `DeviceTarget`

### 8.1 定义

Target 是根据已提交 Server truth 为某台 Device 计算的非秘密期望状态。

Target 可以包含：

- Device/Seat/account 的非秘密关联；
- `AssignmentRevision`；
- `ConfigurationGeneration`；
- 固定 upstream/hostname/profile 的派生标识；
- Caddy 配置所需的非秘密策略；
- 是否需要当前 credential revision 的元数据；
- session/home 策略；
- 可比较的目标 hash。

Target 不包含：

- password；
- private key；
- Server vault ciphertext；
- 任意 shell、路径、UID、unit、环境或自由格式 Caddy fragment；
- CSR 自报 SAN 的授权含义。

### 8.2 生成

Target 生成必须是可重放纯计算或近似纯计算：

```text
Server truth + frozen policy → DeviceTarget
```

同一输入应产生同一语义 Target。Target 持久化与缓存方式是实现细节，但 generation 必须可审计。

### 8.3 惰性

Target 变化不自动创建 Command。操作员必须显式创建 `SYNC_STATE`。

## 9. `DeviceObserved`

Observed snapshot 是 Device 对自身实际状态的 typed 报告。

至少应能表达独立维度：

- Machine identity 状态；
- Device certificate 状态；
- Gateway certificate 状态；
- applied assignment revision；
- applied configuration generation；
- installed credential revision（不得含秘密）；
- Caddy `BLOCKED`/`READY` 和 config hash；
- upstream health 的有限状态；
- Session/Home epoch 和状态；
- LKG 状态；
- 最近稳定 ErrorCode；
- snapshot 生成时间和 boot/session identity。

不得用单个 `READY` 覆盖全部维度。

Observed 可能陈旧。Server 必须保留接收时间和 freshness 语义，并在 UI 中区分：

- 未观察；
- 当前；
- 陈旧；
- 设备离线；
- 明确失败。

## 10. `Drift`

Drift 是纯比较结果：

```text
Drift = compare(Target, latest valid Observed)
```

Drift 不应持有独立业务真相。可以缓存，但必须能从 Target 和 Observed 重算。

建议维度：

- assignment drift；
- configuration drift；
- credential revision drift；
- Gateway certificate drift；
- Caddy activation drift；
- session/home drift；
- unknown due to stale/missing observation。

“无 Drift”不等于设备在线，也不等于全部安全证据有效。

## 11. `Operation`

Operation 只用于需要以下至少一种能力的动作：

- 多 Device 批量聚合；
- 异步进度；
- 重试/取消的 operator 视图；
- 统一的业务结果和审计关联。

普通同步领域 CRUD 不必须创建 Operation。

Operation 状态建议：

```text
PENDING → RUNNING → SUCCEEDED
                  ↘ PARTIAL
                  ↘ FAILED
          ↘ CANCELED
```

Operation 状态由其 OperationTarget/Command 结果归约，不由 HTTP request 直接设置为成功。

## 12. `Command`

Command 是面向单台 Device 的 durable intent。

必须包含：

- `CommandId`；
- command kind；
- target Device；
- 创建时冻结的必要 revision/generation/epoch；
- payload 的 typed 版本；
- 生命周期；
- redacted result；
- audit correlation。

Command receipt 在 Device 持久化前不得确认。相同 `CommandId` 必须返回相同业务结果或当前进行状态，不得重复副作用。

Command 与 Attempt 详见 [状态与执行模型](state-and-execution.md)。

## 13. `AuditEvent` 与 `ChangeEvent`

### 13.1 AuditEvent

记录：

- 谁；
- 在何时；
- 对哪个资源；
- 执行什么动作；
- 结果；
- correlation ID；
- redacted 差异；
- 相关 Operation/Command；
- evidence locator（如适用）。

Import 相关 AuditEvent 仅可记录 redacted 分类、数量、受影响 Seat/Device identity 与 before/after revision，以及 import lineage 标识；不得记录 password、raw CSV、password-derived digest 或完整 candidate 秘密材料。

不得记录密码、private key、完整证书私密材料、任意上传原文或未脱敏错误链。

### 13.2 ChangeEvent/outbox

用于把已提交领域变化可靠通知给：

- SSE；
- Target invalidation；
- dispatcher；
- 其他内部消费者。

规则：

- 与领域事务原子写入；
- 消费至少一次时必须幂等；
- 不得把 ChangeEvent 当作审计替代；
- 不得包含秘密；
- 消费失败不回滚已经提交的领域事实。

## 14. 本地运行时领域

### 14.1 Machine identity startup

输入：

- 当前硬件来源；
- 已持久化 Machine Hardware ID；
- identity-bound artifacts 是否存在；
- vault 可否解密。

输出是封闭决策，例如：

- `FIRST_BOOT_ALLOWED`
- `IDENTITY_MATCH`
- `IDENTITY_UNAVAILABLE_FAIL_CLOSED`
- `IDENTITY_MISMATCH_FAIL_CLOSED`
- `VAULT_DECRYPT_FAILED`
- `RECOVERY_REQUIRED`

不能输出“猜测最可能是同一台机器并继续”。

### 14.2 Session

Session 使用 `SessionEpoch` 绑定：

- logind session；
- contest UID；
- seat；
- graphical session type；
- boot ID；
- Agent lease。

所有 lock/unlock/terminate/UI action 都必须校验当前 epoch。陈旧 Agent 或陈旧 UI action 被拒绝。

### 14.3 Home

Home transaction 使用 `HomeEpoch`，至少经历：

```text
NOT_PREPARED
  → PREPARING
  → PREPARED
  → ACTIVE
  → CLEANING
  → CLEAN
```

失败进入 `FAILED_REQUIRES_RECOVERY` 或等价状态。无法证明 Home 安全时不得开始受管 session。

后端在部署时选择：

- OverlayFS；或
- staged-copy。

运行时不得静默从一个后端切换到另一个后端。

## 15. 删除、重置和替换

### Device 删除

- Server 侧显式授权；
- 停止新 Command；
- 撤销或标记证书；
- 解除 binding；
- 记录审计；
- Client 本地清理按 runbook 执行；
- 不复用旧 `DevicePk`。

### Device 替换

- 原 Device lifecycle 结束；
- 新硬件独立 Enrollment；
- 人工重建 binding；
- 显式 `SYNC_STATE` 和 `SYNC_SECRET`；
- 不复制 Device private key 或 Client vault。

### 单生命周期竞赛重置

- 通过破坏性 runbook 清理业务状态和秘密；
- 不在业务模型中创建历史 Event；
- 保留或导出审计需由部署策略明确；
- 重置后 confirmed contest configuration 为空，`ContestConfigurationRevision = 0`；
- 下一次 import 仍走普通 first-import lifecycle（baseline 为空集合），不引入特殊分支。

## 16. 领域测试最低要求

每个聚合至少具有：

- value object 边界测试；
- 状态转移正向测试；
- 陈旧 revision/generation/epoch 拒绝测试；
- 事务回滚测试；
- audit/outbox 原子性测试；
- secret redaction 测试；
- property-based 或 table-driven 不变量测试；
- adapter 与 domain error 的穷举映射测试。

跨聚合集成测试重点：

- first / no-op / material Import Commit 语义与 revision 规则；
- invalid、stale baseline、expiry、preview token mismatch、discard、transaction failure 均不改变 confirmed truth；
- 幂等预检为 step 0：已 COMMITTED 同 key/同 body 在 CAS/binding 重算之前返回存储结果；post-success 在 revision bump/unbind 之后的同 key 重试仍零副作用成功；key/body 冲突返回 idempotency conflict；
- explicit Import Discard：terminal `DISCARDED`、token 失效、confirmed truth 不变；对已 discarded 幂等；不得撤销已 COMMITTED import；
- no-op：`before_revision == after_revision`，无 unbind-and-replace、无内容 outbox、无 Target churn；
- atomic unbind-and-replace 与 rollback（仅 material）；
- import secret redaction（无 password-derived digest 进入普通 surface）；
- Import Commit 始终 `AUTO_COMMAND_COUNT = 0`，不创建远端副作用；
- binding 变化只使 Target/Drift 变化；
- Target/Observed 可重算 Drift；
- secret revision 不进入 Target 明文；
- Operation 只在需要时创建；
- Device replacement 不复用身份；
- session/home 陈旧 epoch 不能操作当前状态。
