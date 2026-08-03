# Natsume V2 边界契约

> 状态：`NORMATIVE`  
> 适用范围：HTTP、Enrollment、WSS Device control、Protobuf、D-Bus、CommandStatus 和公开错误  
> 机器结构权威源：OpenAPI、Protobuf descriptor、D-Bus introspection、SQL migration 和生成代码

本文件定义稳定语义，不复制完整 schema。字段名、编号和路由必须从机器 schema 生成或验证。未实现行为的完整字段表与 wire 结构延迟到对应 Phase 实现时由机器 schema 定义。

## 1. 契约原则

1. 每个边界使用封闭 typed contract。
2. 输入必须先完成认证、大小限制、版本和结构校验，再进入 application。
3. **网络输入不得直接成为任意命令、路径、UID、unit、环境、upstream 或配置片段。**
4. 公开边界返回稳定 `ErrorCode`；领域内部保留 typed error。
5. **password 明文、private key 与 Device Token 值不进入通用 API、Observed、日志、指标或普通 audit。**
6. 兼容性只在明确的 wire/schema version 范围内提供。
7. 未知 enum/oneof、超限 frame、重复非法字段或版本不匹配必须显式失败。
8. 人类叙述状态不能偷偷成为 wire 字段。
9. Command replay 由 Panel-owned `command_id` 以及 `request_fingerprint_version` / `request_fingerprint_sha256` 定义；持久化 payload 使用 `payload_version` 与 `frozen_payload_json`。

## 2. 契约所有权与入口拓扑

每个边界的机器权威源（OpenAPI、Protobuf descriptor、D-Bus introspection、SQL migration）是其结构的唯一来源；语义由对应模块拥有。**不得手工维护与生成契约并行的“第二份完整字段表”。**

Operator HTTP、Enrollment 与 Device WSS **合并到同一 TCP 端口**（[ADR-0033](adr/0033-enrollment-and-device-control-boundary.md)），各自使用独立路由、授权与限流；防火墙面为一个 TCP 端口。

## 3. Operator HTTP

### 3.1 基本要求

- 使用 HTTPS；operator session 与两级固定角色（`admin` / `viewer`，[ADR-0034](adr/0034-state-execution-and-data-plane-boundary.md)）在 Server 边界执行；
- mutation 必须有 correlation ID，返回“领域已提交”或“Command 已创建”，**不虚构远端完成**；需要 audit 的 mutation 由其 guarded operation 在同一 transaction 内自行插入 audit row；fresh `audit_event_id` 可作为 typed input，但已持久化的同 ID 或预插入 audit row 不能重放为新 mutation 的依据；
- destructive / high-impact mutation 要求明确确认语义；contest configuration 的 **Import Commit** 本身即二次确认动作，不新增独立 confirmation resource；
- 非 Command mutation 使用其领域的 CAS/revision 或明确的 repeat-safe 语义；不得用浏览器重试猜测副作用边界。

### 3.2 Problem Details

HTTP 错误使用 Problem Details 或等价结构（`type`/`title`/`status`/`code`/`correlation_id`/`detail?`/`field_errors?`）。`code` 来自稳定 ErrorCode registry；**调用方不得解析 `title` 或 `detail` 判断业务。** `detail` 仅允许脱敏、对人类有用的描述。

### 3.3 Secret API 约束

允许：上传 CSV 到受限 staging；展示 password **是否**变化（布尔/分类级 redacted 证据）；人工触发 `SYNC_SECRET`；展示 credential revision 和 redacted result；展示 opaque `preview_token`、baseline revision、redacted import summary 与 binding impacts。

**禁止**：

- 返回 password 值；
- 提供通用 secret read endpoint；
- 把 password 明文放入 audit diff、metric 或普通 log；
- 在错误 `detail` 中包含 CSV 行原文或密码；
- 将浏览器 local/session storage 作为秘密存储。

### 3.4 Operator import 边界

Import 是对 confirmed contest configuration 的高影响路径。稳定语义（领域规则以 [领域模型](domain-model.md) 为准，并发模型见 [ADR-0031](adr/0031-contest-import-and-secret-evidence.md)）：

