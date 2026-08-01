# Natsume V2 状态与执行模型

> 状态：`NORMATIVE`  
> 适用范围：Target、Observed、Drift、Command、Caddy、Session 和 Home 的安全 outcome  
> 相关不变量：`INV-STATE-01`、`INV-SECRET-02`、`INV-COMMAND-01`、`INV-DATAPLANE-01`、`INV-DATAPLANE-02`、`INV-SESSION-01`

## 1. 状态与副作用分离

Natsume 同时处理：已提交事实（Server truth）、期望状态（Target）、实际状态（Observed）、纯差异（Drift）、人工意图（Command）和本地原子激活（Caddy/Home）。这些概念压缩成一个"device status"会产生高耦合：普通 CRUD 依赖网络、重试改变业务事实、UI 文案变成状态机输入。本文件冻结这些层次的安全 outcome；具体状态机、字段与事务编排延迟到对应 Phase 实现。

- **Server truth**：已提交领域事实。**提交 Server truth 不意味着 Device 已完成**；import 不创建远端副作用。
- **Target**：从 Server truth 派生的非秘密期望。**不含明文密码，确定性派生，不自动联系 Device。**
- **Observed**：Device 的 typed 实际状态报告。**只接受认证、有界、typed 的 observation；Device 自报属性不构成授权。**
- **Drift**：`compare(Target, latest valid Observed)` 的纯比较结果，可重算。
- **Command**：单 Device durable intent；批量操作 = 批量 Command + 查询聚合；投递/重试观察是 Command 元数据（[ADR-0029](adr/0029-right-sizing-control-plane-machinery.md)）。

## 2. Command 幂等与冲突

- **相同 `command_id`**：payload 相同返回既有状态/结果；payload 不同为冲突；已成功不重复副作用；正在运行不并发执行第二次；已失败按策略返回终态，不偷偷重启。需要重新执行时创建新 Command ID。
- **revision/epoch**：Device 在执行前和关键原子提交前检查 assignment/configuration/credential/session/home 各代；**陈旧时用稳定错误拒绝，不"尽量兼容"地部分应用。**
- **Command receipt 在 Device durable 持久化前不得确认**；进程崩溃后相同 Command 能恢复或返回原结果。终态不可被后来的 transport error 覆盖；重复终态消息幂等合并。

具体 Command 状态机与 dispatcher/journal 流程在 Phase 4 实现时定义。

## 3. SYNC_STATE 的安全 outcome

`SYNC_STATE` 必须由操作员显式触发（不自动）。激活失败的 fail-closed 规则：

- **任何中途失败必须保留已验证 LKG 或进入 BLOCKED，不暴露未验证配置；**
- Target 陈旧时拒绝，不修改本地状态；
- 证书/私钥验证失败不激活；`caddy validate` 失败不 reload；reload 失败时回滚 LKG 配置文件并确认旧配置仍有效，否则 BLOCKED；
- upstream 不健康或 `/login` 非 TLS 时按冻结 policy 保持 BLOCKED 或 READY-with-health，不自由猜测；
- Observed 上传失败不回滚本地已成功原子动作，重连重报。

`SYNC_STATE` 不签发、不携带、不安装任何证书或 token（`INV-CERT-01`）。具体阶段序列在 Phase 5 实现时定义。

## 4. Gateway readiness

Device Token 与 Gateway certificate 都在 Enrollment 获得（[ADR-0021](adr/0021-provisioning-window-certificate-issuance.md)），但 **Enrollment 成功不得被展示为数据面 ready**：READY 还需要 Target 应用、配置渲染、validate、reload 与健康检查全部通过。证书持有与数据面状态是两个独立维度。

## 5. SYNC_SECRET 的安全 outcome

`SYNC_SECRET` 必须：

- 只能由人类明确触发，**不能由 Target drift 自动触发；**
- Command 创建时冻结 assignment/credential revision，Device 写入前重新校验；
- 凭据文件更新原子，失败时保留旧 secret 或明确标记不可用，**不留半写**；
- 成功后重渲染 Caddy `/login` 注入配置并原子激活（[ADR-0024](adr/0024-domjudge-autologin-via-xheaders.md)）；
- 成功后 Observed 只报告 revision；retry 使用相同 Command ID，不重复不可逆动作；
- 结果 redacted，不向普通 surface 暴露 secret。

具体阶段序列在 Phase 5 实现时定义。

## 6. Caddy 状态

Caddy 业务状态只需 `BLOCKED` / `READY`。

- **BLOCKED**：主页面 HTTP 503；只显示 allowlist 状态；静态本地资源；严格 CSP；动态值只通过 `textContent`；**不显示 password、路径、自由格式错误或 `session_locked`；不代理 DOMjudge。**
- **READY**：需证明当前 Target/revision、Gateway certificate 与 private key 匹配、SAN/有效期、`caddy validate` 通过、fixed TLS upstream policy、reload 成功、本地健康检查、LKG 写入成功或可恢复。

**Session lock/unlock/terminate 不触碰 Caddy 配置、不改变 Caddy 状态，也不将 `session_locked` 放入状态页。**

## 7. Session 与 Home

- **Session**：每个 transition 绑定当前 `SessionEpoch`；Agent 通过 lease 证明属于当前 logind session。陈旧 Agent 或 UI action 被拒绝；Agent 崩溃后 lease 过期，不解锁额外权限，不改变 Caddy。锁定语义走当期镜像桌面的原生 session lock；遮罩类 UI 是呈现层，不是完整性边界（[ADR-0027](adr/0027-single-image-desktop-cycle.md)）。
- **Home**：开始时创建新 `HomeEpoch`；prepare 完成前不启动受管 session；cleanup 只作用于当前 epoch；**无法证明 mount/copy/ownership 安全时 fail closed；不静默切换 backend。** 重置为操作员在场的受控事件，实现为状态文件 + 幂等可重跑步骤。

## 8. 可观测性

Server 与 Device 指标追踪连接、Command 队列/延迟/重试、Observed freshness、Drift、enrollment/签发结果与 stable ErrorCode。**指标 label 不得包含密码、token 值、路径、certificate body、Machine ID 全值或自由格式错误。**

## 9. 测试模型

必须覆盖的安全 fault class：Server 事务成功但 Device 离线、receipt 前后断线、执行中崩溃、重复 Command、相同 ID 不同 payload、陈旧 revision/epoch、窗口关闭签发拒绝、重复 Enrollment 替换语义、无 token upgrade 拒绝、WSS 断线重连收敛、Caddy validate/reload 中断、old LKG 保留、secret 写入中断、Observed 丢失重发、Agent crash/focus denied/display lost、Home prepare/cleanup 中断、cancel 与 terminal status race。具体测试场景随对应 Phase 实现补全。
