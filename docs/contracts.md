# Natsume V2 边界契约

> 状态：`NORMATIVE`  
> 适用范围：HTTP、Enrollment、mTLS QUIC、Protobuf、D-Bus、CommandStatus 和公开错误  
> 机器结构权威源：OpenAPI、Protobuf descriptor、D-Bus introspection、SQL migration 和生成代码

本文件定义稳定语义，不复制完整 schema。字段名、编号和路由必须从机器 schema 生成或验证。

## 1. 契约原则

1. 每个边界使用封闭 typed contract。
2. 输入必须先完成认证、大小限制、版本和结构校验，再进入 application。
3. 网络输入不得直接成为任意命令、路径、UID、unit、环境、upstream 或配置片段。
4. 公开边界返回稳定 `ErrorCode`；领域内部保留 typed error。
5. 密码、private key，以及由 password-bearing CSV 内容派生的 digest/fingerprint/length，不进入通用 API、Observed、日志、指标、SSE、outbox 或普通 audit。
6. 兼容性只在明确的 wire/schema version 范围内提供。
7. 未知 enum/oneof、超限 frame、重复非法字段或版本不匹配必须显式失败。
8. 人类叙述状态不能偷偷成为 wire 字段。

## 2. 契约所有权

| 边界 | 机器权威源 | 语义 Owner |
|---|---|---|
| Operator HTTP | Rust schema → OpenAPI | `operator-api` |
| Web 类型 | OpenAPI 生成 TypeScript | Web adapter |
| Enrollment | OpenAPI/typed Rust request | `identity-enrollment` |
| Device control | `.proto` + descriptor golden | `device-control` |
| Local Device/Agent | D-Bus XML + Rust value types | `local-control-api` |
| Helper capability | D-Bus XML + policy | privileged boundary |
| Stable errors | `natsume-error-code` registry | public-boundary policy |
| SQL | migration | owning domain module |

不得手工维护与生成契约并行的“第二份完整字段表”。

## 3. Operator HTTP

### 3.1 基本要求

- 使用 HTTPS；
- operator session 和 RBAC 在 Server 边界执行；
- mutation 必须有 request/correlation ID；
- mutation 返回“领域已提交”或“异步 Operation 已创建”，不虚构远端完成；
- list/query 支持稳定分页和排序；
- 时间使用明确时区/UTC wire 表示；
- 资源 ID 与显示名称分离；
- destructive / high-impact mutation 要求明确确认语义；其中 contest configuration 的 **Import Commit** 本身即二次确认动作，不新增独立 confirmation resource（见 §3.4）；
- 幂等需要时使用稳定 idempotency key，而不是浏览器重试猜测。

### 3.2 Problem Details

HTTP 错误使用 Problem Details 或等价结构，至少包含：

```text
type
title
status
code
correlation_id
detail?          # 仅允许脱敏、对人类有用的描述
field_errors?    # 仅结构化校验错误
```

`code` 来自稳定 ErrorCode registry。调用方不得解析 `title` 或 `detail` 判断业务。

### 3.3 Secret API 约束

允许：

- 上传 CSV 到受限 staging；
- 展示 password **是否**变化（布尔/分类级 redacted 证据，例如 `PASSWORD_CHANGED`）；
- 人工触发 `SYNC_SECRET`；
- 展示 credential revision 和 redacted result；
- 展示 Server 签发的 opaque `preview_token`、baseline `ContestConfigurationRevision`、redacted import summary 与 binding impacts（见 §3.4）。

禁止：

- 返回 password 值；
- 返回 password length、password fingerprint，或任何由 password-bearing CSV 内容派生的 hash/digest；
- 返回 raw CSV 内容 hash，或其他可形成离线猜测 oracle 的 candidate 内容证据；
- 提供通用 secret read endpoint；
- 把 password 或 password-derived material 放入 JSON audit diff；
- 在 SSE、outbox、metric 或普通 log 中发送 secret / password-derived payload；
- 在错误 `detail` 中包含 CSV 行原文、密码或其派生指纹/长度/digest；
- 将浏览器 local/session storage 作为秘密存储；
- 要求 Browser 重新计算 hash 或 diff 作为 commit 绑定条件。