- **全局同一时刻最多一个 encrypted pending candidate**；`pending_import_candidate` singleton row 存在即为 pending，新 upload 前需显式终止现有 candidate；
- candidate 只在严格解析成功后持久化，row 只包含 candidate ID、expiry、`baseline_configuration_revision`、`baseline_binding_revision`、`preview_token_hash`、`payload_vault_record_id` 和 `redacted_preview_json`；普通 surface 使用 `candidate_id` 和 opaque `preview_token`，不使用 import state/history 或可见 password-bearing snapshot；
- **Server 是 diff classification 的唯一权威**；client/UI 只渲染结构化结果，不得本地重算分类；
- preview 绑定 candidate identity、baseline `configuration_revision`、baseline `binding_revision`、redacted diff 与过期时间；数据库只保存 opaque token 的 hash；
- Commit 校验为**双 CAS**：前者保护 Seat 集合、`account_mappings` 和密码内容，后者保护全局 Seat↔Device Binding 集合；任一前移即拒绝并要求重新 preview；
- Commit、discard 和 expiry 在各自事务中删除 candidate 与其引用的 encrypted payload vault row，仅留下 redacted audit lineage；重复请求不能借由保留 terminal candidate state 取得新的业务结果；
- Import Commit 不创建 Command，不自动 `SYNC_STATE`/`SYNC_SECRET`，**不产生 Device I/O**，也不表示 Device 已同步；
- 任何 invalid、stale、expiry、discard、authorization failure 或 transaction failure **均不得改变 confirmed truth、binding 或相关 revision**；
- 清空 confirmed configuration 只能通过独立 single-lifetime reset，不得由 import 隐式完成。

### 3.5 Direct Command creation

Command create/replay 的唯一 HTTP 资源契约为：

```text
PUT /api/v2/commands/{command_id}
```

- **Panel 在发起请求前生成 `command_id`**。它必须是 canonical lowercase hyphenated UUIDv7：可解析、version 为 7，且其 canonical string 与 path 输入逐字符相同。Server 不生成、重写或为同一意图替换这个 ID。
- request 的持久化 target 是 `device_pk`，并使用封闭 `kind`、可选 `group_correlation_id` 与 versioned typed client input。`reason_code` 等 typed input 只有在 `frozen_payload_json` schema 接受时才保存；`commands` 不另设这些 top-level columns。
- Server 对 canonical request 计算 versioned、domain-separated SHA-256 fingerprint，并持久化为 `request_fingerprint_version` 和 `request_fingerprint_sha256`。输入覆盖 target Device、kind、reason、group correlation 和 typed client input；不覆盖 retry time、actor 或 Server 后续冻结/派生状态。
- 没有该 ID 时，Server 原子持久化 Command 与 `created_audit_event_id` 指向的创建 audit，返回 `201`。已有该 ID 且 fingerprint 相同，返回同一个已持久化 Command，返回 `200`，不再写 audit 或重复任何副作用。已有该 ID 且 fingerprint 不同，返回 `409` / `COMMAND_REQUEST_CONFLICT`。
- 非 canonical UUIDv7 返回 `400` / `COMMAND_ID_INVALID`。错误不得回显原始 request、fingerprint、secret 或未脱敏诊断。
- 每个 bulk target 使用一个独立 `command_id`。可选 `group_correlation_id` 只用于查询和审计分组；即使它参与 fingerprint，也不表达顺序、原子性、重试或跨 Device lifecycle。
- 本节冻结 OpenAPI 和后续实现必须遵守的语义；**不声明 Phase 0 已经提供 HTTP listener、授权 handler、Command repository、dispatcher 或实际 Panel mutation。**

## 4. Enrollment

Enrollment 使用 server-auth HTTPS：Client 必须验证预配置 Server trust 和 IP-SAN/endpoint；**不使用 TOFU 或 dangerous verifier**；请求有严格大小和速率限制；与 operator API 共享进程但使用独立路由、授权和限流。

**窗口门禁**：仅在 `provisioning_window.state = 'open'` 时受理（[ADR-0033](adr/0033-enrollment-and-device-control-boundary.md)）；singleton 只有 `state`、`revision` 和 `last_audit_event_id`。窗口关闭时以稳定 ErrorCode 拒绝且零状态变更；restart/restore 的 open→closed recovery 是 close-once audited CAS，不是历史状态 replay。

**请求**包含 `enrollment_requests` 所需的 `machine_hardware_id`、`hardware_identity_quality`、`gateway_csr_der`、`gateway_spki_sha256`、`client_version` 和 `protocol_version`；Server 记录 `source_ip`、state、可选 resolution/resolved device/issuance audit 和 `created_at`。**不得包含**：Caddy config、DOMjudge password、任意 certificate profile、任意路径/unit/shell。

**响应**可携带 `device_pk`、Device Token、Gateway leaf + chain；持久化只记录 `devices`、`enrollment_requests`、`device_tokens`、`gateway_certificates` 与 `audit_events` 的 migration-defined facts。`gateway_certificates` 不保存 leaf/chain bytes，失败无半成品。

