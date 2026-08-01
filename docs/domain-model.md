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
5. Target、Observed、Drift 和 Command 含义相互独立。
6. **远端副作用不在普通领域事务中"假装完成"。**
7. **删除、重置和替换必须显式，不能通过 identity fallback 隐式发生。**
8. **所有陈旧性判断使用单调 revision/epoch，而不是时间戳猜测。**

## 2. 标识和值对象

领域使用一组稳定值对象区分业务身份与硬件标识。独立单调计数器限于（[ADR-0029](adr/0029-right-sizing-control-plane-machinery.md)）：

- `ContestConfigurationRevision`：confirmed 内容修订，import baseline CAS token；
- `AssignmentRevision`：binding 修订；
- `CredentialRevision`：账号秘密修订；
- `SessionEpoch` / `HomeEpoch`：本地运行时代际。

面向单台 Device 的非秘密配置代际由 `(ContestConfigurationRevision, 站点 policy 版本)` **确定性派生**，不独立计数；Device 侧仍按代际拒绝陈旧 `SYNC_STATE`。

内部主键（`DevicePk`，UUIDv7）不得从硬件数据推导；硬件身份（`MachineHardwareId`，UUIDv5，派生配方见 [ADR-0025](adr/0025-deterministic-hardware-identity-recipe.md)）不是认证凭据。

值对象必须在进入 domain 前完成结构校验。**Domain 不解析自由格式路径、URL、shell、证书文本或 UI 文案。**

## 3. ContestConfiguration import

每个 `seat,account,password` CSV 都是完整的 contest configuration candidate，不是增量 patch。边界规则（并发模型见 [ADR-0028](adr/0028-single-operator-import-and-secret-evidence-scope.md)）：

- 只接受固定三列 UTF-8 CSV（可带 BOM）；不接受额外列、XLSX/ODS、公式、列映射或自动猜测；
- **全局同一时刻最多一个 pending candidate**；新 upload 前需显式 discard；
- `password` 只进入加密 staging 和 secret commit path，明文不进任何普通 surface；
- Commit 校验为双 CAS：baseline `ContestConfigurationRevision` + `AssignmentRevision`；任一失配 → 拒绝并重新 preview，**不改变 confirmed configuration、binding、Target truth 或相关 revision**；
- 任何 `INVALID`（结构性错误、candidate 内重复 account、空或仅 header candidate）、expiry 或 discard 同样不改变上述任何状态；
- Import Commit 不创建 Command，不自动执行 `SYNC_STATE` 或 `SYNC_SECRET`，不产生 Device I/O；
- 清空 confirmed configuration 只能通过独立的 single-lifetime reset，不得由 import 隐式完成。

完整 import lifecycle、preview evidence 与 diff taxonomy 的具体字段在 Phase 2 实现时定义。

## 4. Device

- 一个 `MachineHardwareId` 不得同时对应两个 active Device。
- provisioning 窗口内同一 `MachineHardwareId` 重复 Enrollment 为**受审计的替换**：旧 Device Token 失效、新产物签发（[ADR-0021](adr/0021-provisioning-window-certificate-issuance.md)）；若旧连接仍存活，记录异常审计事件。
- 窗口外的硬件身份冲突：**不自动合并、不选择"最近上线者"、不删除凭据**，停止敏感进展并返回稳定错误，要求人工执行恢复 runbook。
- Device 删除/替换不复用 `DevicePk` 或凭据文件；替换走窗口重开 + 新 Enrollment 的受审计生命周期，而不是 merge。

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
4. 在最短生命周期内解密并发送；
5. 不进入普通结构、日志或事件；
6. 只记录 redacted 审计元数据。

## 7. DeviceTarget

Target 是根据已提交 Server truth 为某台 Device 计算的**非秘密**期望状态。Target **不包含** password、private key、token、任意 shell/路径/UID/unit/环境或自由格式 Caddy fragment。Target 生成是可重放的纯计算（`Server truth + frozen policy → DeviceTarget`），其代际由输入 revision 派生。**Target 变化不自动创建 Command**；操作员必须显式创建 `SYNC_STATE`。