Server 内部 candidate digest/revision 仅可存在于 encrypted staging / secret-safe persistence，不得出现在 ordinary response、error、audit、log、metric、SSE 或 outbox。

### 3.4 Operator import lifecycle（语义）

本小节只定义 Operator HTTP 边界上的稳定语义。领域 taxonomy、transaction 顺序与 revision 规则以 [领域模型](domain-model.md) 为准；完整 OpenAPI/Rust machine contract 的实现与生成物更新属于后续任务，本次文档不修改生成契约。

Import 是对 **confirmed contest configuration** 的 high-impact 路径；是否发生内容突变取决于 commit 分支。统一流程（首次与后续相同）为：

```text
upload
  -> encrypted staging
  -> strict parse
  -> immutable candidate import
  -> server-computed redacted diff
  -> explicit Import Commit 或 Import Discard
```

Import Commit 分支（幂等预检之后）：

```text
idempotency preflight (step 0)
  -> [material] live validation + baseline CAS
       + 必要的 atomic unbind-and-replace
       + 内容替换 / revision bumps / content outbox
  -> [no-op] live validation
       + lineage/redacted audit only
       （before_revision == after_revision；无 unbind-and-replace / 内容 outbox / Target churn）
```

#### 3.4.1 Preview 证据

成功 preview（或等价 read-model）至少向授权 Operator 返回：

- `import_id`：candidate identity；
- baseline `ContestConfigurationRevision`（空 confirmed configuration 时为 `0`）；
- opaque `preview_token`：Server 签发或持久化的不透明句柄，指向签发时冻结的 preview evidence；普通 surface 不得改用 password-derived digest/fingerprint/length 作为绑定材料；
- expiry（candidate 与/或 preview token 的有效期语义）；
- redacted summary：分类计数、`is_noop`、`commit_allowed`、`blocking_reasons`；该 summary 与 items 在签发后不可变；
- binding impacts：精确集合，元素至少为 `(SeatCode, DevicePk 或允许展示的非秘密 Device identity, AssignmentRevision, UNBIND_ON_COMMIT)`；count 为 0 时仍须显式给出空集合；
- 不返回完整 diff taxonomy 字段表以外的秘密材料；taxonomy 与 `is_noop` 权威见 [领域模型 §4.2–4.3](domain-model.md)。

Preview evidence 在签发时刻不可变地绑定：candidate identity、baseline `ContestConfigurationRevision`、完整 redacted diff、精确 binding impact 集合、preview actor/authorization context、expiry。Stored evidence / summary / items 签发后不得原地突变；若结果需更新，必须重新 preview 并签发新 token。

Server 是 diff classification 的唯一权威；client/UI 只渲染结构化结果，不得本地重算分类或要求 Browser 持有 password-derived 绑定材料。

空/仅 header candidate，或 candidate 内重复 `account`，必须在 preview 结果中体现为 `INVALID` 且 `commit_allowed = false`（稳定错误类别见 §3.4.4），不得进入可提交状态。

#### 3.4.2 Import Commit 作为二次确认

**Import Commit** 是显式二次确认动作，不是额外 confirmation resource。成功结果必须区分 **material** 与 **no-op**。

Commit 请求语义输入：

| 输入 | 含义 |
|---|---|
| `import_id` | 要提交的 candidate identity |
| `preview_token` | 与 preview 绑定的 opaque Server 证据 |
| `baseline_revision` | Operator 审阅时所依据的 confirmed configuration revision（CAS token） |
| `idempotency_key` | 稳定幂等键；相同 key 的重试不得重复副作用 |
| `correlation_id` | 请求关联 ID，贯穿 audit/response |

Actor（操作员身份与授权上下文）**不得**由 client 自报为可信任主体；必须从 Server auth / session / RBAC context 获取，并在 commit 的 live validation 路径内重验。

**幂等预检（step 0）**：在 authorization 重验、preview evidence 相等校验、baseline CAS、binding impact 重算/比较及任何 mutation **之前**执行：

