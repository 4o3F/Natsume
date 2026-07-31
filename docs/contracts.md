# Natsume V2 边界契约

> 状态：`NORMATIVE`  
> 适用范围：HTTP、Enrollment、mTLS QUIC、Protobuf、D-Bus、CommandStatus 和公开错误  
> 机器结构权威源：OpenAPI、Protobuf descriptor、D-Bus introspection、SQL migration 和生成代码

本文件定义稳定语义，不复制完整 schema。字段名、编号和路由必须从机器 schema 生成或验证。未实现行为的完整字段表、wire 结构与事务编排延迟到对应 Phase 实现时由机器 schema 定义。

## 1. 契约原则

1. 每个边界使用封闭 typed contract。
2. 输入必须先完成认证、大小限制、版本和结构校验，再进入 application。
3. **网络输入不得直接成为任意命令、路径、UID、unit、环境、upstream 或配置片段。**
4. 公开边界返回稳定 `ErrorCode`；领域内部保留 typed error。
5. **密码、private key，以及由 password-bearing CSV 内容派生的 digest/fingerprint/length，不进入通用 API、Observed、日志、指标、SSE、outbox 或普通 audit。**
6. 兼容性只在明确的 wire/schema version 范围内提供。
7. 未知 enum/oneof、超限 frame、重复非法字段或版本不匹配必须显式失败。
8. 人类叙述状态不能偷偷成为 wire 字段。

## 2. 契约所有权

每个边界的机器权威源（OpenAPI、Protobuf descriptor、D-Bus introspection、SQL migration）是其结构的唯一来源；语义由对应模块拥有。**不得手工维护与生成契约并行的“第二份完整字段表”。**

## 3. Operator HTTP

### 3.1 基本要求

- 使用 HTTPS；operator session 和 RBAC 在 Server 边界执行；
- mutation 必须有 request/correlation ID，返回“领域已提交”或“异步 Operation 已创建”，**不虚构远端完成**；
- destructive / high-impact mutation 要求明确确认语义；contest configuration 的 **Import Commit** 本身即二次确认动作，不新增独立 confirmation resource；
- 幂等使用稳定 idempotency key，而不是浏览器重试猜测。

### 3.2 Problem Details

HTTP 错误使用 Problem Details 或等价结构（`type`/`title`/`status`/`code`/`correlation_id`/`detail?`/`field_errors?`）。`code` 来自稳定 ErrorCode registry；**调用方不得解析 `title` 或 `detail` 判断业务。** `detail` 仅允许脱敏、对人类有用的描述。

### 3.3 Secret API 约束

允许：上传 CSV 到受限 staging；展示 password **是否**变化（布尔/分类级 redacted 证据）；人工触发 `SYNC_SECRET`；展示 credential revision 和 redacted result；展示 Server 签发的 opaque `preview_token`、baseline revision、redacted import summary 与 binding impacts。

**禁止**：

- 返回 password 值、password length、password fingerprint 或任何 password-derived hash/digest；
- 返回 raw CSV 内容 hash，或其他可形成离线猜测 oracle 的 candidate 内容证据；
- 提供通用 secret read endpoint；
- 把 password 或 password-derived material 放入 JSON audit diff、SSE、outbox、metric 或普通 log；
- 在错误 `detail` 中包含 CSV 行原文、密码或其派生指纹/长度/digest；
- 将浏览器 local/session storage 作为秘密存储；
- 要求 Browser 重新计算 hash 或 diff 作为 commit 绑定条件。

Server 内部 candidate digest/revision 仅可存在于 encrypted staging / secret-safe persistence。

### 3.4 Operator import 边界

Import 是对 confirmed contest configuration 的高影响路径。Operator HTTP 边界上的稳定语义（领域 taxonomy、transaction 顺序与 revision 规则以 [领域模型](domain-model.md) 为准）：

