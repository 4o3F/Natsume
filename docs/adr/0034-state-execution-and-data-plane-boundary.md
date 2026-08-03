# ADR-0034: State, execution, and data-plane boundary

> Status: `ACCEPTED`
> Scope: Target/Observed/Drift, direct Command identity and execution, Caddy data plane, and DOMjudge access
> Consolidates: ADR-0008, ADR-0013, ADR-0014, ADR-0024, ADR-0029
> Supersedes: consolidated historical records; see [`history-map.md`](history-map.md)
> Superseded by: —

## Context

Committed Server truth、请求动作和 Device 实际状态是不同事实。把 import、binding、Target 或 Command success 当作 Device reality，会掩盖离线执行失败、restart 和本地回退；让配置变更自动触发 Device action 又会把 domain transaction、remote availability 与 secret distribution 耦合。

数据面在配置、证书、reload 或 upstream 不安全时仍需提供本机可诊断的 fail-closed 页面，并为不知道凭据的选手完成 DOMjudge auto-login。DHCP 排除了 source-IP authentication，X-Headers 是必须由目标环境验证的外部 contract。

Command 是单 Device 的明确意图，不是跨设备 workflow。浏览器重试必须复用 Panel 生成的 ID 与同一 request fingerprint；另行生成 Server-side ID 或把 delivery 观察建成独立业务 aggregate，会混淆操作员意图、持久化命令和 Device side effect。

## Decision

### State layers

- Server truth 是已提交领域事实；其变更不证明 Device 已改变。
- Target 是从 Server truth 与 frozen policy 确定派生的 non-secret expectation，不包含 password、private key、token、任意 path/UID/unit/upstream/Caddy fragment；Target 变更不联系 Device。
- Device-facing non-secret generation 是从 current Server truth 派生的 contract artifact，不建立独立 mutable counter 或通用 version system；`observed_device_states` 只保存 Device 报告的 `received_generation`、`applied_generation`，以及可选 `gateway_configuration_revision`。
- Observed 是最新有效 typed Device snapshot，也是 Device 实际业务状态的唯一来源；stale/unavailable 表示 unknown，不推断为 READY。
- Drift 是 Target 与最新有效 Observed 的纯比较，不是独立 truth。
- Command receipt、retry 或 terminal success 只描述该 Command，不能替代 Observed。

### Direct durable Command 与 Panel-owned identity

- 只有授权 operator 可创建 `SYNC_STATE` 或 `SYNC_SECRET`；import、binding、credential revision 或 Drift 不得隐式创建 Command。
- **Panel 是 `command_id` 的唯一生成者。** 每次创建前，Panel 生成 canonical lowercase hyphenated UUIDv7；Server、WSS 与 Device 不为同一请求重写、补发或替换该 ID。
- 创建入口固定为 `PUT /api/v2/commands/{command_id}`。持久化的 `commands` row 使用 `device_pk`、`kind`、`state`、`request_fingerprint_version`、`request_fingerprint_sha256`、可选 `group_correlation_id`、`payload_version`、`frozen_payload_json`、`created_at`、`deadline_at`、可选 terminal fields 和 `created_audit_event_id`；请求中的 `reason_code` 或 typed input 只在其被该 typed frozen payload 接受时保存，不另设顶层列。
- Server 对 canonical request 计算 versioned、domain-separated SHA-256 fingerprint，并保存为 `request_fingerprint_version` 与 `request_fingerprint_sha256`。它覆盖 target Device、Command kind、reason、可选 group correlation 和 typed client input；不覆盖重试时间、actor 或 Server 后续派生状态。相同 ID 且相同 fingerprint 返回既有 `Command`，不重复 audit 或 side effect；相同 ID 且 fingerprint 不同返回稳定 conflict。
- 首次持久化返回 `201`；相同 canonical request 的 replay 返回 `200`；非 canonical UUIDv7 返回 `400` / `COMMAND_ID_INVALID`；同 ID 不同 request 返回 `409` / `COMMAND_REQUEST_CONFLICT`。这些 response 只证明 Command 已持久化，不证明 Device 已执行。
- `Command.command_id`、WSS `Command`、Device durable journal、WSS `CommandStatus` 与每 Command 的 audit correlation 必须保留完全相同的 ID 字符串。Device 不得将重复投递变成第二个业务执行；同 ID 而不相同的 `frozen_payload_json` 内容必须 conflict/reject。
- `SYNC_STATE` 与 `SYNC_SECRET` 都是 direct single-Device durable Command。`SYNC_STATE` 只应用 non-secret Target，绝不签发、携带或安装 certificate/token。每个 Command 的 frozen content 由带 `payload_version` 的 typed `frozen_payload_json` 保存；row 不含独立的 frozen Seat、BindingRevision、credential revision 或 dispatcher-metadata 列。Device 写入前重检适用的当前事实，并只返回 redacted result 与 installed revision。
- ack 前必须 durable receipt；completed 不重复 side effect，terminal failure 不自动重启，新执行使用新 ID。revision/epoch failure 在部分应用前拒绝。
- bulk action 生成 N 个独立 Command ID。它们可以共享可选 `group_correlation_id`，但该值仅用于查询和审计分组：不表示顺序、原子性、重试策略或跨 Device lifecycle。
- 本 ADR 冻结 identity、HTTP、WSS 和 journal 语义；Phase 0 的声明不表示已经实现 HTTP listener、authorization、Command repository、dispatcher、WSS journal 或 Panel mutation。

