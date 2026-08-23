# ADR-0038: Unified ordinary-WSS Device control authority

> Status: `ACCEPTED`
> Implementation: `FOUNDATIONS PRESENT — AUTHORITY PENDING ATOMIC CUTOVER`
> Scope: Device control identity, unified Enrollment/control WSS, dynamic authority actors, credential activation, and destructive pre-release cutover
> Consolidates: —
> Supersedes: [ADR-0033](0033-enrollment-and-device-control-boundary.md)
> Superseded by: —

## Implementation boundary

本 ADR 于 2026-08-19 接受为预发布 flag-day authority 目标。Batch 1 已按 owner 的预发布 BC 决策原位拆分 production Proto、把单一 package 改为 `natsume.device.control`、把当前双端 subprotocol 同步改为 `natsume.control`，并加入 dormant Challenge/Proof/Bundle/Ack/Activated/Ready/SessionReady 与 crypto/schema foundations；不存在 `control_v2`、第二 descriptor 或旧/新 package 兼容层。无 `ClientInit`、无 `ControlEnvelope`、无 Hello。

这些 foundation 不授予 control-key authority。当前网络认证仍是 HTTPS Enrollment、Device Token 与 Bearer-before-101；wire 已是定向 handshake/Active envelopes，尚无 runtime key-auth consumer。Atomic flag day 才删除 Token/public Enrollment HTTP、启用 PreAuth/actor/Ack authority 并收紧 transitional schema。任何中间版本都不得同时接受 Token 与 control key 作为 authority。

**2026-08-20 修订（持久化时刻）**：`credential_bundles.activation_deadline_unix_ms` / `created_at_unix_ms` 与 `enrollment_requests.created_at_unix_ms` / `activation_deadline_unix_ms` 均为 INTEGER UTC epoch milliseconds。无 RFC 3339 TEXT。`device_id`、`binding_id` UUID occupancy、vault `account_id` PK、无 `revision_counters`、无 `import_payload` 均保持。

**2026-08-20 修订（无 ControlKeyId）**：Server natural key 是 `public_key`；不派生 ControlKeyId。daemon manifest pins hex(public_key)。Enrollment/WSS Token 路径仍按本 ADR 原子 flag day 重写，本记录仍是目标。

## Context

当前设计把公开 HTTPS Enrollment 与 Bearer WSS 控制分为两条 Device 路径，credential issuance、reconnect、replacement、lifecycle eviction 与 command dispatch 因而跨越多个 admission 与 runtime owner。把 persisted Device 在启动时预建为 runtime slot 仍会保留该分裂，并把启动枚举变成 authority 前提。

部署仍是单 Server 进程、单 SQLite、约 500 台 Device、无 HA、物理受控 provisioning period。Machine Hardware ID 可观察，不能认证 Device。Gateway TLS key 服务本地浏览器 origin，不能兼任 control identity。

首次 control key 没有更早的 Device credential 可以背书。窗口内接受未知 hardware 的 first key 是明确的残余信任，只能由容量限制、物理清点、Binding 与 secret release gate 约束，不能被描述为制造商身份或硬件证明。

## Decision

### Dedicated control identity

每台 Device 在完成本地 Machine Hardware ID 验证后、首次网络连接前生成专用 Ed25519 control signing key。Private key 为 daemon-owned `0600`、create-only PKCS#8 DER，不离开 Device。**无 ControlKeyId**：Server natural key 是 `public_key`（32-byte Ed25519）；`device_control_keys.public_key` 是 PK。daemon manifest pins hex(public_key)。Control key 与 Server TLS、Origin CA、Gateway TLS key 完全分离。

算法固定为 Ed25519。增加算法必须修订 domain/version 并增加跨实现 golden。使用维护中的 Ed25519/PKCS#8 Rust 实现；禁止手写曲线运算、签名解析或自定义密码学原语。

### Ordinary-WSS challenge-response

目标继续使用普通 pinned server-auth TLS 1.3 与 RFC 6455 Upgrade，不要求 mTLS、TLS exporter 或认证 header：

