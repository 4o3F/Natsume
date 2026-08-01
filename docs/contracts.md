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

## 2. 契约所有权与入口拓扑

每个边界的机器权威源（OpenAPI、Protobuf descriptor、D-Bus introspection、SQL migration）是其结构的唯一来源；语义由对应模块拥有。**不得手工维护与生成契约并行的"第二份完整字段表"。**

Operator HTTP、Enrollment 与 Device WSS **合并到同一 TCP 端口**（[ADR-0023](adr/0023-wss-control-channel-with-device-token.md)），各自使用独立路由、授权与限流；防火墙面为一个 TCP 端口。

## 3. Operator HTTP

### 3.1 基本要求

- 使用 HTTPS；operator session 与两级固定角色（`admin` / `viewer`，[ADR-0029](adr/0029-right-sizing-control-plane-machinery.md)）在 Server 边界执行；
- mutation 必须有 request/correlation ID，返回"领域已提交"或"Command 已创建"，**不虚构远端完成**；
- destructive / high-impact mutation 要求明确确认语义；contest configuration 的 **Import Commit** 本身即二次确认动作，不新增独立 confirmation resource；
- mutation 幂等由 CAS/revision 或天然幂等语义保证，不依赖浏览器重试猜测。

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

Import 是对 confirmed contest configuration 的高影响路径。稳定语义（领域规则以 [领域模型](domain-model.md) 为准，并发模型见 [ADR-0028](adr/0028-single-operator-import-and-secret-evidence-scope.md)）：

- **全局同一时刻最多一个 pending candidate**；新 upload 前需显式 discard 旧 candidate；
- **Server 是 diff classification 的唯一权威**；client/UI 只渲染结构化结果，不得本地重算分类；
- 普通 surface 只使用 opaque `preview_token`（绑定 candidate 身份、baseline revision、redacted diff 与过期时间）；
- Commit 校验为**双 CAS**：baseline `ContestConfigurationRevision` + `AssignmentRevision`；任一前移即拒绝并要求重新 preview；重复提交因 CAS 前移而安全失败；
- Import Commit 不创建 Command，不自动 `SYNC_STATE`/`SYNC_SECRET`，**不产生 Device I/O**，也不表示 Device 已同步；
- 任何 invalid、stale、expiry、discard、authorization failure 或 transaction failure **均不得改变 confirmed truth、binding 或相关 revision**；
- 清空 confirmed configuration 只能通过独立 single-lifetime reset，不得由 import 隐式完成。

## 4. Enrollment

Enrollment 使用 server-auth HTTPS：Client 必须验证预配置 Server trust 和 IP-SAN/endpoint；**不使用 TOFU 或 dangerous verifier**；请求有严格大小和速率限制；与 operator API 共享进程但使用独立路由、授权和限流。

**窗口门禁**：仅在 provisioning 窗口开启时受理（[ADR-0021](adr/0021-provisioning-window-certificate-issuance.md)）；窗口关闭时以稳定 ErrorCode 拒绝且零状态变更。

**请求**包含：`MachineHardwareId`、规范化硬件证据摘要、Gateway CSR、协议/客户端版本。**不得包含**：Caddy config、DOMjudge password、任意 certificate profile、任意路径/unit/shell。

**响应**包含：`device_id`、Device Token、Gateway leaf + chain。签发在同一 Server 事务中完成（Device 行 + token 哈希 + 证书台账 + AuditEvent），失败无半成品。

**替换语义**：同一 `MachineHardwareId` 窗口内重复 Enrollment 使旧 token 立即失效、签发新产物，作为 re-enrollment 审计；若旧连接仍存活，记录异常审计事件。

**Client 收尾**：leaf 与本地私钥 SPKI 匹配、chain 通到预置 origin CA、SAN 等于配置 hostname、本地持久化原子完成后才提交结果；**中途失败不得留下"看似已 Enrollment"的半状态**（重试自然落入替换语义）。

## 5. Device control：WSS

- **传输**：WebSocket over server-auth TLS；Protobuf 消息作为 WS binary frame（一帧一消息，无自定义 length-prefix framing）；协议版本经 `Sec-WebSocket-Protocol`（如 `natsume.v1`）协商，不匹配在 upgrade 拒绝；
- **认证**：upgrade 时经 `Authorization: Bearer <Device Token>` 提交；Server 常数时间比对哈希并映射 DevicePk；**无 token / 错误 token / 已吊销 token → 401，发生在任何 Protobuf 解码之前**；
- TLS early data（0-RTT）保持关闭；认证失败按 IP 限流；
- Frame 必须有明确最大长度、封闭 envelope kind、correlation/command ID 和解码失败稳定错误；超限 frame、未知版本、非法 oneof 必须关闭连接，**不得猜测**；
- keep-alive 使用 WS ping/pong；连接中断不改变 Server truth；重连后通过 durable Command 和 Observed 收敛。

## 6. Command 契约

Command 绑定 `command_id`、kind、target Device、issued-at/expiry policy、必要 revision/epoch、typed payload version 与 redacted audit correlation。

**禁止通用能力**：`EXEC`、`RUN_SHELL`、`WRITE_FILE`、`SYSTEMD_UNIT`、`INSTALL_CERTIFICATE`、`APPLY_CADDY_FRAGMENT`、`SET_ENV`，以及任意 URL/upstream/path/UID。