**替换语义**：同一 `machine_hardware_id` 窗口内重复 Enrollment 可用 `resolution = 'replace_device_credentials'` 关联既有 `device_pk`；替换 `device_tokens.token_hash` 并签发新的 certificate metadata，作为 re-enrollment 审计；若旧连接仍存活，记录异常审计事件。

**Client 收尾**：leaf 与本地私钥 SPKI 匹配、chain 通到预置 origin CA、SAN 等于配置 hostname、本地持久化原子完成后才提交结果；**中途失败不得留下“看似已 Enrollment”的半状态**（重试自然落入替换语义）。

## 5. Device control：WSS

- **传输**：WebSocket over server-auth TLS；Protobuf 消息作为 WS binary frame（一帧一消息，无自定义 length-prefix framing）；协议版本经 `Sec-WebSocket-Protocol`（如 `natsume.v1`）协商，不匹配在 upgrade 拒绝；
- **认证**：Device Token 必须由 CSPRNG 生成 32 bytes；upgrade 时经 `Authorization: Bearer <Device Token>` 提交；Server 常数时间比对 `device_tokens.token_hash` 并映射 `device_pk`；**无 token / 错误 token / 不再有对应 token row → 401，发生在任何 Protobuf 解码之前**；
- TLS early data（0-RTT）保持关闭；认证失败按 IP 限流；
- Frame 必须有明确最大长度、封闭 envelope kind 和 command/correlation ID；超限 frame、未知版本、非法 oneof 必须关闭连接，**不得猜测**；
- `Command.command_id` 和 `CommandStatus.command_id` 必须验证为 canonical UUIDv7，并将 HTTP path 的同一字符序列原样带入和带回；WSS/Device 不得另行生成或格式化 ID，且 validation error 不回显原 ID；
- keep-alive 使用 WS ping/pong；连接中断不改变 Server truth；重连后通过 direct durable Command 和 Observed 收敛。

## 6. Command 契约

Command current row 直接绑定 `command_id`、`device_pk`、`kind`、`state`、request fingerprint version/hash、可选 `group_correlation_id`、`payload_version`、`frozen_payload_json`、`created_at`、`deadline_at`、可选 terminal error/result 和 `created_audit_event_id`。它不保存秘密 payload copy、certificate 或 token issuance 数据；每 Command 的 frozen typed input 只在 `frozen_payload_json` 内表示。

V2 业务 family 限于：`SYNC_STATE`、`SYNC_SECRET`、`SESSION_LOCK`/`UNLOCK`/`TERMINATE`、`HOME_PREPARE`/`CLEAN`、`OBSERVE_NOW`、`DEVICE_RETIRE`（具体枚举以 `.proto` 为准；新增 family 必须证明不是任意远程管理能力）。**Command receipt 在 Device durable 持久化前不得确认**，且只表示“已可靠接收”不表示“已成功执行”。终态由可选 `terminal_error_code` 或 `redacted_terminal_result_json` 表示（[ADR-0034](adr/0034-state-execution-and-data-plane-boundary.md)）。

相同 `command_id` 的重投递必须保持同一 `frozen_payload_json`；Device journal 对相同 ID 不重复副作用，对同 ID 但不同 frozen payload conflict/reject。Server DB、WSS Command、journal、CommandStatus 和 audit correlation 的 ID 关联不可断开。

## 7. `SYNC_STATE`

`SYNC_STATE` payload 只携带非秘密、封闭的 Target snapshot 或其 typed plan。Device 必须验证 target Device、baseline revision 与派生代际、command freshness、本地 identity、payload schema/version，以及所有派生 hostname/upstream 均来自允许集合。应用失败必须保留已验证 LKG 或进入 BLOCKED（详见 [状态与执行模型](state-and-execution.md)）。

**`SYNC_STATE` 不签发、不携带、不安装任何证书或 token**；Gateway certificate 只在 Enrollment 获得（`INV-CERT-01`）。

## 8. `SYNC_SECRET`

Payload 在传输和内存中使用秘密专用类型。Device 必须在写入前重新验证当前 binding 和 revision；**陈旧 secret 不得安装**；凭据文件更新原子，不留半写；随后由 Daemon 重渲染含凭据的 Caddy `/login` 注入配置并原子激活（[ADR-0034](adr/0034-state-execution-and-data-plane-boundary.md)）。成功结果只报告已安装 credential revision、redacted status 和 audit correlation；**不得回显 password**。

## 9. Observed snapshot