```text
TLS 1.3
→ HTTP 101 / exact subprotocol natsume.control
→ ServerHandshakeEnvelope: ServerChallenge
→ ClientHandshakeEnvelope: ClientProof{purpose = EnrollmentAttempt | ResumeSession}
→ (Enrollment) ServerHandshakeEnvelope: CredentialBundle{enrollment_id, gateway_leaf_der}
→ (Enrollment) ClientHandshakeEnvelope: CredentialAck{enrollment_id, bundle_sha256}
→ (Enrollment) ServerHandshakeEnvelope: EnrollmentActivated{enrollment_id, device_id, bundle_sha256}
→ (Enrollment) ClientHandshakeEnvelope: EnrollmentReady{enrollment_id, device_id, bundle_sha256}
→ ServerHandshakeEnvelope: SessionReady{session_id bytes}
→ Active envelopes echo session_id
```

HTTP 101 只建立 transport。Proof 完成前，连接没有 Device、Enrollment、Command、Observed 或 lifecycle authority。

`ClientProof.purpose` 是结构化 `oneof`，不是可被服务端改写的 hint。缺失 purpose 拒绝。冻结的 purpose：

| Purpose | 签名 key | 结构化内容 |
|---|---|---|
| `EnrollmentAttempt` | 候选新 key | 必带 canonical UUIDv7 `enrollment_id`、32-byte `candidate_public_key`、非空 `gateway_csr_der` 与非 `UNSPECIFIED` `evidence_quality` |
| `ResumeSession` | 当前 active key | 空 message；不得携带任何 Enrollment material |

Device 在第一次网络尝试前 crash-safe 持久化 `enrollment_id`、control key、Gateway key 与 exact CSR。处于 Prepared 或 BundleInstalled 的 Device 每次连接都发送完全相同的 `EnrollmentAttempt`；只有本地 Active manifest 已持久化后才发送 `ResumeSession`。Server 不把 Enrollment 改写为 Resume，也不在拒绝后要求 Device 猜测另一 purpose。control-key rotation 不是 handshake。

Server 将 `(machine_hardware_id, enrollment_id, candidate_public_key, gateway_csr_der, evidence_quality)` 视为 immutable Enrollment transaction material；daemon/agent version、connection challenge、signature 与 retry time 不属于该 material。同 `enrollment_id` + exact material 是 replay；同 ID + 不同 material 是稳定 conflict。不同 live ID 对同一 HWID 或候选 key 的占用同样拒绝。一个已经 active 的 transaction 收到其 exact `EnrollmentAttempt` 时仍从 immutable `CredentialBundle` 开始重放；随后 exact Ack 是 no-op，再返回 `EnrollmentActivated`。这保持所有 Enrollment connection 的唯一消息序列，不是把 Enrollment 改写为 Resume。

每条 WSS connection 拥有一次性随机 challenge ID 与 server nonce；它们只存在于该 connection-local PreAuthSession，并在 proof 成功、失败、timeout、非法 message 或 disconnect 后销毁。不得建立全局 challenge lookup 或允许第二次 proof。

Proof crypto 不再逐字段维护第二套 byte schema，也不承担协议语义校验。发送侧 clone typed `ClientProof`、只清空 `signature`，用 pinned Prost `0.14.4` 编码两个完整 typed message，并计算固定摘要：

```text
canonical_input =
  "NATSUME-WSS-CONTROL-PROOF-v2\0"
  || "/api/v2/device/control\0"
  || "natsume.control\0"
  || ServerChallenge.encode_length_delimited_to_vec()
  || ClientProof{signature: empty}.encode_length_delimited_to_vec()
proof_digest = SHA-256(canonical_input)
signature = ordinary Ed25519.sign(proof_digest)
```

SHA-256 通过 `Sha256::update` 按上述 chunks 增量计算，不分配 combined transcript `Vec`。domain、route 与 subprotocol 是固定的 NUL 分隔前缀，两个完整 Protobuf 消息由 Prost 的 length-delimited 编码自定界。这里签的是固定 32-byte digest，仍是普通 Ed25519，不是 Ed25519ph；接收侧使用 ordinary `verify_strict(proof_digest, signature)`。