## 8. DeviceObserved

Observed snapshot 是 Device 对自身实际状态的 typed 报告。**Observed 不得携带秘密；Device 自报的属性不构成授权。** Observed 可能陈旧；Server 保留接收时间和 freshness 语义，不得用单个 `READY` 覆盖全部维度。上报节奏为变化时上报 + 低频周期兜底。

## 9. Drift

Drift 是纯比较结果（`compare(Target, latest valid Observed)`），不持有独立业务真相，可从 Target 和 Observed 重算。"无 Drift"不等于设备在线，也不等于全部安全证据有效。

## 10. Command

**Command** 是面向单台 Device 的 durable intent。**Command receipt 在 Device 持久化前不得确认；相同 `CommandId` 必须返回相同业务结果，不得重复副作用。** Command 创建时冻结必要 revision/epoch；陈旧时用稳定错误拒绝，不"尽量兼容"地部分应用。

批量操作 = 批量创建 Command，进度视图由查询聚合；投递/重试观察记录为 Command 元数据。跨设备聚合业务层（Operation）已延后，引入须新 ADR（[ADR-0029](adr/0029-right-sizing-control-plane-machinery.md)）。

Command 的具体状态机与字段见 [状态与执行模型](state-and-execution.md)，在 Phase 4 实现时定义。

## 11. AuditEvent

**AuditEvent** 记录谁、何时、对哪个资源、执行什么动作、结果、correlation、redacted 差异与 evidence locator，**与领域事务同事务原子写入**；audit 写失败则整个事务回滚。敏感变更必须有 redacted audit；**不得记录密码、private key、Device Token 值、任意上传原文或未脱敏错误链。**

进程外事件分发（outbox/SSE）已删除；Web Panel 以轮询获取状态（[ADR-0029](adr/0029-right-sizing-control-plane-machinery.md)）。

## 12. 本地运行时领域

- **Machine identity startup**：identity 检查先于一切 identity-bound 产物使用。决策是封闭枚举（如 `FIRST_BOOT_ALLOWED`、`IDENTITY_MATCH`、`IDENTITY_UNAVAILABLE_FAIL_CLOSED`、`IDENTITY_MISMATCH_FAIL_CLOSED`、`CREDENTIALS_UNREADABLE_FAIL_CLOSED`、`RECOVERY_REQUIRED`）。**不能输出"猜测最可能是同一台机器并继续"。**
- **Session**：所有 lock/unlock/terminate/UI action 都必须校验当前 `SessionEpoch`；陈旧 Agent 或陈旧 UI action 被拒绝。
- **Home**：开始时创建新 `HomeEpoch`；prepare 完成前不启动受管 session；cleanup 只作用于当前 epoch；**无法证明 mount/copy/ownership 安全时 fail closed；不静默切换 backend**。重置是操作员在场的受控事件，实现为状态文件 + 幂等可重跑步骤。

## 13. 删除、重置和替换

- **Device 删除**：Server 显式授权、停止新 Command、吊销 Device Token（删行）、标记证书台账、解除 binding、记录审计；不复用旧 `DevicePk`。
- **Device 替换**：原 lifecycle 结束、重开 provisioning 窗口、新硬件独立 Enrollment、人工重建 binding、显式 `SYNC_STATE`/`SYNC_SECRET`；不复制凭据文件。
- **单生命周期竞赛重置**：通过破坏性 runbook 清理业务状态和秘密；重置后 `ContestConfigurationRevision = 0`，下一次 import 走普通 first-import lifecycle。

## 14. 领域测试最低要求

每个聚合至少覆盖：value object 边界、正向状态转移、陈旧 revision/epoch 拒绝、事务回滚、audit 原子性、secret redaction 与 adapter 错误穷举映射。具体场景随对应 Phase 实现补全。