### Fail-closed Caddy 与 DOMjudge

- Caddy 只有 `BLOCKED` 与 `READY` 两个业务状态。`BLOCKED` 使用 package-contained local resource 返回 503，不代理 DOMjudge，只显示 allowlisted typed state。
- BLOCKED 页面使用 strict CSP、本地静态资源与 `textContent`；不得暴露 secret、path、free-form error、source chain 或 `session_locked`。
- Device Daemon 从已验证本地材料渲染完整固定配置，validate、atomic activate、reload、local health check，并在失败时恢复 LKG；不确定状态保持 verified LKG 或进入 BLOCKED。
- READY 需要 current Target/revision、匹配且有效的 Gateway key/certificate/SAN、validate/reload/health 成功、固定 TLS upstream policy 与可恢复 LKG。
- Caddy 只在 `/login` 注入 `X-DOMjudge-Login` 和 base64 `X-DOMjudge-Pass`；其他 route 不注入。upstream 必须使用 TLS。
- credential source 为 `0600 root:root`，rendered Caddy secret artifact 为 `0640 root:natsume-gateway`；两者不得进入 Target、Observed、audit diff、metrics、普通日志或 Session Agent。
- 保持 `Accept-Encoding` transparent，不新增本地 `encode`；upstream brotli 是 deployment contract。
- session lock/unlock/terminate 不修改 Caddy state 或配置。

### Right-sized control plane

当前 schema 不声明独立 configuration counter、通用 version system、dispatch statistics、flexible RBAC 或 Caddy Admin API。`commands` 只持久化 migration 定义的 current row；需要跨 Device workflow、外部事件消费者或独立 failure semantics 时必须先出现真实消费者并通过新 ADR。

## Alternatives

- automatic reconciliation 或保存即同步：掩盖 operator intent，并可能隐式分发 secret。
- password 放入 Target/`SYNC_STATE`：扩大 secret surface，绕过 `SYNC_SECRET` 授权与 revision 检查。
- 用 Target、Command success 或 event delta 表示状态：无法表达 local failure、restart、freshness 与 recovery。
- Server 生成 command ID 或使用 `POST` action endpoint：无法把 Panel 的一次意图稳定地贯穿 HTTP、WSS、journal 与 audit。
- bulk workflow record：为当前没有消费者的跨 Device 生命周期、状态和恢复语义建模。
- browser failure、Server-hosted/free-form error page：无法在本地 outage 中安全诊断。
- DOMjudge IP auth、custom Caddy module、form broker 或未验证 Basic auth：DHCP 或额外协议/供应链成本不可接受。
- 现在增加事件投递机制或 Admin API：当前规模没有消费者，却增加持久状态与失败语义。

## Consequences

### Positive

- Panel 的一次创建意图、持久化 Command、WSS message、Device journal 和 audit 使用同一个可验证 ID。
- same-ID replay 不会重复敏感副作用；same-ID/different-request 有稳定而窄的冲突语义。
- operator intent、secret handling、audit、retry 和 Device reality 分别可观察。
- crash/offline recovery 通过 direct durable Command 与 Observed 收敛。
- Caddy activation 收敛为 validate、atomic activate、reload、health 与 LKG 的有限失败模型。
- DOMjudge credential consumer path 可枚举且窄。

### Negative / trade-offs

- Panel 必须在网络请求前正确生成并持久保留 canonical UUIDv7，不能把重试交给 Server 的隐式 ID 策略。
- bulk 视图只能查询聚合；没有跨 Device rollback、进度 lifecycle 或跨 Device retry。
- operator 必须显式处理 Drift 和 state/secret 两类同步。
- Observed 需要持久化、freshness 与 unknown UI。
- rendered Caddy 配置含可逆 credential material，严格依赖权限、redaction 与 upstream TLS。
- DOMjudge version、X-Headers、TLS trust 与 brotli 是外部部署依赖。

## Acceptance basis and revisit trigger

证据必须覆盖无 implicit Command、canonical UUIDv7 正/反例、`PUT /api/v2/commands/{command_id}` 的 `201/200/400/409` 契约、same-ID fingerprint replay/conflict、`request_fingerprint_*` 与 `frozen_payload_json` 的持久化规则、Command ID 在 HTTP/WSS/journal/status/audit 的一致性、bulk group 只作查询/审计分组、Command durability/crash/stale revision、Observed freshness/re-report、audit atomicity、Caddy certificate/validate/reload/health/LKG/BLOCKED/CSP/no-secret，以及 DOMjudge X-Headers route scope、upstream TLS 和 brotli passthrough。

出现真实外部 event consumer、cross-Device staged workflow、dynamic Caddy consumer、DOMjudge contract 变化或选手需要拥有 credential 时，用新 ADR 重开相应子边界。

## Normative sources

- [Architecture](../architecture.md)
- [Domain model](../domain-model.md)
- [Contracts](../contracts.md)
- [State and execution](../state-and-execution.md)
- [Security and recovery](../security-recovery.md)
- [Dependency policy](../dependency-policy.md)
- [Supported platform](../supported-platform.md)