`verify_proof_strict` 只解析 Ed25519 public key/signature、拒绝 weak key、重算 digest 并做 strict crypto verification；它不检查 Challenge/Proof UUID、nonce、purpose semantics、ID、hash 长度、challenge equality 或 max-init。Batch 2 consumer 在字段的实际使用边界执行所需 semantic validation，不提前引入 speculative shared validator。因而任意 typed fields 都可以形成 cryptographically valid proof，但在 consumer validation 通过前不授予 authority。完整消息仍确保任何已签字段变化都会使 digest/signature 失效。该设计**不宣称TLS channel binding**。

安全假设明确为：TLS在Natsume Server进程内终止；不存在TLS-terminating proxy或共享TLS identity中介；daemon只签署其当前pinned Server WSS connection收到的challenge，并不暴露任意signing oracle。若这些假设失效，必须重开本ADR并采用channel-bound proof。

`ServerHandshakeEnvelope` 与 `ClientHandshakeEnvelope` 各占一个 standalone binary WebSocket message，不属于 Active envelope。这里的 message 是 tungstenite 重组 RFC 6455 fragmentation 后暴露的应用边界；不要求访问 raw frame。`max_message_size` 在 Protobuf decode 前约束重组后的完整消息，`max_frame_size` 独立约束单个 transport fragment。Client 与 Server 都把经语义校验的 typed `ServerChallenge` / `ClientProof` 交给 pinned Prost `0.14.4` encoder；proof digest 对 Challenge 与清空 `signature` 的 Proof 做 length-delimited canonical encoding。入站字段顺序、非最短 varint、显式 default、unknown bytes、重复字段的被覆盖值与等价 repeated 布局不进入签名。Server 丢弃收到的 raw bytes，只继续使用规范再编码结果。切换 Protobuf runtime 或编码版本必须重开本 ADR 并重算全部语义摘要 golden。

### Unified dynamic actor

Registry 启动为空，不枚举 persisted Device。只有在 proof、bounded DB classification 与 capacity admission 成功后，才按需创建或复用 process-lifetime DeviceActor。

Machine Hardware ID 是 primary runtime shard，只负责让 first Enrollment、existing reconnect 与 DeviceId operator action 汇聚同一个 actor，不授予 authority。DeviceId 与 `public_key` 是同一 entry 的 aliases；alias presence 同样不授予 authority。

Actor 是 durable Enrollment transaction、immutable CredentialBundle delivery、CredentialAck、EnrollmentActivated/Ready barrier、Resume、lifecycle、Command dispatch/status、Observed 与 disconnect 的唯一排序点。control-key rotation 不是 handshake。

全新 `EnrollmentAttempt` 对任何已有 Machine Hardware ID 都必须拒绝且零签发；唯一例外是同 `enrollment_id` + exact material 对既有 awaiting/active transaction 的幂等重放。disabled/revoked Device 不能通过 Enrollment、reconnect、Ack 或 recovery 隐式恢复。control-key rotation 不是 handshake。

### Credential activation and replay

Enrollment 在 `CredentialAck` 前不创建 Device row 或 active control-key row。Actor 持久化 transaction、preallocated DeviceId 与一个 immutable public CredentialBundle，并在同一 WSS 上发送。Client 完整验证并 crash-safely 持久化 bundle 后发送 CredentialAck；随后一个 application-owned transaction 创建 Device、以 `device_control_keys.status = 'active'` 激活 control key 与 Gateway certificate、写 audit，并将 Enrollment transaction 置为 active。

`CredentialBundle` 只有 `enrollment_id` 与 `gateway_leaf_der`。`bundle_sha256 = SHA-256(CredentialBundle.encode_to_vec())`，输入是经语义校验的 typed message，并使用 proof transcript 同一 pinned Prost 版本。canonical bytes 与 SHA-256 是可持久化 public data；`CredentialAck` 回显 `enrollment_id` 与 `bundle_sha256`。Awaiting 或 active transaction 收到 exact EnrollmentAttempt 都必须重放同一 typed bundle，禁止重新签名、重新分配 DeviceId 或再次激活另一把 key。Awaiting 状态的 exact Ack 执行一次 activation transaction；active 状态的 exact Ack 验证当前 active Device/key/certificate ownership 后是 durable no-op。两者都返回同一 `EnrollmentActivated` facts。

