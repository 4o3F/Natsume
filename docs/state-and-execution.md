# Natsume V2 状态与执行模型

> 状态：`NORMATIVE`  
> 适用范围：Target、Observed、Drift、Operation、Command、Attempt、Caddy、Session 和 Home  
> 相关不变量：`INV-STATE-01`、`INV-SECRET-02`、`INV-COMMAND-01`、`INV-DATAPLANE-01`、`INV-SESSION-01`

## 1. 为什么分离状态和副作用

Natsume 同时处理：

- Server 已提交事实；
- 面向 Device 的期望状态；
- Device 实际状态；
- 人工触发的远端动作；
- 网络投递和重试；
- 本地原子激活；
- 长期审计。

这些概念如果压缩成一个“device status”或一个通用任务表，会产生高耦合：普通 CRUD 依赖网络、重试改变业务事实、UI 文案变成状态机输入。V2 因此严格分离以下层次。

## 2. 六层模型

```text
Server truth
    ↓ pure/controlled derivation
Target
    ↔ compare
Observed
    ↓
Drift

Human intent
    ↓
Operation? → Command → Attempt(s)
                         ↓
                    Device execution
                         ↓
                     Observed
```

- Server truth：已提交领域事实；
- Target：非秘密期望；
- Observed：实际观察；
- Drift：纯比较；
- Operation：可选的业务聚合；
- Command：单 Device durable intent；
- Attempt：投递/执行观察。

## 3. Server truth 与 Target

### 3.1 事务提交

以下动作只提交领域事实：

- Import Commit（CSV candidate 的显式二次确认）；
- Device lifecycle 变化；
- binding 变化；
- 非秘密策略变化；
- operator/RBAC 变化。

领域事务原子写入：

```text
domain state + AuditEvent + ChangeEvent/outbox
```

**例外：no-op Import Commit。** 当 `is_noop = true` 时，不写入 confirmed-content / binding 突变，不提升 contest configuration、credential 或 assignment revision，不写内容变化 ChangeEvent/outbox，也不触发 Target churn；仅原子写入 import lineage 与 redacted AuditEvent（及幂等/terminal 所需的非内容元数据）。

它不等待 Device，也不直接创建“已同步”结论。

Import Commit 是 domain transaction，须区分 **material** 与 **no-op**：

- **Material** Import Commit：在同一 Server transaction 中校验 baseline CAS 与 binding freshness；仅当 preview 已授权的待解绑 bound Seat 集合非空时执行 atomic unbind 并提升对应 `AssignmentRevision`，再替换 confirmed configuration；仅在实际 material/binding 变化时提升相关 revision 并写内容变化 ChangeEvent/outbox。
- **No-op** Import Commit：只记录 lineage 与 redacted AuditEvent；无 confirmed-content 突变、无 contest/credential/assignment revision bump、无内容变化 ChangeEvent/outbox、无 Target churn。

