# ADR-0038: Unified ordinary-WSS Device control authority

> Status: `ACCEPTED`
> Implementation: `NOT DEPLOYED — PENDING ATOMIC CUTOVER`
> Scope: Device control identity, unified Enrollment/control WSS, dynamic authority actors, credential activation, and destructive pre-release cutover
> Consolidates: —
> Supersedes: [ADR-0033](0033-enrollment-and-device-control-boundary.md)
> Superseded by: —

## Implementation boundary

本 ADR 于 2026-08-19 接受为预发布 flag-day 目标，不表示目标已经实现。

在 production schema、descriptor、route、Server 与 daemon 同批切换之前，当前 HTTPS Enrollment、Device Token、Bearer-before-101、`natsume.v1` 与现有恢复流程继续是机器契约。任何中间版本都不得混合 Token/control-key authority、旧/新 registry 或旧/新 Enrollment state。

## Context

当前设计把公开 HTTPS Enrollment 与 Bearer WSS 控制分为两条 Device 路径，credential issuance、reconnect、replacement、lifecycle eviction 与 command dispatch 因而跨越多个 admission 与 runtime owner。把 persisted Device 在启动时预建为 runtime slot 仍会保留该分裂，并把启动枚举变成 authority 前提。

部署仍是单 Server 进程、单 SQLite、约 500 台 Device、无 HA、物理受控 provisioning period。Machine Hardware ID 可观察，不能认证 Device。Gateway TLS key 服务本地浏览器 origin，不能兼任 control identity。

首次 control key 没有更早的 Device credential 可以背书。窗口内接受未知 hardware 的 first key 是明确的残余信任，只能由容量限制、物理清点、Binding 与 secret release gate 约束，不能被描述为制造商身份或硬件证明。

## Decision

### Dedicated control identity

每台 Device 在完成本地 Machine Hardware ID 验证后、首次网络连接前生成专用 Ed25519 control signing key。Private key 为 daemon-owned `0600`、create-only PKCS#8 DER，不离开 Device；Server 只保存 public key 与派生 key ID。Control key 与 Server TLS、Origin CA、Gateway TLS key 完全分离。

ControlKeyId 固定为：

```text
SHA-256(
  "NATSUME-CONTROL-KEY-ID-v1\0"
  || u8(0x01)                 // algorithm_id: ED25519
  || uint16_be(32)
  || public_key[32]
)
```

`0x01` 是本版本冻结的唯一 algorithm ID；增加算法必须修订 domain/version 并增加跨实现 golden，不能复用该字节。使用维护中的 Ed25519/PKCS#8 Rust 实现；禁止手写曲线运算、签名解析或自定义密码学原语。

### Ordinary-WSS challenge-response

目标继续使用普通 pinned server-auth TLS 1.3 与 RFC 6455 Upgrade，不要求 mTLS、TLS exporter 或认证 header：

```text
TLS 1.3
→ HTTP 101 / exact subprotocol natsume.control.v2
→ standalone binary WebSocket message: ServerChallenge
→ standalone binary WebSocket message: ClientProof
→ standalone binary WebSocket message: exact ClientInit
→ classified dynamic DeviceActor
```

HTTP 101 只建立 transport。Proof 完成前，连接没有 Device、Enrollment、Command、Observed 或 lifecycle authority。

每条 WSS connection 拥有一次性随机 challenge ID 与 server nonce；它们只存在于该 connection-local PreAuthSession，并在 proof 成功、失败、timeout、非法 message 或 disconnect 后销毁。不得建立全局 challenge lookup 或允许第二次 proof。

签名 transcript 以固定 domain 分隔，并覆盖固定 route `/api/v2/device/control`、subprotocol `natsume.control.v2`、challenge、双方 nonce、协议版本、intent、control public key、Machine Hardware ID、optional DeviceId、Enrollment attempt ID 与 exact ClientInit hash。Fresh challenge 证明 connection-local freshness 与 key possession；intent、route/subprotocol context 与 exact ClientInit hash 防止跨用途、跨 route 或 ClientInit substitution。该设计**不宣称 TLS channel binding**。

安全假设明确为：TLS在Natsume Server进程内终止；不存在TLS-terminating proxy或共享TLS identity中介；daemon只签署其当前pinned Server WSS connection收到的challenge，并不暴露任意signing oracle。若这些假设失效，必须重开本ADR并采用channel-bound proof。

ServerChallenge、ClientProof 与 ClientInit 各占一个 standalone binary WebSocket message，不属于 active envelope。这里的 message 是 tungstenite 重组 RFC 6455 fragmentation 后暴露的应用边界；不要求访问 raw frame。`max_message_size` 在 Protobuf decode 前约束重组后的完整消息，`max_frame_size` 独立约束单个 transport fragment。ClientInit 由共享 canonical encoder 一次构造并逐字节发送；未来 wire adapter 必须在 application classification 前拒绝 unknown、duplicate、non-minimal 或非 canonical encoding。

### Unified dynamic actor

Registry 启动为空，不枚举 persisted Device。只有在 proof、exact ClientInit、bounded DB classification 与 capacity admission 成功后，才按需创建或复用 process-lifetime DeviceActor。

Machine Hardware ID 是 primary runtime shard，只负责让 first Enrollment、existing reconnect、key rotation 与 DeviceId operator action汇聚同一个 actor，不授予 authority。DeviceId 与 persisted control-key IDs 是同一 entry 的 aliases；alias presence 同样不授予 authority。