Ack transaction commit 后，Server authority 已 durable active，但连接尚未进入 Active phase。Server 发送 `EnrollmentActivated{enrollment_id, device_id, bundle_sha256}`；Client 验证 transaction/hash，原子写入 Active manifest 后发送 `EnrollmentReady{enrollment_id, device_id, bundle_sha256}`，完整回显 Activated facts。`EnrollmentReady` 只是本连接的 durable-client barrier，不是新的 Server durable state。Server 收到与本连接 Activated facts 完全匹配的 Ready 后才创建并发送 `SessionReady`；同一连接重复的 exact Ready 重放同一个 SessionReady。若 Ready 或 SessionReady 丢失并断线，Client 已持有 Active manifest，下一条连接直接使用 `ResumeSession` 获取新 lease。Resume 一把已被 supersede 的 key 因 `status != 'active'` 被拒绝，不靠 Device 级整数 clock。

断线点的恢复结果冻结如下；任何一行都不依赖 Server 猜测 Client phase：

| 断线前最后一个 durable fact | Client 重连 purpose | Server 行为 | 收敛结果 |
|---|---|---|---|
| Client 只有 Prepared；Server 尚无 transaction | exact `EnrollmentAttempt` | 创建 awaiting transaction，发送 immutable Bundle | Client 持久化 Bundle 后 Ack |
| Server 已持久化 awaiting transaction/Bundle；Client 尚未持久化 Bundle | exact `EnrollmentAttempt` | 重放 exact Bundle | Client 持久化 Bundle 后 Ack |
| Client 已 BundleInstalled；Server 尚未提交 Ack | exact `EnrollmentAttempt` | 重放 exact Bundle；exact Ack 激活一次 | Server 发送 Activated |
| Server Ack transaction 已提交；Client 尚未持久化 Activated | exact `EnrollmentAttempt` | 重放 exact Bundle；exact Ack no-op | Server 重放 Activated，Client 转 Active |
| Client 已持久化 Active；Ready 或 SessionReady 丢失 | `ResumeSession` | 校验 active key/Device 后签发新 lease | 新连接收到 SessionReady |

**2026-08-23 修订（可恢复 Enrollment transaction）**：下列图冻结 Client 本地权威、Proof purpose 与 control `enrollment_requests.state` 的转移。Prepared 与 BundleInstalled 均未 enrolled；Server transaction active 与 Client manifest Active 是两个明确的 durable barrier。`SessionReady` 只携带本连接的 `session_id`；Active 帧只回该值做租约校验。Active envelope 不携带 `authority_revision`。

**2026-08-20 修订（删除 `devices.control_authority_revision`）**：不存在 Device 级 authority integer。当前 control key 由 `device_control_keys.status = 'active'` 表达（partial unique `one_active_device_control_key`）。key replacement supersede 旧行；Resume 被 supersede 的 key 因 status 拒绝。`device_control_keys.activated_revision` / `retired_revision` 与 `enrollment_requests.baseline_authority_revision` 仍残留、曾依赖已删除的 devices 列，属 owed-to-drop，随 keys 表评审处理——不发明新的全局 clock。Identifier 与 FK 使用 `device_id`（含 `resolved_device_id` / `proposed_device_id`），同一 UUIDv7 surrogate。

本地权威与 Enrollment transaction 是不同事实。先落下 identity、canonical UUIDv7 `enrollment_id`、create-only control key、Gateway key 与 exact CSR，才能签 Enrollment Proof；Gateway 证书只存在于 Server 下发的 CredentialBundle 中，Client 在 Ack 前 crash-safe 落盘。Client 只有在收到 `EnrollmentActivated` 并原子写入 Active manifest 后才改变 Proof purpose。

```mermaid
stateDiagram-v2
    [*] --> NoIdentity
    NoIdentity --> Prepared: persist identity, enrollment_id, keys and exact CSR
    Prepared --> Prepared: EnrollmentAttempt retry, bundle not durable
    Prepared --> BundleInstalled: persist exact CredentialBundle
    BundleInstalled --> BundleInstalled: EnrollmentAttempt retry, re-Ack exact bundle
    BundleInstalled --> Active: persist EnrollmentActivated facts
    Active --> Active: ResumeSession obtains a new lease
```

