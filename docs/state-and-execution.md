# Natsume V2 状态与执行模型

> 状态：`NORMATIVE`  
> 适用范围：Target、Observed、Drift、Command、Caddy、Session 和 Home 的安全 outcome  
> 相关不变量：`INV-STATE-01`、`INV-SECRET-02`、`INV-COMMAND-01`、`INV-DATAPLANE-01`、`INV-DATAPLANE-02`、`INV-SESSION-01`

## 1. 状态与副作用分离

Natsume 同时处理：已提交事实（Server truth）、期望状态（Target）、实际状态（Observed）、纯差异（Drift）、人工意图（Command）和本地原子激活（Caddy/Home）。这些概念压缩成一个“device status”会产生高耦合：普通 CRUD 依赖网络、重试改变业务事实、UI 文案变成状态机输入。本文件冻结这些层次的安全 outcome；具体状态机、字段与事务编排延迟到对应 Phase 实现。

- **Server truth**：已提交领域事实。**提交 Server truth 不意味着 Device 已完成**；import 不创建远端副作用。
- **Target**：从 Server truth 派生的非秘密期望。**不含明文密码，确定性派生，不自动联系 Device。**
- **Observed**：Device 的 typed 实际状态报告。**只接受认证、有界、typed 的 observation；Device 自报属性不构成授权。**
- **Drift**：`compare(Target, latest valid Observed)` 的纯比较结果，可重算。
- **Command**：单 Device durable intent；批量操作 = 批量 Command + 查询聚合。持久化范围仅为 migration 定义的 `commands` current row，不声明独立 delivery history 或 dispatch statistics（[ADR-0034](adr/0034-state-execution-and-data-plane-boundary.md)）。

## 2. Command identity、replay 与 conflict

- **ID authority**：Panel 在每次创建前生成 canonical lowercase hyphenated UUIDv7 `command_id`。它使用 `PUT /api/v2/commands/{command_id}`；Server、WSS 和 Device journal 不生成、重写或替换该 ID。
- **canonical request**：Server 以 versioned、domain-separated fingerprint 覆盖通过 schema 验证的 HTTP request 值 `device_id`、`kind`、`payload_version`、`payload`、可选 `reason_code` 与可选 `group_correlation_id`，并保存为 `request_fingerprint_version` 与 `request_fingerprint_sha256`；不覆盖 frozen timestamps、actor、session 或 retry time。同 ID + 同 fingerprint 返回既有 Command；同 ID + 不同 fingerprint 是稳定 conflict。
- **HTTP outcome**：只有当前 `enrolled` Device 可首次持久化，成功为 `201`；target 不存在或存在但 state 不是 `enrolled` 都返回相同四字段 body 的 `404` / `RESOURCE_NOT_FOUND`；same-ID/same-request replay 为 `200`；非 canonical UUIDv7 为 `400` / `COMMAND_ID_INVALID`；same-ID/different-request 为 `409` / `COMMAND_REQUEST_CONFLICT`。Device 在创建后被 disable/revoke 不改变 replay/conflict 分类；这两个分支先于首次持久化资格检查并忽略当前 state。这些 outcome 只表示 Server 持久化，不表示 Device 已执行。
- **跨平面 identity**：同一 ID 字符串必须原样出现在 Server Command、WSS `Command`、Device durable journal、WSS `CommandStatus` 和 per-Command audit correlation。Device durable journal 保存收到的 Command frame bytes；同 ID 且 frame bytes 相同时不重复副作用，同 ID 但 frame bytes 不同时以 `COMMAND_PAYLOAD_CONFLICT` 拒绝。Server 从已存储的 frozen payload 确定性渲染该 ID 的 wire Command，使每次重新投递的 frame byte-identical。
- **执行状态**：已成功不重复副作用；正在运行不并发执行第二次；已失败按策略返回终态，不偷偷重启。需要重新执行时，Panel 创建新的 Command ID。
- **revision/epoch**：Device 在执行前和关键原子提交前检查适用的 binding/configuration/credential/session/home current facts；**陈旧时用稳定错误拒绝，不“尽量兼容”地部分应用。**
- **receipt 与恢复**：Command receipt 在 Device durable 持久化前不得确认；进程崩溃后相同 Command 能恢复或返回原结果。终态不可被后来的 transport error 覆盖；重复终态消息可安全合并。
- **bulk**：每个 target 是独立 Command；可选 `group_correlation_id` 仅支持查询和审计分组，不定义跨 Device 顺序、原子性、retry 或 lifecycle。

具体 Command 状态机、HTTP handler、dispatcher、Device journal 和 Panel mutation 在后续 Phase 实现；本文件不把该契约当作实现完成声明。

## 3. `SYNC_STATE` 的安全 outcome

`SYNC_STATE` 必须由操作员显式触发（不自动）。激活失败的 fail-closed 规则：