二者均不创建 Operation/Command，不自动执行 `SYNC_STATE` 或 `SYNC_SECRET`，也不产生 Device I/O。Material Import 改变 Server truth 后可能出现的 assignment / configuration / secret Drift 不代表 Device 已同步。baseline 或 binding freshness mismatch 须重新 preview，且不得改变 confirmed truth。完整 candidate、redacted preview、baseline CAS、`is_noop` 与 zero-Command 规则见 [领域模型](domain-model.md#42-contest-configuration-import)。

### 3.2 Target 计算

Target 由当前 Server truth 和冻结策略派生。实现可以：

- 在事务后异步重算；
- 按读取惰性计算；
- 持久化 materialized snapshot；
- 使用 hash/generation 做缓存。

无论实现方式如何，必须满足：

- 相同语义输入产生相同语义 Target；
- 每个语义变化对应新的 `ConfigurationGeneration`；
- 密码明文从不进入 Target；
- Target 变化不会自动联系 Device；
- 旧 Target 必须可识别为 superseded；
- Target 仅因实际 Server truth 变化而变为 stale/recomputable；**no-op** Import Commit 不突变 confirmed 内容、不提升 contest configuration / credential / assignment revision、不写内容变化 ChangeEvent/outbox，也不触发 Target churn。

## 4. Observed

Device 在以下时机发送 Observed：

- control connection 建立；
- Command 执行关键阶段；
- 本地状态变化；
- 周期性刷新；
- operator 请求 `OBSERVE_NOW`；
- 故障恢复后。

Observed 发送失败不回滚本地已经成功的原子动作。Device 在重连后重新发送完整 snapshot，使 Server 收敛。

Server 接收规则：

1. 验证 authenticated Device；
2. 验证 schema、大小、sequence 和 boot identity；
3. 原子保存 snapshot；
4. 更新 freshness；
5. 产生 ChangeEvent；
6. 异步重算 Drift/Operation 视图。

Server 不把 Observed 自报 hostname、证书 profile 或 binding 当作授权。

## 5. Drift

Drift 按维度计算，不能只返回布尔值。

建议结果：

```text
IN_SYNC
DRIFTED
UNKNOWN_NO_OBSERVATION
UNKNOWN_STALE_OBSERVATION
NOT_APPLICABLE
BLOCKED_BY_PREREQUISITE
```

其中 `NOT_APPLICABLE` 只用于真正没有该维度的状态，不能用来豁免 Gate 或隐藏失败。

典型顺序：

1. identity/certificate prerequisite；
2. assignment；
3. configuration；
4. secret revision；
5. Caddy activation；
6. session/home。

一个维度失败不应覆盖其他维度的可见状态。

## 6. 何时创建 Operation

创建 Operation：

- 批量向多台 Device 发起同步；
- 需要统一进度、取消或结果摘要；
- 一个 operator 动作会产生多个 Command；
- 需要跨时间展示业务意图。

不创建 Operation：

- 同步完成的普通 CRUD；
- 只读查询；
- 单纯 Target 重算；
- AuditEvent；
- ChangeEvent/outbox；
- 单个立即完成且不需要异步追踪的本地 Server 动作。

单 Device `SYNC_STATE` 可以直接创建 Command，也可以由 UI 为一致呈现创建一目标 Operation；二者都不能迫使所有 CRUD 使用 Operation。

## 7. Operation 归约

OperationTarget 对每台 Device 聚合 Command：

```text
PENDING
RUNNING
SUCCEEDED
FAILED
CANCELED
SKIPPED_STALE
```

Operation 总体归约：

| 子目标 | 总体 |
|---|---|
| 全部成功 | `SUCCEEDED` |
| 部分成功、部分失败 | `PARTIAL` |
| 全部失败 | `FAILED` |
| 仍有未终态 | `RUNNING` |
| 发起前全部取消 | `CANCELED` |

归约函数必须确定且可重算。人工 override 必须单独审计，不能直接改写历史 Command 结果。

## 8. Command 生命周期

```text
CREATED
  → QUEUED
  → DISPATCHING
  → RECEIVED
  → RUNNING
  → SUCCEEDED
  ↘ FAILED
  ↘ REJECTED_STALE
  ↘ REJECTED_CONFLICT
  ↘ EXPIRED
  ↘ CANCELED
```

允许实现合并内部中间状态，但必须保留：

- Server 是否已 durable；
- Device 是否已 durable receipt；
- 是否正在执行；
- terminal 结果；
- stable ErrorCode；
- 时间和 attempt 记录。

### 8.1 创建

创建 Command 时冻结：

- target Device；
- kind；
- 必要 revision/generation/epoch；
- typed payload 或 payload reference；
- expiry/retry policy；
- operator/audit correlation。

Command 创建与其业务 intent 记录原子提交。秘密 payload 应使用受限加密存储或短生命周期派生，不进入通用 Command JSON。

### 8.2 投递

Dispatcher：

1. 选择非终态 Command；
2. 校验 Device lifecycle 和 current connection；
3. 创建 Attempt；
4. 发送相同 `command_id`；
5. 记录 receipt/status；
6. 按 retry policy 重试。

重试不创建新 Command。

### 8.3 Device receipt

Device journal 在发送 receipt 前 durable：

- `command_id`；
- kind；
- payload hash；
- 状态；
- 必要 revision；
- terminal result（如已存在）。

进程崩溃后，相同 Command 能恢复或返回原结果。

### 8.4 终态

终态不可被后来的 transport error 覆盖。重复终态消息必须幂等合并。若 Server 丢失确认，重发相同 Command 后 Device 返回已存结果。

## 9. Attempt

Attempt 是观察，不是业务身份。包含：

- attempt number；
- connection identity；
- start/end；
- transport result；
- receipt latency；
- last status；
- redacted diagnostics。

Attempt 可以失败而 Command 仍非终态。网络瞬断不得把业务 Command 立即标记为失败，除非 expiry/retry policy 已结束。

## 10. 幂等和冲突

### 10.1 相同 `command_id`

- payload hash 相同：返回既有状态/结果；
- payload hash 不同：`REJECTED_CONFLICT`；
- 已成功：不得重复副作用；
- 正在运行：不得并发执行第二次；
- 已失败：按命令策略返回既有终态，不偷偷重启。

需要人工重新执行时创建新的 Command ID，并关联 previous command。

### 10.2 revision/generation/epoch

Device 在执行前和关键原子提交前检查：

- assignment revision；
- configuration generation；
- credential revision；
- session epoch；
- home epoch。

陈旧时使用稳定 stale 错误，不应用“尽量兼容”的部分状态。

## 11. `SYNC_STATE` 状态机

概念阶段：

```text
VALIDATING
→ STAGING_NON_SECRET
→ PREPARING_GATEWAY_KEY
→ REQUESTING_GATEWAY_CERT
→ VALIDATING_GATEWAY_CERT
→ STAGING_CADDY
→ VALIDATING_CADDY
→ ACTIVATING
→ VERIFYING
→ RECORDING_LKG
→ REPORTING
```

某些阶段可以跳过，例如现有 Gateway certificate 仍满足当前 Target。

### 11.1 原子边界

以下组合必须避免半激活：

- Gateway key/certificate；
- Caddy config；
- config hash/generation；
- LKG metadata；
- Observed activation status。

实现可以使用 staging directory、rename、数据库 transaction 和 Caddy atomic load，但任何中途失败都必须保留原 READY LKG 或进入 BLOCKED，不暴露未验证配置。

### 11.2 失败策略

| 失败 | 行为 |
|---|---|
| Target 陈旧 | 拒绝，不修改本地状态 |
| Gateway CSR conflict | 停止当前 Command，保留旧 LKG |
| 证书验证失败 | 不激活，保留旧 LKG或 BLOCKED |
| Caddy config validation 失败 | 不 load |
| Caddy load 失败 | 尝试确认旧配置仍有效；否则 BLOCKED |
| upstream 不健康 | 按冻结 policy 保持 READY-with-health 或 BLOCKED；不得自由猜测 |
| 进程崩溃 | 从 journal/staging 恢复，重复 Command 幂等 |
| Observed 上传失败 | 本地结果不回滚，重连重报 |

upstream health 的具体 READY policy 必须在环境冻结时明确。

## 12. Gateway certificate readiness

Device Identity 与 Gateway certificate 是两个独立维度：

```text
device_identity_ready
gateway_certificate_ready
```

`READY-DEVICE-ID`、`READY-GATEWAY-CERT` 只可作为文档叙述别名，不得添加为新的 wire、API、数据库或通用运行时状态字段。

Gateway certificate 需要：

- active `SYNC_STATE`；
- authenticated Device；
- current Target；
- current generation/revision；
- Server 派生 SAN/profile；
- 本地 private key 匹配；
- certificate validation；
- Caddy activation完成或明确等待。

Enrollment 成功不能被展示为 Gateway ready。

## 13. `SYNC_SECRET` 状态机

```text
VALIDATING_ASSIGNMENT
→ FETCHING_SERVER_SECRET
→ ENCRYPTING/SENDING
→ DEVICE_VALIDATING
→ WRITING_CLIENT_VAULT
→ VERIFYING_REVISION
→ REPORTING_REDACTED_RESULT
```

规则：

- 只能由人类明确触发；
- 不能由 Target drift 自动触发；
- Command 创建时冻结 assignment/credential revision；
- Device 写入前重新校验；
- vault 更新原子；
- 失败时保留旧 secret 或明确标记不可用，不留下半写；
- Caddy/Browser 只通过受限 adapter 使用 secret；
- 成功后 Observed 只报告 revision；
- retry 使用相同 Command ID，不重复不可逆动作。

## 14. Caddy 状态

Caddy 业务状态只需要：

```text
BLOCKED
READY
```

可以有内部过渡状态，但 UI/Observed 不应暴露大量实现特例。

### 14.1 BLOCKED

- 主页面返回 HTTP 503；
- 只显示 allowlist 状态；
- 静态本地资源；
- 严格 CSP；
- 动态值只通过 `textContent`；
- 不显示 password、路径、自由格式错误或 `session_locked`；
- 不代理 DOMjudge。

### 14.2 READY

进入 READY 需要证明：

- 当前 Target/revision；
- Gateway certificate 和 private key 匹配；
- SAN/profile/validity；
- Caddy config validation；
- fixed upstream policy；
- load 成功；
- 本地健康检查；
- LKG 写入成功或可恢复。

### 14.3 与 Session 解耦

Session lock/unlock/terminate：

- 不调用 Caddy Admin；
- 不改变 config hash；
- 不改变 configuration generation；
- 不改变 Caddy status；
- 不将 `session_locked` 放入状态页。

## 15. Session 状态

概念状态：

```text
NO_MANAGED_SESSION
STARTING
ACTIVE
LOCKING
LOCKED
UNLOCKING
TERMINATING
FAILED
```

每个 transition 绑定当前 `SessionEpoch`。Agent 通过 lease 证明自己属于当前 logind session。

### 15.1 Agent 展示状态

UI presentation 独立于业务 session state，可表达：

- `HIDDEN`
- `VISIBLE_FOCUSED`
- `VISIBLE_UNFOCUSED`
- `DISPLAY_UNAVAILABLE`
- `DISPLAY_LOST`

Wayland focus denial 是可观察结果，不通过桌面专用 hack 修改核心状态机。

### 15.2 Agent 崩溃

- lease 过期；
- Daemon 不把旧 Agent 当作当前会话代理；
- 根据冻结策略重新启动/替换受管 session；
- 不解锁额外权限；
- 不改变 Caddy。

## 16. Home transaction

Home backend 在部署时固定为 OverlayFS 或 staged-copy。

事务规则：

1. 开始时创建新的 `HomeEpoch`；
2. 所有步骤记录 durable progress；
3. path/UID 在本地 policy 派生；
4. Helper 只执行封闭动作；
5. prepare 完成前不启动受管 session；
6. cleanup 只作用于当前 epoch；
7. 失败不静默切换 backend；
8. crash recovery 根据 journal 判断继续、回滚或人工处理；
9. 无法证明 mount/copy/ownership 安全时 fail closed。

## 17. 取消和过期

- 尚未被 Device receipt 的 Command 可以安全取消；
- 已 receipt 的 Command 取消是请求，不保证中断原子阶段；
- 已进入 certificate/Caddy/Home 原子提交的 Command 必须先完成到安全点；
- 过期不撤销已经成功完成的本地结果；
- 取消/过期之后仍接收到 terminal success 时，Server 保存真实结果并在 Operation 视图标记 race；
- 不通过删除 Command 隐藏历史。

## 18. 可观测性

### Server 指标

- active Device connections；
- Command queue/latency/retry；
- Operation result；
- Observed freshness；
- Drift count；
- enrollment/certificate outcomes；
- stable ErrorCode count；
- audit/outbox backlog。

### Device 指标

- connection/reconnect；
- journal recovery；
- Command duration；
- vault/identity result 的有限码；
- Caddy activation；
- session/home transition；
- Agent lease；
- Observed upload。

指标 label 不得包含 Seat 原始密码、账号密码、路径、certificate body、Machine ID 全值或自由格式错误。

## 19. 测试模型

必须覆盖：

- Server 事务成功但 Device 离线；
- receipt 前/后断线；
- 执行中崩溃；
- 重复相同 Command；
- 相同 ID 不同 payload；
- 陈旧 generation/revision/epoch；
- Gateway request 幂等与 SPKI conflict；
- Caddy validate/load 中断；
- old LKG 保留；
- secret 写入中断；
- Observed 丢失和重发；
- Agent crash/focus denied/display lost；
- Home prepare/cleanup 中断；
- Operation 部分成功归约；
- cancel 与 terminal status race。
