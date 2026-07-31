# Natsume V2 领域模型

> 状态：`NORMATIVE`  
> 适用范围：Server 业务状态、Target/Observed 语义和本地运行时身份  
> 不包含：数据库列级 schema、Protobuf 字段编号、HTTP 路由

数据库 migration 是物理 schema 的权威来源；本文件定义稳定的业务含义、聚合边界和安全不变量。未实现行为的具体字段、状态枚举与事务编排延迟到对应 Phase 实现时定义。

## 1. 建模原则

1. 一个实例只建模当前一场竞赛，不创建 `Event` 聚合。
2. Confirmed contest configuration 只能通过完整 candidate 的显式 Import Commit 被替换。
3. 内部主键与外部/硬件标识分离。
4. **密码是秘密值，不是普通实体属性。**
5. Target、Observed、Drift、Operation 和 Command 含义相互独立。
6. **远端副作用不在普通领域事务中“假装完成”。**
7. **删除、重置和替换必须显式，不能通过 identity fallback 隐式发生。**
8. **所有陈旧性判断使用单调 revision/generation/epoch，而不是时间戳猜测。**

## 2. 标识和值对象

领域使用一组稳定值对象区分业务身份与硬件标识，并区分内容/凭据/绑定/配置/会话各维度的单调 revision（`ContestConfigurationRevision`、`CredentialRevision`、`AssignmentRevision`、`ConfigurationGeneration`、`SessionEpoch`、`HomeEpoch` 等）。内部主键（`DevicePk`，UUIDv7）不得从硬件数据推导；硬件身份（`MachineHardwareId`，UUIDv5）不是认证凭据。

值对象必须在进入 domain 前完成结构校验。**Domain 不解析自由格式路径、URL、shell、证书文本或 UI 文案。**

## 3. ContestConfiguration import

每个 `seat,account,password` CSV 都是完整的 contest configuration candidate，不是增量 patch。安全与不变量边界：

- 只接受固定三列 UTF-8 CSV（可带 BOM）；不接受额外列、XLSX/ODS、公式、列映射或自动猜测；
- `password` 只进入加密 staging 和 secret commit path；
- **不得向普通 API、Browser、audit、log、metric、SSE 或 outbox 暴露 raw CSV hash、password fingerprint、password length 或其他 password-derived digest；**
- 任何 `INVALID`（结构性错误、candidate 内重复 account、空或仅 header candidate）、stale baseline、binding-stale、expiry、preview mismatch 或 discard 均**不改变 confirmed configuration、binding、Target truth 或相关 revision；**
- Import Commit 不创建 Operation/Command，不自动执行 `SYNC_STATE` 或 `SYNC_SECRET`，不产生 Device I/O；material import 改变 Server truth 后的 Target/Drift 变化不代表 Device 已同步；
- 清空 confirmed configuration 只能通过独立的 single-lifetime reset，不得由 import 隐式完成。

完整 import lifecycle、preview evidence、baseline CAS、atomic unbind-and-replace、idempotency 与 diff taxonomy 的具体字段和顺序在 Phase 2 实现时定义。

## 4. Device

- 一个 `MachineHardwareId` 不得同时对应两个 active Device。
- 硬件身份冲突时：**不自动合并、不选择“最近上线者”、不删除 vault**，停止敏感进展并返回稳定错误，要求人工执行恢复 runbook。
- Device 删除/替换不复用 `DevicePk`、Device private key 或 Client vault；re-enroll 创建受审计的新生命周期，而不是 merge。

## 5. Binding

- 一个 Seat 同时最多绑定一台 active Device，反之亦然；
- 只有当前 confirmed contest configuration 中的 Seat 与 active Device 可绑定；
- 每次有效绑定变化增加 `AssignmentRevision`；unbind 也是显式领域操作；
- 保留的 Seat code 上的 binding 在 account/password 变化时保持不变；
- **binding 修改只改变 Server truth 和 Target，不自动同步 Device；**
- secret sync 必须绑定发起时的 Seat、Device 和 assignment revision。

## 6. Credential

密码明文不作为普通 `Account` 字段暴露。领域只公开账号、`CredentialRevision`、是否存在可用秘密、最后更新时间与受限 secret handle。读取密码的 application use case 必须：