| 已有记录 | 判定 |
|---|---|
| 已 `COMMITTED` 且 key + 语义 body 相同 | 返回已存储业务结果；**零副作用**（不重跑 live 校验与 mutation） |
| 已 `COMMITTED` 但 key/body 不匹配 | `idempotency conflict` |
| 非终态同 key 但 body 不同 | `idempotency conflict` |
| 无命中 / 首次执行 | 进入 live validation 与 mutation |

**成功后重试**：material 成功可能导致 revision bump 与 unbind；之后相同 key + 相同语义 body 的重试仍必须在 step 0 返回存储成功结果，不得因当前 live baseline/binding 已变而误报 stale baseline 或 binding-stale。

仅首次/非重放路径的 live Commit 校验：

- 重验 authorization、candidate state（非 `DISCARDED`/不可 commit 终态）、preview token→不可变 evidence 逐字段相等、expiry、baseline CAS；
- 对全部 `REMOVED` Seat **重算** binding impacts 并与 evidence 精确集合相等比较；
- Commit **仅可**解绑 preview evidence 已授权的精确 binding impact 集合；不得解绑集合外 Device；
- 始终 `AUTO_COMMAND_COUNT = 0`：不创建 Operation/Command，不自动 `SYNC_STATE` / `SYNC_SECRET`，不产生 Device I/O；
- 不得虚构远端 Device 已同步。

**Material 成功响应**（`is_noop = false` 且内容实际变化）：

- 领域已提交且 Server truth 已按 candidate 替换（含必要的 unbind-and-replace）；
- redacted 结果至少包括 `before_revision`、`after_revision`（`after_revision > before_revision` 当 contest 内容变化时）、impact counts、`is_noop = false`；
- 可伴随内容变化 ChangeEvent/outbox；可能引发 Target 重算/churn；
- 不表示 Device 已同步。

**No-op 成功响应**（`is_noop = true`，定义见领域模型：无 ADDED/REMOVED/account/password 变更、无 INVALID、无 binding impact、全部有效项 UNCHANGED）：

- 仍为显式 Import Commit 成功，candidate 进入 terminal `COMMITTED`；
- **仅** lineage / redacted audit（及幂等/terminal 元数据）；
- `before_revision == after_revision`；**不**提升 contest configuration、credential 或 assignment revision；
- **不**执行 unbind-and-replace；
- **不**写内容变化 ChangeEvent/outbox；
- **不**制造 Target churn；
- 响应必须可区分 no-op 与 material（至少 `is_noop` 与 before/after revision 相等），不得把 no-op 表述为「Server truth 已变更」或暗示发生了 unbind/outbox。

#### 3.4.3 Import Discard

**Import Discard** 是公开边界上的显式动作（与 runbook 自愿中止对应），不是仅出现在错误表中的附带状态。

| 输入 | 含义 |
|---|---|
| `import_id` | 要放弃的 candidate identity |

- Actor 从 Server auth / session / RBAC context 获取并校验授权；不得信任 client 自报身份。
- 成功效果：candidate → 终端 `DISCARDED`；该 import 上的 `preview_token`/evidence 对后续 commit **失效**；confirmed contest configuration、binding、相关 revision 与 Target **均不变**；无 Device I/O、无内容变化 outbox；encrypted staging 清理遵循安全策略。
- 对已 `DISCARDED` 的同一 `import_id` 再次调用必须**幂等**（无额外副作用）。
- 对已 `COMMITTED` 的 import 调用 Discard 必须拒绝，且**不得**撤销或回滚已提交的 confirmed truth。
- 失败（未授权、未知 `import_id`、非法状态等）均不得改变 confirmed truth。

#### 3.4.4 稳定错误类别

公开边界使用稳定 `ErrorCode` 映射以下语义类别（具体码值以 `natsume-error-code` registry 为准；调用方不得解析 `title`/`detail` 判业务）：

