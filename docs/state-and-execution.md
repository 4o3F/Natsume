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
- **Command**：单 Device 显式意图，分 Converge（领域键幂等，非终态时可重投同一 payload）与 Oneshot（仅 live socket）。Server actor 与 Client executor 以单消费者 channel 串行 Command；`commands` current row 用 Device 内 `enqueue_order` 冻结 FIFO、用 `queued → in_flight` 表达 durable delivery cut 与 operator 审计，不声明 Device journal 或独立 delivery history。

## 2. Command identity、replay 与投递二分

- **ID authority**：Panel 在每次创建前生成 canonical lowercase hyphenated UUIDv7 `command_id`。它使用 `PUT /api/v2/commands/{command_id}`；Server 与 WSS 不生成、重写或替换该 ID。PUT 是 operator 审计入口，不是 Device 执行权威。
- **canonical request**：Server 以 versioned、domain-separated fingerprint 覆盖通过 schema 验证的 HTTP request 值 `device_id`、`kind`、`payload_version`、`payload`、可选 `reason_code` 与可选 `group_correlation_id`，并保存为 `request_fingerprint_version` 与 `request_fingerprint_sha256`；不覆盖 frozen timestamps、actor、session 或 retry time。同 ID + 同 fingerprint 返回既有 Command；同 ID + 不同 fingerprint 是稳定 conflict。
- **HTTP outcome**：只有当前 `enrolled` Device 可首次持久化，成功为 `201`；target 不存在或存在但 state 不是 `enrolled` 都返回相同四字段 body 的 `404` / `RESOURCE_NOT_FOUND`；same-ID/same-request replay 为 `200`；非 canonical UUIDv7 为 `400` / `COMMAND_ID_INVALID`；same-ID/different-request 为 `409` / `COMMAND_REQUEST_CONFLICT`。Device 在创建后被 disable/revoke 不改变 replay/conflict 分类；这两个分支先于首次持久化资格检查并忽略当前 state。这些 outcome 只表示 Server 已记下意图，不表示 Device 已执行。
- **串行、durable order 与 cut**：Server DeviceActor 与 Client command executor 各有单消费者有界 channel。所有首次创建（离线 Device 也一样）经 actor 分配该 Device 内唯一、严格递增的 `enqueue_order` 并与 Command/audit 原子提交；该值限 `1..=i64::MAX`、不进 wire、不要求连续，exact replay/conflict 不消费。actor 重建先收敛唯一 `in_flight`，否则按 `enqueue_order` 取最早 queued；到上界 fail closed。Server 同时最多一条 `in_flight` Command，并必须在任何 socket write 前 durable 提交 `queued → in_flight`；转换失败时零 wire 副作用。Client 完成本地副作用并返回 terminal status 后才开始下一条。WS I/O、ping/pong、Observed 与 disconnect 检测可并发。
- **Converge vs Oneshot**：Converge（`sync_state` / `sync_secret` / `reset_home`）按领域键幂等，只有同一非终态 operator 意图可在 drift 时重投同一 ID/payload。Oneshot（`lock_session` / `unlock_session` / `terminate_session` / `open_binding_prompt`）仅 current live socket；不转投 replacement lease，重连不重放。Device **不**维护 command journal。
- **Converge 键**：`sync_state` = Server-derived assignment hash vs Observed `applied_hash`；`sync_secret` = Target `(binding_id,account_id,credential_revision)` vs Observed credential 完整上下文且 `state=INSTALLED`；`reset_home` = Target `home_epoch` vs Observed 可选 `completed_home_epoch`（缺失/较小未收敛，相等收敛；超前或观测回退 fail closed；同 epoch 可重入，已完成则为 success/no-op；`HOME_EPOCH_STALE` 仅当命令 epoch < 本地已完成 epoch）。credential revision 与两个 Home epoch 的出现值统一限 `1..=i64::MAX`。
- **Oneshot 目标与不确定结果**：命令作用于开始执行时捕获的 current graphical session，不携带 wire session target。创建时无通过 initial Observed barrier 的 current lease，或仍 `queued` 时所属 lease 断开/被替换/Server 重启，确定终止为 `failed/COMMAND_NOT_DELIVERED`。进入 `in_flight` 后在 terminal status 前发生 send failure、disconnect 或 restart 则 terminal `outcome_unknown`。两者都不自动重放；wire `CommandState` 仍只有 `SUCCEEDED` / `FAILED`。
- **`OPEN_BINDING_PROMPT`**：空 body，无 TTL，无 `prompt_message_id`。Device 打开 screen 即报 `SUCCEEDED`。现场提交是独立 `BindingRequest{seat_code}` → `BindingResult{state,error_code}`；每个 Active session 最多一个 in-flight BindingRequest，不携 request ID 或 occupancy `binding_id`。
- **binding/account/revision**：Device 在 secret 写入前检查当前 assignment 的 `(binding_id,account_id)` 与 Command 匹配，并校验 revision。任一不同都稳定拒绝，不部分应用。
- **恢复**：Converge `in_flight` 在新连接 initial Observed 后，匹配 frozen convergence key 则推定 `succeeded`，仍 drift 则退回 `queued` 并重投，Observed ahead/regression/无法比较则 fail closed；本地可重入步骤（Home 用状态文件 + `RecoverHomeInstance`）不靠 Command receipt journal。已收到的 terminal status 不被后来 drift 或 transport error 覆盖；重新执行需新 Command。
- **terminal lifecycle**：disable/revoke 的 guarded transaction 在 post-commit eviction 前将旧 Device 全部 queued（Converge/Oneshot）收口为 `failed/COMMAND_NOT_DELIVERED`、全部 in-flight 收口为 `outcome_unknown`，既有终态不变。status 先提交则保留，lifecycle 先提交则 late old-lease status 无 authority。终态 row 与 HTTP replay/conflict 保留；恢复/新 Device 不复活旧意图。
- **Status 配对**：Server 只接受 current lease 上与 current `in_flight.command_id` 匹配的 `CommandStatus`。已终态的 exact same status 为零写入 no-op，冲突终态或非 current ID 为 protocol violation。每次 delivery attempt 最多一个 terminal status；Converge 恢复可以同一 ID 开始新 attempt。
- **bulk**：每个 target 是独立 Command；可选 `group_correlation_id` 仅支持查询和审计分组，不定义跨 Device 顺序、原子性、retry 或 lifecycle。

