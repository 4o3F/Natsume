# ADR-0033: Enrollment and Device control boundary

> Status: `ACCEPTED`
> Scope: provisioning-window Enrollment, Gateway certificate authority, Device Token lifecycle, and WSS Device control
> Consolidates: ADR-0012, ADR-0016, ADR-0021, ADR-0023
> Supersedes: consolidated historical records; see [`history-map.md`](history-map.md)
> Superseded by: —

## Context

Device 在 Enrollment 前没有 control credential；进入选手使用期后，本地工作站不再可信。因此初始授权必须限制在短暂、物理受控、可审计的 provisioning window，运行期控制面只接受已认证 Device。

部署是单站点、带宽受限 LAN，UDP 通过性不确定，Server 单实例且需要双向 control semantics。证书授权和 Device control authentication 必须是分离、可测试的小边界，不能让 Client 选择 SAN/profile，也不能永久开放 issuance surface。

provisioning window 的当前开关和它的审计证据是不同事实。保留 revision ledger 会把当前状态误建模为可恢复的业务历史；本设计只需要一个当前 singleton，以及不可替代的 redacted audit lineage。

## Decision

### TLS 与 provisioning window

- Operator HTTP、Enrollment 与 Device control 共享一个 Server TCP port，但 route、authentication、authorization 和 rate limit 分离。
- 所有 Device-facing 入口使用 Server-authenticated TLS；Client 校验预配置 trust 与 configured IP-SAN/endpoint，禁止 TOFU 和 dangerous verifier。
- provisioning window 默认关闭。当前状态由 `provisioning_window` singleton 表示，只有 `state`（`closed`/`open`）、单调 `revision` 和 nullable `last_audit_event_id`；没有 `changed_at`、provisioning revision ledger、通用 instance state 或历史状态表。
- 正常 open/close 是显式、持久化、受审计的 operator action。guarded operation 接收 fresh `audit_event_id` 作为 typed input，在同一 transaction 内自行插入 redacted `AuditEvent`、以预期 `state + revision` CAS 更新 singleton，并更新 `last_audit_event_id`；已持久化的同 ID 或预插入 audit row 因唯一约束失败，不能重放为新变更的依据。任何 audit、CAS 或 commit 失败都不得留下半个窗口变更。
- restart、recovery 或 backup restore 不得自动打开窗口。恢复时已 `closed` 的 singleton 零写入、零 recovery audit；只有观察到 `open` 时，Server 才在一个事务内写入 `system:recovery` 的 redacted audit 并 CAS 关闭、`revision + 1`。成功后所有后续恢复都看到 `closed`，因此只关闭并审计一次。
- 只有窗口内 Enrollment 可以签发 Device Token 或 Gateway certificate；关闭时请求返回稳定错误且 Server state 零变更。
- issuance transaction 关联 `devices`、`enrollment_requests`、`device_tokens`、`gateway_certificates` 与 `audit_events`：Enrollment request 记录硬件 ID/质量、CSR DER、SPKI hash、client/protocol/source-IP、`state`（`pending`/`approved`/`rejected`/`issued`/`expired`/`conflict`）、可选 `resolution`（`create_device`/`replace_device_credentials`）、resolved Device 和 issuance audit；`issued` request 必须同时具有 resolution、resolved Device 和 issuance audit，且 issuance audit 只可用于 `issued`；同一 hardware-ID/SPKI 最多一个 live `pending`/`approved` request。Device Token row 以 `device_pk` 为键并关联唯一 Enrollment request；certificate row 也关联唯一 Enrollment request。失败不得留下部分 issuance。
- 同一 `machine_hardware_id` 在窗口内重复 Enrollment 使用 `enrollment_requests.resolution = 'replace_device_credentials'` 的受审计替换路径：`device_tokens.token_hash` 被替换以使旧 bearer credential 失效，并签发新的 Gateway certificate metadata；仍存活的旧连接记录为 anomaly。

### Gateway certificate authority

- Gateway private key 在 Device 本地生成且不离开 Device。
- PKI scope 仅为 `control CA → Server TLS leaf` 与 `origin CA → Device Gateway leaf`；Gateway profile 是唯一 Device-side certificate profile。
- hostname、SAN、profile、EKU 与 validity 均由 Server site policy 决定；CSR 中的 CN/SAN/profile 只证明 possession 和结构，不授予 authority。
- Gateway hostname 是 installation/preseed 输入，使用与 Server endpoint 相同的 canonical parser/validator，不来自 Target。
- validity 必须覆盖 provisioning-window start 至 contest end 加明确 margin，并在赛事前检查。
- `gateway_certificates` 只记录 `certificate_id`、`device_pk`、`enrollment_request_id`、`serial`、`spki_sha256`、`not_after` 与 `status`（`active`/`revoked`/`expired`/`retired`）；它不保存 certificate bytes，并以每 Device 最多一张 `active` row 约束当前证书，不承诺 browser-consumed revocation distribution。
- Client 在持久化前验证 private-key/SPKI、origin-CA chain 与 configured-hostname SAN；finalization 失败不得形成“看似已 Enrollment”的本地状态。