`NoIdentity` / `Prepared` / `BundleInstalled` 均未 enrolled。缺 ID/key/CSR、坏 key/CSR、identity 不匹配或 manifest phase/material 不一致全部 fail closed，不自动生成新 transaction 或改发 Resume。Active manifest 缺失时，即使 Server transaction 已 active，Device 仍用相同 EnrollmentAttempt 取回 exact Bundle，经幂等 Ack 取回 `EnrollmentActivated`。control-key rotation 不是 handshake。

```mermaid
flowchart TD
    proof[ClientProof]
    proof --> purpose{oneof purpose}
    purpose -->|EnrollmentAttempt| tx{transaction classification}
    tx -->|absent, HWID/key free| createTx[create awaiting_credential_ack + immutable bundle]
    tx -->|same id + exact material, awaiting| replayBundle[replay exact CredentialBundle]
    tx -->|same id + exact material, active| replayActiveBundle[replay exact CredentialBundle]
    tx -->|id/material/ownership mismatch| reject[reject zero writes]
    purpose -->|ResumeSession| resumeKey{signature key is active and Device enrolled?}
    resumeKey -->|yes| lease[issue session_id]
    resumeKey -->|no| reject
```

Enrollment 与 Resume 是互斥 purpose。只有 exact transaction replay 可以跨 Server awaiting/active phase 收敛；Resume 对非 active key、Enrollment 对不相关的既有 HWID/key，全部拒绝且零签发。不得把一种 purpose 改写成另一种。

```mermaid
stateDiagram-v2
    [*] --> awaiting_credential_ack: new EnrollmentAttempt
    awaiting_credential_ack --> active: durable CredentialAck
    awaiting_credential_ack --> expired: window or deadline
    awaiting_credential_ack --> awaiting_credential_ack: replay same bundle
    active --> active: exact EnrollmentAttempt replays same bundle
    active --> active: exact CredentialAck is no-op; send Activated
```

这些 `enrollment_requests.state` 值仅适用于 durable control Enrollment transaction。CredentialAck 才创建 `devices` row 与 `status = 'active'` 的 control key；EnrollmentActivated/Ready 不重复激活任何 Server authority。`devices.state` 的 disable/revoke 由 operator lifecycle 排序，Enrollment / reconnect / Ack 不得隐式复活。control-key rotation 不是 handshake。

### Limits, shutdown, and restart

TLS handshake、WSS、PreAuth、signature verification、DB classification、provisional actor、frame size、deadline 与 outbound send 均使用以下冻结的 hard limits：

| 边界 | 值 |
|---|---:|
| Device rows + live Enrollment transaction reservations | 600 |
| provisional actors | 128 |
| in-flight TLS handshakes | 128 global |
| TLS handshakes per IPv4 / IPv6 `/64` | 4 |
| post-TLS HTTP/WSS connections | 2048 global，复用current HTTP connection cap |
| upgraded Device WSS | 768 |
| concurrent PreAuth connections | 64 |
| PreAuth per IPv4 / IPv6 `/64` | 4 |
| signature verification concurrency | 16 |
| PreAuth DB reads | 8 |
| TLS handshake timeout | 5s |
| Challenge send / Proof timeout | 各3s |
| total PreAuth timeout | 10s |
| Proof binary message | 1024 bytes |
| Active binary message | 64 KiB |
| outbound send timeout | 10s |

Permit顺序固定为`TCP accept → global/per-source TLS-handshake permit → spawn handshake → post-TLS HTTP connection permit(2048) → Device-WSS permit(768) → global/per-source PreAuth permit(64/4) → subprotocol → 101`。TLS permit在握手结束释放；HTTP permit持有至Hyper connection结束；Device-WSS permit持有至socket关闭；PreAuth permit持有至actor attach或preauth关闭。任一permit满时在对应阶段立即拒绝，不创建无界waiter。Provisional actor semaphore满时在proof/classification后返回ServerBusy并关闭。Rate state必须bounded，IPv6按`/64`归一。