Actor 是 first Enrollment、pending approval、immutable CredentialBundle delivery、CredentialAck、Resume、key rotation/recovery、Gateway refresh、lifecycle、Command dispatch/status、Observed 与 disconnect 的唯一排序点。

FIRST 对任何已有 Machine Hardware ID 都必须拒绝且零签发。Enrolled Device 的 distinct key 必须显式 operator approval；disabled/revoked Device 不能通过 Enrollment、reconnect、Ack 或 recovery 隐式恢复。

### Credential activation and replay

First Enrollment 在 Ack 前不创建 Device row 或 active control-key row。Actor 持久化 request、preallocated DeviceId 与一个 immutable public CredentialBundle，并在同一 WSS 上发送。Client 完整验证并 crash-safely 持久化 bundle 后发送 CredentialAck；随后一个 application-owned transaction 创建 Device revision 1、激活 control key 与 Gateway certificate、写 audit，并将 request 置为 active。Commit 后同一 socket 获得 SessionReady 并进入 Active。

CredentialBundle canonical bytes 与 SHA-256 是可持久化 public data。Response loss 必须重放同一 bytes，禁止为同一 issuance 重新签名、重新分配 DeviceId 或重复推进 revision。

不同 key replacement 中，旧 key 与旧 session 保持 authority，直到新 candidate 对 exact bundle 完成 durable Ack。一个 transaction supersede 旧 key、activate 新 key并推进 ControlAuthorityRevision；commit 后旧 lease失效。Approval operation 直接生成并持久化 bundle，状态从 PendingApproval 进入 AwaitingCredentialAck，不保留独立 Approved/CredentialIssued 阶段。

### Limits, shutdown, and restart

TLS handshake、WSS、PreAuth、signature verification、DB classification、provisional actor、frame size、deadline 与 outbound send 均使用以下冻结的 hard limits：

| 边界 | 值 |
|---|---:|
| Device rows + live FIRST reservations | 600 |
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
| Challenge send / Proof / ClientInit timeout | 各3s |
| total PreAuth timeout | 10s |
| Proof binary message | 1024 bytes |
| ClientInit binary message | 48 KiB |
| Active binary message | 64 KiB |
| outbound send timeout | 10s |

Permit顺序固定为`TCP accept → global/per-source TLS-handshake permit → spawn handshake → post-TLS HTTP connection permit(2048) → Device-WSS permit(768) → global/per-source PreAuth permit(64/4) → subprotocol → 101`。TLS permit在握手结束释放；HTTP permit持有至Hyper connection结束；Device-WSS permit持有至socket关闭；PreAuth permit持有至actor attach或preauth关闭。任一permit满时在对应阶段立即拒绝，不创建无界waiter。Provisional actor semaphore满时在proof/classification后返回ServerBusy并关闭。Rate state必须bounded，IPv6按`/64`归一。

Fleet capacity由 SQLite 中 Device rows 与 live first-enrollment requests 共同表达。Provisional actors 使用单一 RAII semaphore；Registry 不保存可能漂移的持久化 class 或 budget counter。Registry 只拥有 aliases、actor task state 与 typed Running/ShuttingDown phase。

Startup只执行 schema/set-based recovery、初始化 quotas 与空 registry，然后 listener bind。Restart不恢复 actor、ticket、candidate、session或outbound frame；pending request与immutable bundle从 SQLite按需重放，active key重新 proof。

### Flag-day rollout

Batch 0 只允许决策、依赖准入、deterministic vectors 与 isolated private feasibility listener；不得修改 production route、AppState、descriptor、migration、OpenAPI、daemon或Token/WSS bytes。

后续 unreachable/dormant batch可以构建新协议、schema、actor与client，但任何启用的中间版本都不得同时支持旧Token和新control key authority。Atomic cutover同批删除Device Token、public Device Enrollment HTTP、旧Proto/registry，重写初始migration并重建预发布DB与Device image。

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
- Replacement、disable与revoke由revision和lease明确排序。

### Negative / trade-offs

- First key来自物理受控窗口而非manufacturer credential；Binding与secret release必须等待inventory reconciliation。
- 未认证peer在proof前已经获得101，因此TLS/WSS/PreAuth limit是load-bearing安全边界。
- Flag day破坏当前预发布Token状态，需要协调重建DB与Device image。
- Ed25519/PKCS#8增加经过审查的crypto dependency与本地private-key lifecycle。
- 为避免actor retirement ABA，process-lifetime provisional actors最多128个；窗口内恶意有效proof可耗尽该额度直到Server restart。该风险由物理受控窗口、per-source limits与立即关窗约束，未来若需要在线回收必须另行冻结incarnation/tombstone设计。

## Acceptance basis and revisit trigger

Batch 0必须以private isolated listener证明：server-auth TLS 1.3、ordinary WSS 101、random challenge、deterministic Ed25519/PKCS#8 vectors、strict verification、exact ClientInit hash binding与clean close；production surface保持不变。

后续还必须证明capacity、crash cuts、actor races、immutable replay、filesystem durability、lifecycle ordering与500–600 Device envelope。Batch 0不关闭G4。

当TLS在外部终止、provisioning不再物理受控、要求HA、多Server、出现manufacturer Device credential，或无法接受destructive coordinated rollout时重开本ADR。

## Normative sources

- [Architecture](../architecture.md)
- [Domain model](../domain-model.md)
- [Contracts](../contracts.md)
- [Security and recovery](../security-recovery.md)
- [Dependency policy](../dependency-policy.md)