### WSS Device control

- Device control 使用 Server-authenticated TLS 上的 WebSocket；每个 Protobuf message 对应一个 binary frame，版本通过 `Sec-WebSocket-Protocol` 协商。
- TLS early data 关闭；frame、connection、version 与 envelope limit 由 contract 强制。
- Device Token 是 Server 使用 CSPRNG 生成的 opaque 32-byte bearer credential；`device_tokens` 只保存 `device_pk`、唯一 `enrollment_request_id` 与 32-byte `token_hash`，Client 用 `Authorization: Bearer` 提交。
- missing、wrong 或不再有对应 `device_tokens` row 的 Token 在 Protobuf decode 前返回 `401`，hash compare 为 constant-time，失败认证受 rate limit。
- `device_tokens` 没有 TTL、issued-at 或 plaintext token 列；Token 只通过 operator revocation、audited re-enrollment replacement 或 single-lifetime reset 失效。
- keep-alive 使用 WebSocket ping/pong；断线不改变 Server truth 或 Command truth，重连通过 direct-Command durable contract 收敛。

## Alternatives

- Enrollment pre-shared token：引入 Device 尚未受管前的分发、泄露与恢复通道。
- TOFU、permissive verifier 或仅 Server-auth transport：无法同时抵抗 LAN impersonation 并建立 Device authority。
- provisioning revision ledger、append-only window history 或 generic instance state：把当前安全开关扩展成没有消费者的状态模型；审计已提供必要证据。
- 恢复时直接重开、每次启动重复 close/audit，或在 audit/CAS 失败后继续：分别扩大签发面、制造重复证据，或破坏安全状态和审计的原子性。
- generic certificate/artifact endpoint 或 Client-selected SAN/profile：扩大 signing surface 并把授权交给不可信请求。
- baked/shared Gateway key、self-signed browser trust 或运行期 issuance：破坏每机 key isolation、安装期 hostname 与窗口边界。
- QUIC/client mTLS Device identity、JWT 或额外 control stack：当前部署无消费者，增加 PKI、refresh、firewall 和运维面。
- 证书在 `SYNC_STATE` 签发：把 issuance authority 延伸到不可信运行期，已明确拒绝。

## Consequences

### Positive

- issuance surface 在时间和 profile 上都关闭；Gateway key 留在 Device，授权属性留在 Server。
- 当前窗口状态保持窄；审计提供 open/close/recovery 的证据，而不把旧开窗状态重新当作权威事实。
- close-once recovery 在重复启动和 restore 中只执行一次，不伪造额外 operator action。
- 单 TCP port 与 WSS 降低 firewall 和 protocol surface；匿名输入在 decode 前拒绝。

### Negative / trade-offs

- provisioning window 是敏感持久状态，需要严格操作与恢复纪律。
- Device Token 是 bearer credential，安全依赖正确 TLS 验证和 root-owned local storage。
- 窗口关闭后不进行常规 Token/leaf rotation，validity 必须赛前正确选择。
- 当前边界不适用于 cross-internet、multi-site 或第三方 Device identity verification。

## Acceptance basis and revisit trigger

证据必须覆盖正确/错误 Server trust 与 IP-SAN、closed-window 零变更、无窗口外 issuance route、正常 open/close 的 audit+CAS 原子性、open 状态 restart/restore 的 close-once recovery、closed 状态 restart 的零写入、Server transaction 与 Client finalization 原子性、CSR authority rejection、re-enrollment replacement、certificate preflight、pre-decode `401`、protocol limit 和断线收敛。

出现 per-Device hostname、多 venue/Internet、失去物理 provisioning window、第三方 Device identity verifier，或 Server trust/network assumption 变化时，先修订 ADR-0030，再用新 ADR 替代本边界。

## Normative sources

- [Architecture](../architecture.md)
- [Domain model](../domain-model.md)
- [Contracts](../contracts.md)
- [Security and recovery](../security-recovery.md)
- [State and execution](../state-and-execution.md)
- [Dependency policy](../dependency-policy.md)
- [Supported platform](../supported-platform.md)