| 类别 | 语义 |
|---|---|
| invalid candidate | candidate 存在 `INVALID` 行或其他使 `commit_allowed = false` 的结构性/业务阻断，**包括**重复 `account`、空/仅 header candidate；必须修复文件并创建新 candidate；不得通过 import 清空配置 |
| expired / discarded | candidate 或 `preview_token` 已过期、已 discard，或不再处于可 commit 状态 |
| stale baseline | 请求的 `baseline_revision` 与当前 confirmed `ContestConfigurationRevision` 不一致；必须重新 preview |
| binding stale | commit 时对 `REMOVED` Seat 重算的当前 binding impact 集合与 preview evidence 中的精确集合不相等（新增/缺失/identity 或 `AssignmentRevision` 变更等）；必须重新 preview |
| preview mismatch | `preview_token` 无法对应签发时冻结的 evidence，或与 `import_id`/baseline/完整 redacted diff/binding impacts/actor 绑定的相等校验失败 |
| idempotency conflict | 相同 `idempotency_key` 绑定了不同 commit 语义输入 |
| authorization failure | actor 未认证、无权限，或当前 authorization 与 evidence 绑定 context 重验失败 |
| transaction failure | Server 事务失败并完整回滚；confirmed truth 不变；不含历史配置产品级 rollback |

上述失败、过期、discard、stale baseline、binding stale、preview mismatch、authorization failure 与 transaction failure **均不得**改变 confirmed contest configuration、binding、Target truth 或相关 revision。幂等重试在 step 0 命中已成功（同 key/同 body）时返回同一业务结果，不重复副作用，且不因 post-success revision bump/unbind 而改走 live stale 路径。

#### 3.4.5 非目标（本契约边界）

- 不新增独立 confirmation resource 或额外“confirm token”产品对象；
- 不以 secret-derived public content hash 作为 Browser 可见绑定；普通绑定仅使用 opaque `preview_token`；
- 不在 ordinary surface 暴露 password 值、length、fingerprint 或 password-derived digest；
- 不把 Import 表述为 Device Target/Observed snapshot 操作；
- 不允许将 preview evidence/summary/items 视为可原地更新的可变句柄；
- 不通过 CSV import 提供清空 confirmed configuration 的路径（清空仅 single-lifetime reset）；
- 不把 no-op Import Commit 表述为 unbind-and-replace 或 Server 内容 truth 变更；
- 不把 Discard 实现为可撤销已 COMMITTED import 的 rollback。

## 4. Enrollment

### 4.1 传输

Enrollment 使用 server-auth HTTPS：

- Client 必须验证预配置的 Server trust 和 IP-SAN/endpoint；
- 不使用 TOFU 或 dangerous verifier；
- 请求有严格大小和速率限制；
- 失败不会自动切换到匿名 QUIC；
- Enrollment 与 operator API 可以共享 Server 进程，但必须使用独立路由、授权和限流策略。

### 4.2 请求语义

Enrollment request 只包含建立 Device Identity 所需的 typed material，例如：

- wire/schema version；
- Machine Hardware ID；
- Device Identity CSR；
- 有限设备元数据；
- client nonce/请求 ID；
- 协议能力。

不得包含：

- Gateway CSR；
- Gateway SPKI；
- Gateway SAN；
- Caddy config；
- DOMjudge password；
- 通用 certificate profile；
- 任意路径、unit 或 shell；
- “请签发任何证书”的通用请求。

### 4.3 响应语义

成功响应只返回：

- Device Identity leaf；
- 必要 chain；
- Server 分配的 Device 标识；
- control endpoint 的受限配置；
- 协议版本/能力；
- 必要的审计 correlation。

不得返回 Gateway certificate。

### 4.4 原子性

Client 只有在验证以下条件后才提交 Enrollment 结果：

- leaf 与本地 Device private key 匹配；
- chain/trust 合法；
- SAN/profile/usage 符合 Device identity policy；
- Server 返回的 Device identity 一致；
- 本地持久化可原子完成。

中途失败不得留下“看似已 Enrollment”的半状态。

## 5. Device control：mTLS QUIC

### 5.1 TLS