## 3. `SYNC_STATE` 的安全 outcome

`SYNC_STATE` 必须由操作员显式触发（不自动）。激活失败的 fail-closed 规则：

- **任何中途失败必须保留已验证 LKG 或进入 BLOCKED，不暴露未验证配置；**
- Target 陈旧时拒绝，不修改本地状态；
- 证书/私钥验证失败不激活；`caddy validate` 失败不 reload；reload 失败时回滚 LKG 配置文件并确认旧配置仍有效，否则 BLOCKED；
- upstream 不健康或 `/login` 非 TLS 时按冻结 policy 保持 BLOCKED 或 READY-with-health，不自由猜测；
- Observed 上传失败不回滚本地已成功原子动作，重连重报 slim snapshot。

Wire assignment 是 `oneof {unbound,bound}`；`bound` 完整携带 `binding_id`、`account_id`、`seat_code`、`domjudge_username`。`canonical_hash` 是双端从 validated assignment 派生的 SHA-256，不在 `SyncState` 中传输。Account mapping 变化时，应用新 assignment 必须先使旧 credential context 失效并保持 BLOCKED，不能把新 username 与旧 password 组合。

`SYNC_STATE` 不签发、不携带、不安装任何证书或 token（`INV-CERT-01`）。具体阶段序列在 Phase 5 实现时定义。

## 4. Gateway readiness

Gateway certificate 在人工批准的 Enrollment transaction 中获得（[ADR-0038](adr/0038-unified-ordinary-wss-device-control-authority.md)），但 **Enrollment 成功不得被展示为数据面 ready**：READY 还需要 Target 应用、配置渲染、validate、reload 与健康检查全部通过。

## 5. `SYNC_SECRET` 的安全 outcome

`SYNC_SECRET` 必须：

- 只能由人类明确触发，**不能由 Target drift 自动触发；**
- wire 使用 `SyncSecret{binding_id,account_id,credential_revision,SecretBytes password}`；revision 必须在 `1..=i64::MAX`，Device 写入前重新校验当前 assignment 的 Binding 与 Account 都匹配，并校验 credential revision；
- 凭据文件与完整 credential context 原子、crash-safe 更新，失败时保留旧 secret 或明确标记不可用，**不留半写**；只有 durable 后才能发送 `CommandStatus(SUCCEEDED)` 或 Observed `INSTALLED`，否则发送正常 FAILED；本地 metadata 损坏到不能安全构造 status/Observed 时保持 BLOCKED 并以 `ClientClose` 终止连接，不伪造 `ABSENT`；
- 成功后重渲染 Caddy `/login` 注入配置并原子激活（[ADR-0034](adr/0034-state-execution-and-data-plane-boundary.md)）；
- 成功后 Observed 报告完整 credential context 与 `state=INSTALLED`；仅当 binding、Account、revision 都相同时重推才是 no-op；
- 结果 redacted，不向普通 surface 暴露 secret。