Observed 使用完整或可证明合并的 typed snapshot，**不使用自由格式 status map**；每个维度独立表达状态与有限诊断码，**密码值、token 值、private key、完整路径和内部异常链不得出现**。上报节奏为**变化时上报 + 低频周期兜底**（带宽约束，ADR-0030 F2）。Server 只接受当前 authenticated Device 的 snapshot，校验单调 sequence、合法大小和 schema；**不能把 Device 自报字段直接当作授权**。

## 10. Local D-Bus

**Device Daemon ↔ Session Agent**：UI snapshot 只含展示所需数据（view kind、Seat code、binding 状态、session epoch 等），**不含 password、token、certificate private material、Server 凭据或任意 HTML**。view kind 与 action 为封闭 enum，经版本升级路径扩展（[ADR-0035](adr/0035-session-home-and-desktop-cycle.md)）。调用校验 UID/PID/logind session 和 current epoch；陈旧 epoch 重放被拒绝；Agent 退出导致 lease 过期，不授予额外权限；**lock/unlock 不调用 Caddy adapter**。

**Device Daemon ↔ Privileged Helper**：Helper 方法按 capability 命名，参数必须是封闭 enum、规范化 ID、Helper 内重新派生或 allowlist 校验的路径/UID、明确 epoch，**无 secret**。**Helper 不接受 Server/WSS request 的原始对象。**

## 11. Caddy 控制契约

Device Daemon **不发送任意 Caddyfile、不使用 Caddy Admin API**。控制路径为（[ADR-0034](adr/0034-state-execution-and-data-plane-boundary.md)）：

```text
已验证 Target + 本地证书/凭据材料
  → Daemon 渲染完整配置文件（固定 loopback listen、固定 hostname、固定 DOMjudge upstream、
     固定 TLS material 引用、固定 BLOCKED/READY route 集、仅 /login 的 header 注入）
  → caddy validate
  → 原子替换配置文件（temp + fsync + rename）
  → systemd path unit 触发 reload
  → 本地健康检查；失败回滚 LKG 配置文件
```

执行前必须验证证书/私钥匹配与 SAN/有效期；**未验证配置不得激活**。含凭据的渲染配置是 secret artifact（`0640 root:natsume-gateway`）。`Accept-Encoding` 保持透传，不配置 `encode`（brotli 在 upstream 完成，ADR-0030 F5）。Session lock/unlock contract 不包含任何 Caddy 字段。

## 12. Stable ErrorCode

依赖方向：`DomainError → exhaustive adapter mapping → stable ErrorCode → HTTP/Protobuf/D-Bus/CommandStatus`。**禁止 `stable ErrorCode → domain decision`。**

规则：字符串值显式定义；每个公开 adapter 映射穷举；未分类内部错误映射到有限通用码；`detail` 默认无或脱敏；新内部错误不自动成为新稳定码；删除稳定码需要兼容计划；**Web/Device 不解析 Display 文本**；同一语义跨 transport 使用同一稳定码。

`COMMAND_ID_INVALID` 是非 canonical UUIDv7 的 `400`；`COMMAND_REQUEST_CONFLICT` 是 same-ID/different-fingerprint 的 `409`。实际码值由 `natsume-error-code` crate 维护；该 registry 作为独立 crate 的治理决策已由 [ADR-0036](adr/0036-error-architecture-and-public-codes.md) 接受，实施完成度仍以 Gate evidence 为准。

## 13. 版本和兼容

已发布的 field number、interface name、method/signal/error name、ID 和 revision 语义**不复用、不被数据迁移重写**；破坏性 wire 变化使用新 WS subprotocol 或 interface version；downgrade/rollback 通过发布 runbook 定义，不假设 schema 自动回滚。

## 14. 契约验证

CI 必须证明：生成契约 clean diff；`PUT /api/v2/commands/{command_id}` 的 `201/200/400/409`、canonical UUIDv7 正/反例、same-ID/same-fingerprint replay 与 same-ID/different-fingerprint conflict、`request_fingerprint_*`/`frozen_payload_json` 持久化、ID 在 HTTP/WSS/journal/status/audit 的一致性；WS frame size/version/unknown enum 测试；窗口关闭时 Enrollment 拒绝且零变更；open-window recovery close-once；无 token upgrade 在解码前 401；D-Bus XML/Rust/policy 一致；ErrorCode 映射穷举；secret/path/source-chain redaction；`/login` 之外路由无注入头；Session lock contract 无 Caddy 字段；禁止通用执行能力。具体检查随对应 Phase 实现补全。