- mandatory mutual TLS；
- Client 只使用 Device Identity certificate；
- Server 验证 chain、profile、有效期、撤销/生命周期和 Device mapping；
- 匿名 peer 在 TLS handshake 阶段拒绝；
- 0-RTT 禁用；
- Enrollment trust config 与 QUIC mTLS config 分离；
- 不使用降级为 server-auth-only QUIC 的 fallback。

### 5.2 连接模型

每台 Device 建立受控 QUIC connection。V2 基线使用一条长期双向 control stream；不得把它扩展为通用 RPC 或任意 stream platform。

连接建立后至少完成：

1. exact wire version/能力协商；
2. authenticated Device identity 绑定；
3. boot/connection identity；
4. initial status 或 hello；
5. heartbeats/freshness；
6. Command/Observed 交换。

连接中断不改变 Server truth。重连后通过 durable Command 和 Observed 收敛。

### 5.3 Framing

Frame 必须有：

- 明确最大长度；
- 长度前缀或等价边界；
- exact protocol version；
- 封闭 envelope kind；
- correlation/command ID；
- 解码失败稳定错误和计数；
- 不把未认证 bytes 送入 Protobuf parser。

超限 frame、未知版本、非法 oneof 或重复冲突内容必须关闭相应 stream/connection，不得猜测。

## 6. Control envelope

概念 envelope：

```text
ControlEnvelope {
  wire_version
  message_id
  correlation_id?
  command_id?
  payload: oneof {
    hello
    heartbeat
    command
    command_receipt
    command_status
    observed_snapshot
    gateway_certificate_request
    gateway_certificate_result
    protocol_error
  }
}
```

实际字段和编号由 `.proto` 决定。以下语义稳定：

- `command_id` 只标识一个 durable Command；
- Attempt 不生成新的 `command_id`；
- receipt 与 terminal status 分离；
- Observed snapshot 不嵌入 password；
- ProtocolError 不包含内部 source chain；
- Gateway request/result 只用于 active `SYNC_STATE`。

## 7. Command 契约

### 7.1 通用 envelope

Command 至少绑定：

- `command_id`；
- kind；
- target Device；
- issued-at/expiry policy；
- assignment revision、configuration generation 或 epoch 中的必要集合；
- typed payload version；
- redacted audit correlation。

禁止通用：

- `EXEC`
- `RUN_SHELL`
- `WRITE_FILE`
- `SYSTEMD_UNIT`
- `INSTALL_CERTIFICATE`
- `APPLY_CADDY_FRAGMENT`
- `SET_ENV`
- 任意 URL/upstream/path/UID。

### 7.2 Command family

V2 允许的业务 family：

| Kind | 目的 | 关键绑定 |
|---|---|---|
| `SYNC_STATE` | 应用非秘密 Target | assignment revision + configuration generation |
| `SYNC_SECRET` | 安装当前密码 | assignment revision + credential revision |
| `SESSION_LOCK` | 锁定当前受管会话 | session epoch |
| `SESSION_UNLOCK` | 解锁当前受管会话 | session epoch |
| `SESSION_TERMINATE` | 结束当前受管会话 | session epoch |
| `HOME_PREPARE` | 准备 Home | assignment revision + home epoch |
| `HOME_CLEAN` | 清理 Home | home epoch |
| `OBSERVE_NOW` | 请求新的 Observed snapshot | current connection identity |
| `DEVICE_RETIRE` | 进入受控退役 | lifecycle revision |

具体枚举以当前 `.proto` 为准。新增 family 必须证明不是任意远程管理能力。

### 7.3 Receipt

Device 只有在以下内容 durable 后才发送 receipt：

- `command_id`；
- kind/payload hash；
- 当前生命周期状态；
- 幂等 lookup 所需信息；
- 初始状态。

receipt 表示“已经可靠接收”，不表示“已成功执行”。

### 7.4 Status

CommandStatus 至少区分：

```text
PENDING
RECEIVED
RUNNING
SUCCEEDED
FAILED
REJECTED_STALE
REJECTED_CONFLICT
CANCELED
EXPIRED
```

终态必须携带稳定 ErrorCode 或明确 success result。不得把自由格式日志当作状态。

## 8. `SYNC_STATE`