- **Server 是 diff classification 的唯一权威**；client/UI 只渲染结构化结果，不得本地重算分类；
- 普通 surface 只使用 opaque `preview_token`，**不要求 Browser 持有 password-derived 绑定材料**；
- Import Commit 不创建 Operation/Command，不自动 `SYNC_STATE`/`SYNC_SECRET`，**不产生 Device I/O**，也不表示 Device 已同步；
- 任何 invalid、stale baseline、binding-stale、expiry、preview mismatch、discard、authorization failure 或 transaction failure **均不得改变 confirmed truth、binding 或相关 revision**；
- 清空 confirmed configuration 只能通过独立 single-lifetime reset，不得由 import 隐式完成；
- 不新增独立 confirmation resource，不以 secret-derived hash 作为 Browser 可见绑定。

完整 preview evidence 字段、idempotency step 0、material/no-op 响应形状与稳定错误类别在 Phase 2 实现时定义。

## 4. Enrollment

Enrollment 使用 server-auth HTTPS：Client 必须验证预配置 Server trust 和 IP-SAN/endpoint；**不使用 TOFU 或 dangerous verifier**；请求有严格大小和速率限制；失败不自动切换到匿名 QUIC；Enrollment 与 operator API 共享 Server 进程时必须使用独立路由、授权和限流。

Enrollment request **不得包含** Gateway CSR/SPKI/SAN、Caddy config、DOMjudge password、通用 certificate profile 或任意路径/unit/shell；**不得返回 Gateway certificate**。Client 只有在 leaf 与本地 private key 匹配、chain/trust 合法、SAN/profile/usage 符合 policy 且本地持久化可原子完成时才提交结果；**中途失败不得留下“看似已 Enrollment”的半状态**。

## 5. Device control：mTLS QUIC

- **mandatory mutual TLS**；Client 只使用 Device Identity certificate；Server 验证 chain、profile、有效期、撤销/生命周期和 Device mapping；
- **匿名 peer 在 TLS handshake 阶段拒绝**；**0-RTT 禁用**；Enrollment trust config 与 QUIC mTLS config 分离；**不降级为 server-auth-only QUIC fallback**；
- Frame 必须有明确最大长度、长度前缀、exact protocol version、封闭 envelope kind、correlation/command ID 和解码失败稳定错误；**不把未认证 bytes 送入 Protobuf parser**；
- 超限 frame、未知版本、非法 oneof 或重复冲突内容必须关闭相应 stream/connection，**不得猜测**；
- 连接中断不改变 Server truth；重连后通过 durable Command 和 Observed 收敛。

## 6. Command 契约

Command 绑定 `command_id`、kind、target Device、issued-at/expiry policy、必要 revision/generation/epoch、typed payload version 与 redacted audit correlation。

**禁止通用能力**：`EXEC`、`RUN_SHELL`、`WRITE_FILE`、`SYSTEMD_UNIT`、`INSTALL_CERTIFICATE`、`APPLY_CADDY_FRAGMENT`、`SET_ENV`，以及任意 URL/upstream/path/UID。

V2 业务 family 限于：`SYNC_STATE`、`SYNC_SECRET`、`SESSION_LOCK`/`UNLOCK`/`TERMINATE`、`HOME_PREPARE`/`CLEAN`、`OBSERVE_NOW`、`DEVICE_RETIRE`（具体枚举以 `.proto` 为准；新增 family 必须证明不是任意远程管理能力）。**Command receipt 在 Device durable 持久化前不得确认**，且只表示“已可靠接收”不表示“已成功执行”。终态必须携带稳定 ErrorCode 或明确 success result，不得把自由格式日志当作状态。

## 7. `SYNC_STATE` 与 Gateway certificate

`SYNC_STATE` payload 只携带非秘密、封闭的 Target snapshot 或其 typed plan。Device 必须验证 target Device、assignment/configuration revision、command freshness、本地 identity、payload schema/version，以及所有派生 hostname/upstream/profile 均来自允许集合。应用失败必须保留已验证 LKG 或进入 BLOCKED（详见 [状态与执行模型](state-and-execution.md)）。