- **任何中途失败必须保留已验证 LKG 或进入 BLOCKED，不暴露未验证配置；**
- Target 陈旧时拒绝，不修改本地状态；
- 证书/私钥验证失败不激活；`caddy validate` 失败不 reload；reload 失败时回滚 LKG 配置文件并确认旧配置仍有效，否则 BLOCKED；
- upstream 不健康或 `/login` 非 TLS 时按冻结 policy 保持 BLOCKED 或 READY-with-health，不自由猜测；
- Observed 上传失败不回滚本地已成功原子动作，重连重报。

`SYNC_STATE` 不签发、不携带、不安装任何证书或 token（`INV-CERT-01`）。具体阶段序列在 Phase 5 实现时定义。

## 4. Gateway readiness

Device Token 与 Gateway certificate 都在 Enrollment 获得（[ADR-0033](adr/0033-enrollment-and-device-control-boundary.md)），但 **Enrollment 成功不得被展示为数据面 ready**：READY 还需要 Target 应用、配置渲染、validate、reload 与健康检查全部通过。证书持有与数据面状态是两个独立维度。

## 5. `SYNC_SECRET` 的安全 outcome

`SYNC_SECRET` 必须：

- 只能由人类明确触发，**不能由 Target drift 自动触发；**
- Command 的 frozen typed input 使用 `frozen_payload_json`；Device 写入前重新校验适用的 BindingRevision 与 credential revision；
- 凭据文件更新原子，失败时保留旧 secret 或明确标记不可用，**不留半写**；
- 成功后重渲染 Caddy `/login` 注入配置并原子激活（[ADR-0034](adr/0034-state-execution-and-data-plane-boundary.md)）；
- 成功后 Observed 只报告 revision；transport retry 保持相同 Command ID，不重复不可逆动作；
- 结果 redacted，不向普通 surface 暴露 secret。

具体阶段序列在 Phase 5 实现时定义。

## 6. Caddy 状态

Caddy 业务状态只需 `BLOCKED` / `READY`。

- **BLOCKED**：主页面 HTTP 503；只显示 allowlist 状态；静态本地资源；严格 CSP；动态值只通过 `textContent`；**不显示 password、路径、自由格式错误或 `session_locked`；不代理 DOMjudge。**
- **READY**：需证明当前 Target/revision、Gateway certificate 与 private key 匹配、SAN/有效期、`caddy validate` 通过、fixed TLS upstream policy、reload 成功、本地健康检查、LKG 写入成功或可恢复。

**Session lock/unlock/terminate 不触碰 Caddy 配置、不改变 Caddy 状态，也不将 `session_locked` 放入状态页。**

## 7. Session 与 Home

- **Session**：每个 transition 绑定当前 `SessionEpoch`；Agent 通过 lease 证明属于当前 logind session。陈旧 Agent 或 UI action 被拒绝；Agent 崩溃后 lease 过期，不解锁额外权限，不改变 Caddy。锁定语义走当期镜像桌面的原生 session lock；遮罩类 UI 是呈现层，不是完整性边界（[ADR-0035](adr/0035-session-home-and-desktop-cycle.md)）。
- **Home**：每次 `HOME_RESET` 建立新的、由 Server 分配且严格单调的 `HomeEpoch`；reset 完成前不启动受管 session；中断的 reset 必须可恢复；**无法证明 mount/copy/ownership 安全时 fail closed；不静默切换 backend。** 重置仍是操作员在场的受控事件，实现为状态文件 + 可安全重复运行的步骤；本地 `PrepareHomeInstance` / `ActivateHomeInstance` / `RecoverHomeInstance` / `GarbageCollectHomeInstance` 分解只属实现面，D-Bus surface 保持不变。

## 8. 可观测性

Server 与 Device 指标追踪连接、Observed freshness、Drift、enrollment/签发结果与 stable ErrorCode。`group_correlation_id` 只能作为有限的查询/审计分组字段，不承载 workflow status。**指标 label 不得包含密码、token 值、路径、certificate body、Machine ID 全值或自由格式错误。**

## 9. 测试模型

必须覆盖的安全 fault class：Panel canonical UUIDv7 正/反例、`PUT` 首次 `201` / replay `200` / invalid `400` / conflict `409`、same-ID/same-fingerprint、same-ID/different-fingerprint、`request_fingerprint_*`/`frozen_payload_json`、HTTP/WSS/journal/status/audit 同 ID、Server 事务成功但 Device 离线、receipt 前后断线、执行中崩溃、重复 Command、陈旧 revision/epoch、窗口关闭签发拒绝、open-window restart/restore close-once、重复 Enrollment 替换语义、无 token upgrade 拒绝、WSS 断线重连收敛、Caddy validate/reload 中断、old LKG 保留、secret 写入中断、Observed 丢失重发、Agent crash/focus denied/display lost、Home reset 中断、cancel 与 terminal status race。具体测试场景随对应 Phase 实现补全。