Payload 只携带非秘密、封闭的 Target snapshot 或其 typed plan。

Device 必须验证：

- target Device；
- assignment revision；
- configuration generation；
- command freshness；
- 本地 identity；
- payload schema/version；
- 所有派生 hostname/upstream/profile 均来自允许集合；
- staging 可原子完成。

应用应遵循：

```text
validate
→ persist command
→ stage
→ request Gateway certificate when required
→ validate returned certificate
→ stage Caddy config
→ validate config
→ atomic activation
→ record LKG
→ emit Observed
```

不要求每次都签发新 Gateway certificate；是否需要由 Target 和现有有效材料决定。

## 9. Gateway certificate 子协议

### 9.1 上下文

Gateway CSR 只允许在以下全部条件成立时提交：

- QUIC peer 已通过 Device mTLS；
- 存在 active `SYNC_STATE` Command；
- request 绑定该 `command_id`；
- target Device、assignment revision 和 configuration generation 一致；
- Server 当前 Target 仍允许该 hostname/profile；
- request 未过期。

Enrollment、匿名 HTTPS、普通 operator API 和通用 certificate endpoint 都不能签发 Gateway certificate。

### 9.2 请求

概念字段：

```text
command_id
configuration_generation
assignment_revision
request_id
csr_der
spki_fingerprint
```

CSR 中的 SAN、CN 或自报 profile 不授予权限。

### 9.3 签发

Server 从当前 Target/policy 派生：

- SAN；
- hostname；
- EKU/profile；
- validity；
- chain；
- serial；
- 可审计的 policy revision。

Server 验证 CSR 只用于证明 possession 和公钥结构，不信任 CSR 的授权属性。

### 9.4 幂等

同一：

```text
Device + command_id + generation + request_id + SPKI
```

必须返回同一签发结果或可识别的既有结果。

同一 request 但不同 SPKI 必须返回 conflict。错误 generation、无 active command、错误 Device 或匿名连接必须拒绝。

## 10. `SYNC_SECRET`

Payload 在传输和内存中使用秘密专用类型，至少绑定：

- `command_id`；
- Device；
- Seat；
- assignment revision；
- account；
- credential revision；
- secret ciphertext/受保护 payload；
- expiry。

Device 必须在写入 vault 前重新验证当前 binding 和 revision。陈旧 secret 不得安装。

成功结果只报告：

- 已安装 credential revision；
- redacted status；
- audit correlation。

不得回显 password、hash 派生材料或完整 ciphertext。

## 11. Observed snapshot

Observed 使用完整或可证明合并的 typed snapshot，不使用自由格式 status map。

建议维度：

```text
identity
device_certificate
gateway_certificate
binding
configuration
credential
caddy
upstream
session
home
lkg
last_error
boot_id
snapshot_sequence
```

每个维度独立表达状态、revision/generation、有限诊断码。密码值、private key、完整路径和内部异常链不得出现。

Server 只接受：

- 当前 authenticated Device 的 snapshot；
- 单调或可解释 sequence；
- 合法大小和 schema；
- 不能把 Device 自报字段直接当作授权。

## 12. Local D-Bus：Device Daemon ↔ Session Agent

### 12.1 Daemon 提供

- 获取当前 typed UI snapshot；
- 订阅/轮询 snapshot revision；
- 提交 Seat/binding action；
- 请求 lock/unlock/terminate；
- Agent lease/heartbeat；
- 获取当前 session eligibility 的有限结果。

### 12.2 Snapshot

UI snapshot 只包含展示所需数据，例如：

- view kind；
- title/message key 或已审计文本；
- machine short ID；
- Seat code；
- binding 状态；
- command in-progress；
- presentation/focus 状态；
- session epoch；
- allowed actions。

不得包含 password、vault path、certificate private material、Server token 或任意 HTML。

### 12.3 调用约束

- system/session bus policy 限制调用方；
- 校验 UID、PID/logind session 和 current epoch；
- action 是封闭 enum；
- 重放陈旧 epoch 被拒绝；
- Agent 退出导致 lease 过期，不授予额外权限；
- lock/unlock 不调用 Caddy adapter。