Fleet capacity由 SQLite 中 Device rows 与 live Enrollment transaction reservations 共同表达。Provisional actors 使用单一 RAII semaphore；Registry 不保存可能漂移的持久化 class 或 budget counter。Registry 只拥有 aliases、actor task state 与 typed Running/ShuttingDown phase。

Startup只执行 schema/set-based recovery、初始化 quotas 与空 registry，然后 listener bind。Restart不恢复 actor、ticket、candidate、session或outbound frame；pending request与immutable bundle从 SQLite按需重放，active key重新 proof。

### Flag-day rollout

Batch 0 只建立决策、依赖准入、deterministic vectors 与 isolated private feasibility listener。Batch 1 使用预发布 breaking change 原位拆分唯一 Proto/package/descriptor 并同步现有 peers 的 subprotocol，同时只增加 dormant crypto/schema/client-local foundations；不维护旧 wire 兼容副本。

后续 dormant batch可以构建actor与client authority路径，但任何启用的中间版本都不得同时支持Device Token和control key authority。Atomic cutover同批删除Device Token、public Device Enrollment HTTP、旧 Token/Bearer registry 与 transitional schema state，并重建预发布DB与Device image。wire 已无 `ControlEnvelope` / `ClientInit` / Hello。

## Alternatives

- 保留Bearer或移入首帧：继续使用可重放共享秘密，并保留Enrollment/control双路径。
- mTLS bootstrap：需要预存在的client PKI，且连接内签发的新证书不能自然成为已完成TLS握手的client identity。
- TLS-exporter Upgrade proof：可在101前认证，但需要手工拆解TLS/WebSocket；本部署中connection-local random challenge已经提供所需的freshness。
- PAKE：需要独立预置shared secret；Machine Hardware ID不能被转换成该secret。
- 预建persisted Device slot：让启动枚举成为authority前提，并仍需另一条unknown Enrollment路径。
- 复用Gateway key：耦合不相干的key usage与恢复生命周期。

## Consequences

### Positive

- 一条WSS和一个actor排序全部Device authority transition。
- First Enrollment可在原socket进入Active，不产生Bearer。
- Immutable bundle replay使response-loss边界确定化。
- Empty startup与bounded provisional actor避免启动枚举和无界unknown allocation。
- Replacement、disable 与 revoke 由 key status 与 lease 明确排序。

### Negative / trade-offs

- First key来自物理受控窗口而非manufacturer credential；Binding与secret release必须等待inventory reconciliation。
- 未认证peer在proof前已经获得101，因此TLS/WSS/PreAuth limit是load-bearing安全边界。
- Flag day破坏当前预发布Token状态，需要协调重建DB与Device image。
- Ed25519/PKCS#8增加经过审查的crypto dependency与本地private-key lifecycle。
- 为避免actor retirement ABA，process-lifetime provisional actors最多128个；窗口内恶意有效proof可耗尽该额度直到Server restart。该风险由物理受控窗口、per-source limits与立即关窗约束，未来若需要在线回收必须另行冻结incarnation/tombstone设计。

## Acceptance basis and revisit trigger

Batch 0 的 private isolated listener 已证明 server-auth TLS 1.3、ordinary WSS 101、random challenge、deterministic Ed25519/PKCS#8 vectors、strict verification、canonical ClientProof digest 与 clean close。Batch 1 将 transcript、Prost decode/semantic validation 与 typed canonical re-encoding纳入唯一production protocol crate，但新proof消息仍未接入runtime authority。

后续还必须证明 capacity、crash cuts、actor races、immutable replay、filesystem durability、lifecycle ordering 与 500–600 Device envelope。Foundation 落地不关闭 G4。

当TLS在外部终止、provisioning不再物理受控、要求HA、多Server、出现manufacturer Device credential，或无法接受destructive coordinated rollout时重开本ADR。

## Normative sources

- [Architecture](../architecture.md)
- [Domain model](../domain-model.md)
- [Contracts](../contracts.md)
- [Security and recovery](../security-recovery.md)
- [Dependency policy](../dependency-policy.md)