V2 业务 family 限于：`SYNC_STATE`、`SYNC_SECRET`、`SESSION_LOCK`/`UNLOCK`/`TERMINATE`、`HOME_PREPARE`/`CLEAN`、`OBSERVE_NOW`、`DEVICE_RETIRE`（具体枚举以 `.proto` 为准；新增 family 必须证明不是任意远程管理能力）。**Command receipt 在 Device durable 持久化前不得确认**，且只表示"已可靠接收"不表示"已成功执行"。终态必须携带稳定 ErrorCode 或明确 success result；投递/重试观察记录为 Command 元数据，不是独立业务概念（[ADR-0029](adr/0029-right-sizing-control-plane-machinery.md)）。

## 7. `SYNC_STATE`

`SYNC_STATE` payload 只携带非秘密、封闭的 Target snapshot 或其 typed plan。Device 必须验证 target Device、baseline revision 与派生代际、command freshness、本地 identity、payload schema/version，以及所有派生 hostname/upstream 均来自允许集合。应用失败必须保留已验证 LKG 或进入 BLOCKED（详见 [状态与执行模型](state-and-execution.md)）。

**`SYNC_STATE` 不签发、不携带、不安装任何证书或 token**；Gateway certificate 只在 Enrollment 获得（`INV-CERT-01`）。

## 8. `SYNC_SECRET`

Payload 在传输和内存中使用秘密专用类型。Device 必须在写入前重新验证当前 binding 和 revision；**陈旧 secret 不得安装**；凭据文件更新原子，不留半写；随后由 Daemon 重渲染含凭据的 Caddy `/login` 注入配置并原子激活（[ADR-0024](adr/0024-domjudge-autologin-via-xheaders.md)）。成功结果只报告已安装 credential revision、redacted status 和 audit correlation；**不得回显 password**。

## 9. Observed snapshot

Observed 使用完整或可证明合并的 typed snapshot，**不使用自由格式 status map**；每个维度独立表达状态与有限诊断码，**密码值、token 值、private key、完整路径和内部异常链不得出现**。上报节奏为**变化时上报 + 低频周期兜底**（带宽约束，ADR-0022 F2）。Server 只接受当前 authenticated Device 的 snapshot，校验单调 sequence、合法大小和 schema；**不能把 Device 自报字段直接当作授权**。

## 10. Local D-Bus

**Device Daemon ↔ Session Agent**：UI snapshot 只含展示所需数据（view kind、Seat code、binding 状态、session epoch 等），**不含 password、token、certificate private material、Server 凭据或任意 HTML**。view kind 与 action 为封闭 enum，经版本升级路径扩展（[ADR-0027](adr/0027-single-image-desktop-cycle.md)）。调用校验 UID/PID/logind session 和 current epoch；陈旧 epoch 重放被拒绝；Agent 退出导致 lease 过期，不授予额外权限；**lock/unlock 不调用 Caddy adapter**。

**Device Daemon ↔ Privileged Helper**：Helper 方法按 capability 命名，参数必须是封闭 enum、规范化 ID、Helper 内重新派生或 allowlist 校验的路径/UID、明确 epoch，**无 secret**。**Helper 不接受 Server/WSS request 的原始对象。**

## 11. Caddy 控制契约

Device Daemon **不发送任意 Caddyfile、不使用 Caddy Admin API**。控制路径为（[ADR-0029](adr/0029-right-sizing-control-plane-machinery.md)）：

```text
已验证 Target + 本地证书/凭据材料
  → Daemon 渲染完整配置文件（固定 loopback listen、固定 hostname、固定 DOMjudge upstream、
     固定 TLS material 引用、固定 BLOCKED/READY route 集、仅 /login 的 header 注入）
  → caddy validate
  → 原子替换配置文件（temp + fsync + rename）
  → systemd path unit 触发 reload
  → 本地健康检查；失败回滚 LKG 配置文件
```

执行前必须验证证书/私钥匹配与 SAN/有效期；**未验证配置不得激活**。含凭据的渲染配置是 secret artifact（`0640 root:natsume-gateway`）。`Accept-Encoding` 保持透传，不配置 `encode`（brotli 在 upstream 完成，ADR-0022 F5）。Session lock/unlock contract 不包含任何 Caddy 字段。

## 12. Stable ErrorCode

依赖方向：`DomainError → exhaustive adapter mapping → stable ErrorCode → HTTP/Protobuf/D-Bus/CommandStatus`。**禁止 `stable ErrorCode → domain decision`。**

规则：字符串值显式定义；每个公开 adapter 映射穷举；未分类内部错误映射到有限通用码；`detail` 默认无或脱敏；新内部错误不自动成为新稳定码；删除稳定码需要兼容计划；**Web/Device 不解析 Display 文本**；同一语义跨 transport 使用同一稳定码。

实际码值由 `natsume-error-code` crate 维护；该 registry 作为独立 crate 的决策见 [ADR-0019](adr/0019-stable-error-code-registry.md)（仍 `PROPOSED`）。

## 13. 版本和兼容

已发布的 field number、interface name、method/signal/error name、ID 和 revision 语义**不复用、不被数据迁移重写**；破坏性变化使用新 WS subprotocol 版本或新 interface version；凭据/配置文件格式变化必须有版本头和恢复测试；downgrade/rollback 通过发布 runbook 定义，不假设 schema 自动回滚。

## 14. 契约验证

CI 必须证明：生成契约 clean diff；WS frame size/version/unknown enum 测试；窗口关闭时 Enrollment 拒绝且零变更；无 token upgrade 在解码前 401；D-Bus XML/Rust/policy 一致；ErrorCode 映射穷举；secret/path/source-chain redaction；`/login` 之外路由无注入头；Session lock contract 无 Caddy 字段；禁止通用执行能力。具体检查随对应 Phase 实现补全。