具体阶段序列在 Phase 5 实现时定义。

## 6. Caddy 状态

Caddy 业务状态只需 `BLOCKED` / `READY`。

- **BLOCKED**：主页面 HTTP 503；只显示 allowlist 状态；静态本地资源；严格 CSP；动态值只通过 `textContent`；**不显示 password、路径、自由格式错误或 `session_locked`；不代理 DOMjudge。**
- **READY**：需证明当前 Target/revision、Gateway certificate 与 private key 匹配、SAN/有效期、`caddy validate` 通过、fixed TLS upstream policy、reload 成功、本地健康检查、LKG 写入成功或可恢复。

**Session lock/unlock/terminate 不触碰 Caddy 配置、不改变 Caddy 状态，也不将 `session_locked` 放入状态页。**

## 7. Session 与 Home

- **Session**：WSS Oneshot 在开始时捕获 current graphical session，特权副作用前重检；replacement 发生在执行中则 `SESSION_CONTEXT_STALE`，不改投新 session。Wire 仍无 `SessionTarget` / `session_epoch`。Agent lease 拒绝陈旧 UI action，Session 动作不改变 Caddy。一台 Device 一名选手、autologin；选手不知道 unlock 密码。
- **Home**：每次 `HOME_RESET` 由 Server 分配 `1..=i64::MAX` 的 `home_epoch`；Observed 可选 `completed_home_epoch` 缺失表示从未成功完成 reset，出现时同样限 `1..=i64::MAX`，且只在 Prepare/Activate/Recover/GC 与 completion record 全部 crash-safe 持久化后单调前进；对应 `CommandStatus(SUCCEEDED)` 也只能在该写入之后。较新 reset 执行/恢复期间保持上一完成值。同 epoch 的步骤必须可重入，已完成则为 success/no-op；`HOME_EPOCH_STALE` 仅当命令 epoch < 本地已完成 epoch；重试不得 bump epoch。达到上界时 fail closed，不得 wrap。曾完成但完成记录缺失/损坏时 fail closed，不报告正常 absent。`HOME_RESET` 不拆 daemon WSS。reset 完成前不启动受管 session；中断的 reset 经本地状态文件 + `RecoverHomeInstance` 恢复，不靠 Command durability；**无法证明 mount/copy/ownership 安全时 fail closed；不静默切换 backend。** 重置仍是操作员在场的受控事件。本地分解只属实现面，D-Bus surface 保持不变。

## 8. 可观测性

Server 与 Device 指标追踪连接、Observed freshness、Drift、enrollment/签发结果与 stable ErrorCode。`group_correlation_id` 只能作为有限的查询/审计分组字段，不承载 workflow status。**指标 label 不得包含密码、token 值、路径、certificate body、Machine ID 全值或自由格式错误。**

## 9. 测试模型

必须覆盖的安全 fault class：Panel canonical UUIDv7 正/反例、`PUT` 首次 `201` / replay `200` / invalid `400` / conflict `409`、same-ID fingerprint、`enqueue_order` 分配/唯一性/重启恢复/上界、`queued → in_flight` pre-send durable cut、Server/Client channel 顺序与每 Device 单 in-flight、status-current-lease/ID 配对、Converge initial-Observed 推定成功/退队重投/fail-closed、Oneshot `COMMAND_NOT_DELIVERED` / `outcome_unknown` / 不重放、disable/revoke terminalization 与 late status、assignment hash golden、陈旧 `(binding_id,account_id,credential_revision)`、Account mapping 替换不混用旧密码、revision/epoch zero/上界/overflow、secret/Home write-before-success、`completed_home_epoch` absent/monotonic/in-progress/recovery/ahead/regression 与 stale、initial Observed barrier、Gateway state/hash 独立组合与 Caddy leaf DER SHA-256/validate/reload 中断、secret redaction/写入中断、session replacement 重检与 Home recovery。具体测试随对应 Phase 实现补全。