1. 通过 operator authorization；
2. 绑定明确 `SYNC_SECRET`；
3. 读取当前 assignment 和 credential revision；
4. 在最短生命周期内解密；
5. 不进入普通结构、日志或事件；
6. 发送后清零可清零缓冲；
7. 只记录 redacted 审计元数据。

## 7. DeviceTarget

Target 是根据已提交 Server truth 为某台 Device 计算的**非秘密**期望状态。Target **不包含** password、private key、vault ciphertext、任意 shell/路径/UID/unit/环境或自由格式 Caddy fragment，也不赋予 CSR 自报 SAN 授权含义。Target 生成是可重放的纯计算（`Server truth + frozen policy → DeviceTarget`）。**Target 变化不自动创建 Command**；操作员必须显式创建 `SYNC_STATE`。

## 8. DeviceObserved

Observed snapshot 是 Device 对自身实际状态的 typed 报告。**Observed 不得携带秘密；Device 自报的属性不构成授权。** Observed 可能陈旧；Server 保留接收时间和 freshness 语义，不得用单个 `READY` 覆盖全部维度。

## 9. Drift

Drift 是纯比较结果（`compare(Target, latest valid Observed)`），不持有独立业务真相，可从 Target 和 Observed 重算。“无 Drift”不等于设备在线，也不等于全部安全证据有效。

## 10. Operation 与 Command

- **Operation** 只用于需要批量聚合、异步进度、重试/取消视图或统一业务结果的动作；普通同步 CRUD 不创建 Operation。
- **Command** 是面向单台 Device 的 durable intent。**Command receipt 在 Device 持久化前不得确认；相同 `CommandId` 必须返回相同业务结果，不得重复副作用。** Command 创建时冻结必要 revision/generation/epoch；陈旧时用稳定错误拒绝，不“尽量兼容”地部分应用。

Operation/Command 的具体状态机、字段与归约规则见 [状态与执行模型](state-and-execution.md)，在 Phase 4 实现时定义。

## 11. AuditEvent 与 ChangeEvent/outbox

- **AuditEvent** 记录谁、何时、对哪个资源、执行什么动作、结果、correlation、redacted 差异与 evidence locator。敏感变更必须有 redacted audit；**不得记录密码、private key、完整证书私密材料、任意上传原文或未脱敏错误链。**
- **ChangeEvent/outbox** 与领域事务原子写入，把已提交变化可靠通知内部消费者；不得包含秘密，不得当作审计替代。消费失败不回滚已提交事实。

## 12. 本地运行时领域

- **Machine identity startup**：identity 检查先于 vault。决策是封闭枚举（如 `FIRST_BOOT_ALLOWED`、`IDENTITY_MATCH`、`IDENTITY_UNAVAILABLE_FAIL_CLOSED`、`IDENTITY_MISMATCH_FAIL_CLOSED`、`VAULT_DECRYPT_FAILED`、`RECOVERY_REQUIRED`）。**不能输出“猜测最可能是同一台机器并继续”。**
- **Session**：所有 lock/unlock/terminate/UI action 都必须校验当前 `SessionEpoch`；陈旧 Agent 或陈旧 UI action 被拒绝。
- **Home**：无法证明 Home 安全时不得开始受管 session；后端（OverlayFS 或 staged-copy）在部署时选择，**运行时不静默切换**。

## 13. 删除、重置和替换

- **Device 删除**：Server 显式授权、停止新 Command、撤销/标记证书、解除 binding、记录审计；不复用旧 `DevicePk`。
- **Device 替换**：原 lifecycle 结束、新硬件独立 Enrollment、人工重建 binding、显式 `SYNC_STATE`/`SYNC_SECRET`；不复制 Device private key 或 Client vault。
- **单生命周期竞赛重置**：通过破坏性 runbook 清理业务状态和秘密；重置后 `ContestConfigurationRevision = 0`，下一次 import 走普通 first-import lifecycle。

## 14. 领域测试最低要求

每个聚合至少覆盖：value object 边界、正向状态转移、陈旧 revision/generation/epoch 拒绝、事务回滚、audit/outbox 原子性、secret redaction 与 adapter 错误穷举映射。具体场景随对应 Phase 实现补全。
