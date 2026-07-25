# Natsume V2 领域模型

> 状态：`NORMATIVE`  
> 适用范围：Server 业务状态、Target/Observed 语义和本地运行时身份  
> 不包含：数据库列级 schema、Protobuf 字段编号、HTTP 路由

数据库 migration 是物理 schema 的权威来源；本文件定义稳定的业务含义、聚合边界和事务规则。

## 1. 建模原则

1. 一个实例只建模当前一场竞赛，不创建 `Event` 聚合。
2. Seat universe 在首次成功 CSV commit 后冻结。
3. 内部主键与外部/硬件标识分离。
4. 密码是秘密值，不是普通实体属性。
5. Target、Observed、Drift、Operation 和 Command 含义相互独立。
6. 远端副作用不在普通领域事务中“假装完成”。
7. 删除、重置和替换必须显式，不能通过 identity fallback 隐式发生。
8. 所有陈旧性判断使用单调 revision/generation/epoch，而不是时间戳猜测。

## 2. 标识和值对象

| 名称 | 含义 | 规则 |
|---|---|---|
| `SeatCode` | 现场席位代码 | CSV 首次 commit 后集合不可增加、删除或重命名 |
| `AccountName` | DOMjudge 账号标识 | 不含密码；按输入契约规范化 |
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
  ├─ Seat
  ├─ Account
  └─ Credential metadata

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

- Seat universe；
- account 与 Seat 的当前映射；
- credential revision；
- CSV import lineage；
- 首次 commit 冻结状态。

不拥有：

- Device；
- binding；
- Device Target；
- 密码明文的普通读取接口；
- DOMjudge 运行时状态。

### 4.2 CSV 提交规则

CSV 只接受一个 UTF-8 文本文件，可允许 UTF-8 BOM，列必须恰好为：

```text
seat,account,password
```

每行语义：

- `seat` 必须唯一；
- `account` 必须满足冻结的规范化规则；
- `password` 只能进入加密 staging 和 secret commit path；
- 不接受额外列、公式、sheet、列映射或自动猜测。

首次成功 commit：

1. 建立 Seat universe；
2. 冻结 Seat code 集合；
3. 建立账号和 credential revision；
4. 记录 AuditEvent 和 ChangeEvent；
5. 不创建远端 Command。

后续 commit：

- Seat 集合必须完全相同；
- account/password 可以按明确 preview action 更新；
- unchanged 行不增加 revision；
- password 变化增加 `CredentialRevision`；
- commit 必须全有或全无。

### 4.3 Preview 分类

Preview 至少区分：

- `UNCHANGED`
- `ACCOUNT_CHANGED`
- `PASSWORD_CHANGED`
- `ACCOUNT_AND_PASSWORD_CHANGED`
- `INVALID`
- `SEAT_SET_MISMATCH`

分类是纯计算结果，不产生副作用。UI 不得通过字符串比较自行重建该分类。

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
- 只有冻结 Seat universe 中的 Seat 可绑定；
- 只有 active Device 可绑定；
- 每次有效变化增加 assignment revision；
- unbind 也是显式领域操作；
- binding 修改只改变 Server truth 和 Target，不自动同步 Device；
- secret sync 必须绑定发起时的 Seat、Device 和 assignment revision。

### 6.3 事务边界

一次 binding 变更原子提交：

- Binding；
- AssignmentRevision；
- AuditEvent；
- ChangeEvent/outbox。

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
- 重置后重新初始化 Seat universe。

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

- CSV commit 不创建远端副作用；
- binding 变化只使 Target/Drift 变化；
- Target/Observed 可重算 Drift；
- secret revision 不进入 Target 明文；
- Operation 只在需要时创建；
- Device replacement 不复用身份；
- session/home 陈旧 epoch 不能操作当前状态。