**Gateway CSR 只允许在以下全部条件成立时提交**：QUIC peer 已通过 Device mTLS、存在 active `SYNC_STATE` Command、request 绑定该 `command_id`、target/revision/generation 一致、Server 当前 Target 仍允许该 hostname/profile、request 未过期。**Enrollment、匿名 HTTPS、普通 operator API 和通用 certificate endpoint 都不能签发 Gateway certificate。** CSR 中的 SAN/CN/自报 profile 不授予权限；Server 只用 CSR 证明 possession 和公钥结构。同一 `Device + command_id + generation + request_id + SPKI` 必须返回同一签发结果，不同 SPKI 返回 conflict。

## 8. `SYNC_SECRET`

Payload 在传输和内存中使用秘密专用类型。Device 必须在写入 vault 前重新验证当前 binding 和 revision；**陈旧 secret 不得安装**；vault 更新原子，不留半写。成功结果只报告已安装 credential revision、redacted status 和 audit correlation；**不得回显 password、hash 派生材料或完整 ciphertext**。

## 9. Observed snapshot

Observed 使用完整或可证明合并的 typed snapshot，**不使用自由格式 status map**；每个维度独立表达状态与有限诊断码，**密码值、private key、完整路径和内部异常链不得出现**。Server 只接受当前 authenticated Device 的 snapshot，校验单调 sequence、合法大小和 schema；**不能把 Device 自报字段直接当作授权**。

## 10. Local D-Bus

**Device Daemon ↔ Session Agent**：UI snapshot 只含展示所需数据（view kind、Seat code、binding 状态、session epoch 等），**不含 password、vault path、certificate private material、Server token 或任意 HTML**。调用校验 UID/PID/logind session 和 current epoch；action 是封闭 enum；陈旧 epoch 重放被拒绝；Agent 退出导致 lease 过期，不授予额外权限；**lock/unlock 不调用 Caddy adapter**。

**Device Daemon ↔ Privileged Helper**：Helper 方法按 capability 命名，参数必须是封闭 enum、规范化 ID、Helper 内重新派生或 allowlist 校验的路径/UID、明确 epoch，**无 secret**。**Helper 不接受 Server/QUIC request 的原始对象。**

## 11. Caddy Admin contract

Device Daemon **不发送任意 Caddyfile**；它从已验证 Target 和本地 certificate material 构造内部 activation plan，只允许固定 loopback listen、固定 origin/hostname、固定 DOMjudge upstream、固定 TLS material reference、固定 BLOCKED/READY route 集。执行前必须静态验证、验证证书/私钥匹配与 SAN/profile/有效期，再原子加载并健康检查；**未验证配置不得激活**。Session lock/unlock contract 不包含任何 Caddy 字段。

## 12. Stable ErrorCode

依赖方向：`DomainError → exhaustive adapter mapping → stable ErrorCode → HTTP/Protobuf/D-Bus/CommandStatus`。**禁止 `stable ErrorCode → domain decision`。**

规则：字符串值显式定义；每个公开 adapter 映射穷举；未分类内部错误映射到有限通用码；`detail` 默认无或脱敏；新内部错误不自动成为新稳定码；删除稳定码需要兼容计划；**Web/Device 不解析 Display 文本**；同一语义跨 transport 使用同一稳定码。

实际码值由 `natsume-error-code` crate 维护；该 registry 作为独立 crate 的决策见 [ADR-0019](adr/0019-stable-error-code-registry.md)（仍 `PROPOSED`）。

## 13. 版本和兼容

已发布的 field number、interface name、method/signal/error name、ID 和 revision 语义**不复用、不被数据迁移重写**；破坏性变化使用版本策略或新 interface version；vault format 变化必须有版本、原子迁移和恢复测试；downgrade/rollback 通过发布 runbook 定义，不假设 schema 自动回滚。

## 14. 契约验证

CI 必须证明：生成契约 clean diff、frame size/version/unknown enum 测试、Enrollment 无 Gateway 字段、D-Bus XML/Rust/policy 一致、ErrorCode 映射穷举、secret/path/source-chain redaction、匿名 QUIC 未进入 decoder、Session lock contract 无 Caddy 字段，以及禁止通用执行能力。具体检查随对应 Phase 实现补全。