## 13. Local D-Bus：Device Daemon ↔ Privileged Helper

Helper 方法必须按 capability 命名，例如：

- 收集某类允许的硬件 source；
- 为固定 contest user 执行已验证的 Home transition；
- 对固定目录执行固定 owner/mode 操作；
- 调用允许的登录管理动作。

参数必须是：

- 封闭 enum；
- 经过规范化的 ID；
- 在 Helper 内重新派生或 allowlist 校验的路径/UID；
- 明确 epoch；
- 无 secret。

Helper 不接受 Server/QUIC request 的原始对象。

## 14. Caddy Admin contract

Device Daemon 不发送任意 Caddyfile。它从已验证 Target 和本地 certificate material 构造内部 activation plan。

Activation plan 只允许：

- 固定 loopback listen；
- 固定 origin/hostname；
- 固定 DOMjudge upstream；
- 固定 TLS material reference；
- 固定 BLOCKED/READY route 集；
- config hash；
- configuration generation。

执行前：

1. 生成完整候选配置；
2. 静态验证；
3. 验证证书/私钥匹配；
4. 验证 SAN/profile/有效期；
5. 原子加载；
6. 健康检查；
7. 成功后更新 LKG 和 Observed。

Session lock/unlock contract 不包含任何 Caddy 字段。

## 15. Stable ErrorCode

### 15.1 依赖方向

```text
DomainError
  → exhaustive adapter mapping
  → stable ErrorCode
  → HTTP / Protobuf / D-Bus / CommandStatus
```

禁止：

```text
stable ErrorCode → domain decision
```

### 15.2 规则

- 字符串值显式定义，不依赖 Rust variant debug 名；
- 每个公开 adapter 映射穷举；
- 未分类内部错误映射到有限通用码并记录内部 correlation；
- `detail` 默认无或脱敏；
- 新内部错误不自动成为新稳定码；
- 删除稳定码需要兼容计划；
- Web/Device 不解析 Display 文本；
- 同一语义跨 transport 使用同一稳定码。

### 15.3 分类

registry 应覆盖但不限于：

- validation；
- authentication/authorization；
- identity；
- certificate；
- protocol/version/framing；
- stale/conflict；
- vault/secret；
- state application；
- Caddy/data plane；
- session/home；
- unavailable/internal。

实际码值以 `natsume-error-code` crate 为准。

## 16. 版本和兼容

### 16.1 HTTP/OpenAPI

- 破坏性路由或 schema 变化需要版本策略；
- 生成 TypeScript 必须 clean diff；
- 删除字段前先停止生产和消费；
- secret 字段不能以“deprecated”形式长期保留。

### 16.2 Protobuf

- 已发布 field number 不复用；
- 删除字段使用 `reserved`；
- enum 的未知值处理显式；
- oneof 新分支按 wire compatibility 评估；
- exact wire version 在 connection 级拒绝不支持组合；
- descriptor golden 受 CI 保护。

### 16.3 D-Bus

- interface name、method、signal 和 error name 稳定；
- 破坏性变化使用新 interface version；
- XML 与 Rust types clean diff；
- policy 与方法能力同步测试。

### 16.4 持久化

- migration 只前进；
- downgrade/rollback 通过发布 runbook 定义，不假设 schema 自动回滚；
- ID 和 revision 语义不得被数据迁移重写；
- vault format 变化必须有版本、原子迁移和恢复测试。

## 17. 契约验证

CI 至少包含：

- OpenAPI 生成与 TypeScript clean diff；
- Protobuf descriptor golden；
- frame size/version/unknown enum/oneof 测试；
- Enrollment 无 Gateway 字段的 schema/DB/runtime 测试；
- D-Bus XML/Rust/policy 一致性；
- SQL migration 从空库和升级路径；
- ErrorCode 映射穷举；
- secret/path/source-chain redaction；
- 匿名 QUIC 未进入 decoder；
- Session lock contract 无 Caddy 字段；
- 禁止通用 `CertificateIssueRequest`、`INSTALL_CERTIFICATE` 和任意执行能力。
