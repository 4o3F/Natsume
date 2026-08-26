# Device Control Resolve/Reconcile 最终设计

> 状态：`FINAL DESIGN`
> 适用范围：Device Enrollment、普通 WSS Active session、Gateway credential、Binding、运行时配置、凭据、Session 与 Home 控制
> 实施方式：预发布 flag day；Proto、Server、Client、数据库和测试必须作为一个不可拆分的协议代际同步落地

本文定义 Device control 的完整目标架构。本文范围内的最终语义以本文为准；通用安全、身份、部署和桌面边界继续遵守 [Architecture](architecture.md)、[Domain model](domain-model.md)、[Contracts](contracts.md)、[Security and recovery](security-recovery.md) 与 [ADR-0032](adr/0032-device-identity-and-local-credential-lifecycle.md)。

## 1. 核心模型

系统只有“Server 解析精确目标，Client 持续收敛”的状态控制面，不存在 Command delivery plane。所有资源共享同一组方程，不再按是否需要Client input拆成两种模型：

```text
ConcreteTarget = Resolve(ServerIntent, ClientInput)
ActualState    = Reconcile(ConcreteTarget)
```

Server从一个一致性数据库视图中的committed truth构造每个资源的typed ServerIntent，将current ClientInput交给静态Resolver，并且只从事务提交后重新构造的Intent解析ConcreteTarget。Wire `ServerIntentState`是其中需要Client Input部分的公开投影；Unit-input资源和已经Bound的Binding不为此发送空Intent。Client的InputProvider根据该公开Intent产生durable input；独立Reconciler只消费最新ConcreteTarget，执行Apply、Verify并重新采样后发布ActualState。每次更新都是replace-latest；消息不是操作、队列或delta，重发同一状态不得产生额外业务副作用。

协议必须删除以下概念及其持久化、HTTP、wire、审计和运行时配套：

- 通用 `Command`、`CommandStatus`、FIFO、delivery attempt、`outcome_unknown` 和通用 operation history；
- 显式 `SYNC_STATE`、`SYNC_SECRET`、`OPEN_BINDING_PROMPT` 与 `MaterialRequest`；
- `BindingRequest`、`BindingProposal`、`BindingResult`、`GatewayCredentialProposal` 以及其他资源专用 Request/Result/Proposal Active packet；
- `map<string, Any>`、通用 resource payload、自由格式 config、通用 resource table 与动态 resource registry。

删除投递面是正确性要求：Server crash 后不需要判断某次操作是否送达，Client crash 后不需要恢复一条操作队列，断线也不产生未知结果。两端只需从 durable facts 重新计算“现在应该是什么”和“现在实际上是什么”。

### 1.1 权威分区

| 信息 | 权威 Owner | 约束 |
|---|---|---|
| Device、Binding、Account mapping、credential revision、Gateway credential lifecycle | Server committed truth | Client 不得创建、推进或确认这些事实 |
| Server Intent | Server | 表示当前待解析意图及输入 generation，不是可执行 Client target |
| Concrete Target | Server | `Resolve` 后从 committed truth 编译的精确目标；Client只能 reconcile，不能推进 authority |
| Gateway CSR、Binding submission等 Client Input | Client | 是不可信当前输入；必须绑定 current lease 与 exact ServerIntent并经Resolver校验 |
| Client实际状态与完成证据 | Client ActualState | 只陈述从ConcreteTarget重新采样的本机事实；不授予Server truth |
| Client本地private key、CSR、应用事务与完成记录 | Client durable local state | 必须在相应Input或ActualState发布前crash-safe持久化 |
| Session lease | Server connection-local authority | 不跨连接持久化；只授权当前 fenced socket |

双向快照必须各自静态分成两个 authority plane：

1. `ServerStateSnapshot.intent`：需要Client Input的current ServerIntent公开投影；`target`：所有资源的完整ConcreteTarget；
2. `ClientStateSnapshot.input`：Gateway CSR 与 Binding submission 等当前 ClientInput；`actual`：artifact、Caddy leaf、Binding context、Session状态和durable transition completion。

ClientInput 绝不能被解释为 authority、ACK 或完成证据。ActualState 也不是通用 ACK；所有资源只能用 typed convergence predicate 判断收敛。需要 ActualState 改变生命周期时，由独立 Server IntentPolicy产生 typed transition，事务 commit 后才生成下一轮 Intent；ActualState 不能直接修改 current target。

### 1.2 全局不变量

- Active 双向只传完整状态快照和 typed terminal close。
- 所有 wire 类型和领域类型静态封闭；统一框架统一行为与生命周期，但不抹平领域字段。
- Server 在一次编译中要么产生语义完整、相互一致的 Intent与ConcreteTarget，要么不发送；不得拼接不同数据库时点的 Binding、Account、密码或 credential revision。
- Client 在完整验证新 ServerState 后才同时替换 current Intent/Target；任一 plane不得把缺字段解释为“沿用上一版”。
- 无全局 configuration revision、binding-set clock 或通用 resource version。幂等键来自资源自身的 ID、revision、epoch 和完整 typed context。
- 持久化于 SQLite `INTEGER` 的 revision/epoch wire 值域为 `1..=i64::MAX`；零值、溢出和回退都必须 fail closed。
- 可修正业务拒绝留在对应 current ServerIntent evaluation 中。Malformed、unauthorized、session fence violation 和不可能的状态组合才使用 typed protocol failure/close。
- 所有外部输入先完整 decode 和 semantic validation，再发生任何持久化或副作用。

## 2. 统一 Resource framework

### 2.1 唯一资源代数

每个受控领域只有四种静态类型：

```rust
trait ResourceTypes {
    type ServerIntent;   // Server-owned unresolved policy/authority intent
    type ClientInput;    // untrusted Client-owned current input; may be ()
    type ConcreteTarget; // exact Server-resolved target
    type ActualState;    // Client-sampled result of reconciling that target
}
```

`ClientInput = ()` 只是无需远端输入的特例；它不形成另一种Resource，也不产生空wire message。Binding、Gateway、Runtime、Session与Home全部遵守同一管线。

Level 与 Transition 也只是 ConcreteTarget/ActualState 的两种字段语义，而不是两套控制模型：

- Level 持续要求 actual 与 target exact，例如 Runtime Config、Binding Access、Gateway leaf/data plane 与 Device lock；
- Transition 在 target 中携带单调 epoch，在 actual 中携带 durable completed epoch，例如 terminate 与 Home reset。

### 2.2 Server Resolver

Server先从同一数据库一致性视图构造immutable typed ServerIntent；它已经包含该次解析所需的committed facts与policy。Resolver因此严格只有ServerIntent和current ClientInput两个状态输入，不依赖filesystem、Caddy、桌面session或socket writer：

```rust
trait Resolver {
    type ServerIntent;
    type ClientInput;
    type ConcreteTarget;
    type ServerTransition;

    fn resolve(
        intent: &Self::ServerIntent,
        input: &Self::ClientInput,
    ) -> ResolveDecision<Self::ConcreteTarget, Self::ServerTransition>;
}

enum ResolveDecision<Target, Transition> {
    Stable(Target),
    Commit(Transition),
    ProtocolViolation(ErrorCode),
}
```

- `Stable(Target)`：current ServerIntent与ClientInput已经能给出exact ConcreteTarget；包括等待输入时要求本地保持artifact absent/blocked的目标。
- `Commit(Transition)`：输入导致领域事实、current evaluation或lifecycle改变。Server必须提交typed transaction、重新读取truth、重建ServerIntent并再次调用Resolve；事务前推测的Target永远不能发送。
- `ProtocolViolation`：malformed、unauthorized、generation conflict 或不可能输入；零业务写入并 typed close。

Resolver 没有通用 Grant/Reject 类型。Binding 的 `AcceptBinding` 与 `RecordBindingRejection`、Gateway 的 `GrantGatewayCertificate` 与 `ReplaceGatewayCredential` 都只是各自封闭的 `ServerTransition`。顶层只负责“commit 后重新 resolve”，不解析 transition 内部业务含义。

Wire Intent不是第五种资源状态：它只是typed ServerIntent中供远端InputProvider消费的最小公开字段。Resolver直接使用Server侧完整typed值，绝不能反过来从wire投影补猜数据库事实；Target仍严格满足`Resolve(ServerIntent, ClientInput)`。

### 2.3 Client InputProvider 与 Reconciler

需要输入的资源由独立 InputProvider消费 ServerIntent。Input 必须在发布前 durable；网络重发只能重放同一当前值：

```rust
trait InputProvider {
    type ServerIntent;
    type ClientInput;
    type LocalInputState;

    fn produce(
        intent: &Self::ServerIntent,
        local: &Self::LocalInputState,
    ) -> Option<Self::ClientInput>;
}
```

`None` 表示 current input 尚未完成；完整 Client snapshot中的字段缺失会清除 Server 记录的旧 input，绝不继承上一帧。Gateway 对“当前 generation 已不可恢复”使用其 typed input presence表达，不借用 ActualState；Binding 只在志愿者确认后产生 input。

Reconciler完全不读取 ServerIntent或ClientInput，只接受 ConcreteTarget：

```rust
trait Reconciler {
    type ConcreteTarget;
    type ActualState;
    type LocalFacts;
    type ReconcilePlan;
    type VerifyInput;

    fn plan(
        target: &Self::ConcreteTarget,
        actual: &Self::ActualState,
        local: &Self::LocalFacts,
    ) -> Self::ReconcilePlan;

    fn verify(
        target: &Self::ConcreteTarget,
        input: &Self::VerifyInput,
    ) -> Self::ActualState;

    fn converged(
        target: &Self::ConcreteTarget,
        actual: &Self::ActualState,
    ) -> bool;
}
```

Reconciler只定义纯计划、验证和收敛；具体 controller执行副作用并从 durable artifact或实际runtime重新采样。它不执行SQL、不创建Server transition、不解释Binding evaluation，也不能把apply函数返回值直接当作ActualState。

### 2.4 IntentPolicy 与反馈

ActualState 不反向进入 current Resolve。需要改变 Server lifecycle 时，由独立 IntentPolicy从 Server truth、policy、operator facts和fresh ActualState产生typed transition：

```text
NextServerIntent = DeriveIntent(ServerTruth, Policy, ActualState)
```

例如 Gateway 的 completed reconcile failure或leaf expiry会提交同一个 `ReplaceGatewayCredential`；Binding Access failure只保持原Intent/Target并继续reconcile；Operator Unbind提交Binding truth后创建新negotiation intent。任何反馈都必须先commit，之后才编译下一轮ServerIntent与ConcreteTarget。

### 2.5 静态组合

Server 和 Client 显式组合独立组件，例如：

```text
GatewayCredentialResolver    GatewayInputProvider    GatewayReconciler
BindingResolver              BindingInputProvider    BindingAccessReconciler
RuntimeConfigResolver        RuntimeConfigReconciler
SessionControlResolver       SessionControlReconciler
HomeResolver                 HomeReconciler
```

顶层循环可以统一调度，但不能通过字符串resource name、`Any`、通用JSON payload或动态registry调用领域逻辑。新增资源需要编译期新增类型、组件、Proto字段、验证与测试。

Wire type只在承担独立语义边界时存在，例如完整plane、整体presence、tagged union、security分区或可复用领域值。`SecretBytes` 的redaction boundary、`GatewayCertificateGrant` 的整体presence、`BindingContext` 的Target/Actual复用以及四个authority plane都必须保留；单处使用且不拥有presence或验证语义的同义wrapper必须删除。

## 3. Active wire 与完整快照

### 3.1 Envelope

Active envelope 的 body 只能是：

```text
ServerActiveEnvelope
  session_id: fixed 16-byte UUID
  body: ServerStateSnapshot | ServerClose

ClientActiveEnvelope
  session_id: fixed 16-byte UUID
  body: ClientStateSnapshot | ClientClose
```

`session_id` 是 Server 生成的 canonical UUIDv7，以 RFC 9562 network-order 固定 16 bytes 表示。字符串、可变长 UUID 或业务 Device ID 都不能替代它。每个 envelope 都回显当前 lease；旧 socket、错误长度/variant/version 或非 current lease 的 frame 在任何写入前拒绝。

`ServerClose{error_code, action}` 与 `ClientClose{error_code}` 只表示本连接无法安全继续。每方向每连接至多发送一次，发送后不再发送业务 frame。业务拒绝不使用 Close。ErrorCode 必须是有界、非自由文本的稳定 token；未知但语法合法的 token 保持 opaque。

### 3.2 `ServerStateSnapshot`

Server每帧同时发布完整的Client-visible Intent投影与完整ConcreteTarget：

```text
ServerStateSnapshot
  intent: ServerIntentState
    gateway_credential: optional GatewayCredentialIntent
    binding: optional BindingNegotiationIntent
  target: ConcreteTargetState
    gateway: GatewayTarget
    binding_access: BindingAccessTarget
    runtime_config: RuntimeConfigTarget
    session_control: SessionControlTarget
    home: HomeTarget
```

Intent字段缺失表示当前没有该输入协商，不能继承旧Intent。Unit-input资源不需要伪造一个空公开Intent，但其Server侧typed Intent仍参与Resolve。ConcreteTarget的五个资源字段在semantic decode后全部必需；资源内部presence表达精确absence，例如`BindingAccessTarget.bound`缺失要求access artifact全部清除，`GatewayTarget.certificate`缺失要求数据面保持BLOCKED且当前generation证书不eligible。

完整快照没有通用snapshot ID。WebSocket保证单连接有序；Client另外维护仅进程内receive generation来取消陈旧plan。Intent/Input关联依赖`credential_id`与`negotiation_id`；Target/Actual关联依赖`credential_id`、`binding_id`、`credential_revision`与epoch。Server可以重复发送字节或语义相同的快照，结果必须是no-op。

### 3.3 `ClientStateSnapshot`

```text
ClientStateSnapshot
  input: ClientInputState
    gateway_credential: optional GatewayCredentialInput
    binding: optional BindingInput
  actual: ActualState
    gateway: GatewayActualState
    binding_access: BindingAccessActualState
    runtime_config: RuntimeConfigActualState
    session_control: SessionControlActualState
    home: HomeActualState
```

每帧是两个plane的完整当前值。Client不能只发送变更字段；Input缺失清除Server记录的旧input，Actual资源字段缺失则是malformed而不是继承。Input的durable source仍在Client；丢包后下一份完整snapshot重放当前值。Actual只能由相应ConcreteTarget Reconciler重新采样，不能由Input或ServerIntent推测。

任何通用序列化、日志或持久化层都不得接收包含原始password的`ServerStateSnapshot`。Server在受控内存中编译并直接编码；Client在decode后立即分离secret-bearing ConcreteTarget与可持久化的非秘密semantic Target。

## 4. Session authority 与 freshness barrier

每台 Device 同时只有一个 current control lease。安装新 lease 与 fencing 旧 socket 是同一 Server actor 决策；新连接到达不允许两个 socket 同时拥有 authority。旧 frame 即使签名身份相同，也因 lease 不匹配而零写入拒绝。

`SessionReady` 后第一条 Client Active frame 必须是fresh、完整的`ClientStateSnapshot`。在该帧通过完整语义校验并将Input/Actual以current `session_id`和Server receive-time持久化前：

- 不Resolve Gateway或Binding input；
- 不授予 Gateway leaf；
- 不接受 Binding；
- 不依据ActualState执行Gateway replacement或其他IntentPolicy transition；
- 不宣告任何资源在当前连接已收敛；
- 不基于数据库中的last-known ActualState编译依赖当前本机事实的新Intent。

第一帧可以同时携带Input；Server先原子持久化fresh Input/Actual snapshot，再在同一actor turn中按current lease运行IntentPolicy与Resolver。数据库中上一连接留下的ActualState只用于Panel显示、drift提示和恢复诊断。

每台显示终端只管理一个eligible graphical session。所有登录用户共享同一Device-level ConcreteTarget，不存在per-user Binding、per-user lock、per-user Home或并行contestant session。没有或存在多个eligible graphical session时，Session Reconciler fail closed并报告typed ActualState，不猜测目标用户。

## 5. Enrollment：Active authority 之前的协商

Enrollment 只注册或替换 Device control authority。它与 Gateway credential、Binding 和业务配置完全解耦。

Enrollment发生在Active authority建立之前，因此不放进Active snapshot。它复用相同的Intent/Input/Resolve、exact replay和durable-before-publish原则，但保留pre-auth Handshake transport与人工审核barrier。

### 5.1 Enrollment material 与状态

`EnrollmentAttempt` 的 immutable material 只有：

```text
EnrollmentAttempt
  enrollment_id
  candidate_control_public_key
  evidence_quality
```

- `enrollment_id` 是 Client 在首次网络尝试前生成并 durable 保存的 canonical UUIDv7；
- candidate key 固定为 32-byte Ed25519 public key，必须等于签署当前 `ClientProof` 的 key；private key 是 daemon-owned create-once material，不离开 Client；
- `evidence_quality` 是固定硬件身份配方产生并随 attempt 固化的 `MEDIUM` 或 `STRONG` advisory 值，不授予 authority；
- Machine Hardware ID、daemon/agent version 和 challenge 属于外层 proof/session context，不是 attempt immutable material；
- attempt 不含 Gateway key、CSR、certificate、CredentialBundle、CredentialAck、Binding 或业务配置。

同 `enrollment_id` + exact material 是 replay；同 ID + 不同 material 是稳定冲突。所有新 attempt 必须人工审核。Server durable state 只能是：

```text
pending_review | approved | active | denied
```

`denied` 是该 attempt 的 durable 业务终态。不存在 Enrollment TTL、activation deadline、自动 expiry 或 expiry sweeper。

### 5.2 Challenge、审核与激活

完整 handshake 为：

```text
ordinary pinned server-auth WSS
  -> ServerChallenge{challenge_nonce}
  -> ClientProof{purpose = EnrollmentAttempt | ResumeSession}
  -> EnrollmentReviewStatus{PENDING_REVIEW | DENIED}, when applicable
  -> ServerHandshakeEnvelope.enrollment_activated:
       EnrollmentAuthority{enrollment_id, device_id}
  -> ClientHandshakeEnvelope.enrollment_ready:
       EnrollmentAuthority{exact preceding facts}
  -> SessionReady{session_id}
  -> first ClientStateSnapshot
```

Activated 与 Ready 是两个有方向、有阶段的 envelope variant，但载荷是同一个 `EnrollmentAuthority`，不定义两份字段完全相同的 message。Server variant 表示 authority activation 已提交；Client variant 表示同一组 facts 已 crash-safe 安装。

`ServerChallenge` deadline 只限制当前连接接受唯一 `ClientProof` 的窗口。Proof 已验证并将 attempt durable 接纳后，deadline 销毁，不成为 Enrollment TTL。比赛期间 `pending_review` 与 `approved` 都不自动过期。

Provisioning window 只门禁：

- 新 attempt admission；
- operator approve。

它不门禁 exact replay、已提交 activation 的恢复、Ready/SessionReady 收尾、Resume，也不门禁 Active 中的 Gateway CA 签发。

审核与激活规则如下：

1. 新 exact attempt 在窗口开放时原子写为 `pending_review`，随后才可报告 pending。
2. Operator deny 原子写 `denied` 与 redacted audit；exact replay 得到稳定 denied。
3. Operator approve 时若同一 exact candidate Handshake attachment 在线且 proof 仍为 current，approval 与 authority activation 可在一个事务中提交，最终 state 为 `active`。
4. Approve 时没有 exact current attachment，只提交 `approved`。下一次同一 exact attempt 必须重新经历 challenge/proof；验证成功后再以事务推进为 `active`。
5. Replacement 在新 attempt 的 current exact proof 与 activation transaction commit 前不得 supersede、删除或驱逐旧 control authority。Activation commit 才是 authority cut。
6. 只有 activation commit 成功后发送 `enrollment_activated: EnrollmentAuthority`。Client crash-safe 原子安装 current authority manifest 后才回显 exact `enrollment_ready: EnrollmentAuthority`；Server 收到并验证后才安装 lease并发送 `SessionReady`。

Client 在收到 `SessionReady` 前保留 exact enrollment finalization record。以下恢复都不需要猜测阶段：

- pending/approved 期间断线：重做 challenge，发送 exact attempt；
- activation commit 后 `enrollment_activated` 丢失：active attempt 的 exact replay重新发送同一 `EnrollmentAuthority`；
- Client authority manifest commit 后 `enrollment_ready` 丢失：Client 仍以 exact finalization record 重放该 attempt与同一 `EnrollmentAuthority`；
- Server 收到 `enrollment_ready` 后 SessionReady 丢失：同一路径重新建立一个新 lease并重放 SessionReady；
- 只有收到 SessionReady 并 durable 清除 finalization marker 后，后续连接才使用 `ResumeSession`。

Server 和 Client 的每一个成功 wire barrier 都晚于其所证明的 durable fact。重复 `enrollment_activated`、`enrollment_ready` 与 SessionReady 只重放/替换连接级 lease，不重复 authority mutation。

## 6. Gateway Resolve/Reconcile pipeline

Gateway certificate issuance完全位于已认证、已fenced且通过fresh snapshot barrier的Active session。Server owns credential generation、Device/certificate identity、Gateway hostname SAN、profile、EKU、validity、Origin CA与grant；Client owns private key和exact CSR。

### 6.1 四平面 shape

```text
GatewayCredentialIntent
  credential_id

GatewayCredentialInput
  credential_id
  gateway_csr_der?

GatewayTarget
  credential_id
  certificate: none | durable GatewayCertificateGrant

GatewayActualState
  credential_id?
  state
  gateway_leaf_sha256?
```

`GatewayCredentialIntent` 是Server唯一非终态generation的wire投影。`GatewayTarget.certificate`缺失仍是精确ConcreteTarget：本地必须保持Gateway BLOCKED，且该generation没有eligible certificate；certificate存在表示durable grant已经提交。协议不存在candidate promotion、current/candidate重叠或预生成下一张证书。

`ClientInputState.gateway_credential`整体缺失表示input仍pending。Message存在且CSR存在表示private key与exact CSR已在发布前crash-safe持久化；message存在但CSR缺失表示Client确认该generation的input已不可恢复，Resolver必须执行replacement。它不是Error packet或通用枚举。

### 6.2 InputProvider 与 Resolve

新Device第一份fresh `ClientStateSnapshot`持久化后，若不存在非终态generation，IntentPolicy原子创建唯一head；Binding状态不参与。每个`credential_id`由Server创建，重复snapshot、dirty hint、transaction retry或restart不得创建第二个非终态generation。

Client看到新Intent后：

1. 在本地生成private key与CSR；
2. 严格本地校验key/CSR；
3. 原子持久化`{credential_id, private_key, exact CSR, input state}`；
4. 才发布带CSR的`GatewayCredentialInput`。

首次发布后同一Intent只能重放same ID/same CSR，不得静默换key。若发布前尚未形成durable input，input字段保持缺失；若发布后或crash recovery中确定key/CSR不一致、丢失或不可恢复，发布same ID且CSR absent的typed recovery input。

Gateway Resolver规则为：

- input absent：`Stable(GatewayTarget{same ID, certificate absent/current grant})`；
- exact current ID + CSR absent：`Commit(ReplaceGatewayCredential)`；
- exact current ID +首次合法CSR：严格验证DER、signature、算法、key strength和SPKI后签发，并`Commit(GrantGatewayCertificate)`；
- same ID + same CSR：exact replay，零重复签名/零重复generation，从committed truth返回current target；
- same ID + different CSR、malformed CSR、unauthorized lease或foreign generation：`ProtocolViolation`，零业务写入并typed close。正确恢复路径是CSR-absent input，不能用不同CSR隐式换key；
- stale terminal ID：`Stable(current target)`，不能恢复旧generation。

CSR requested subject、SAN、extension、EKU和validity不授予authority；Server完全从Device与部署policy构造证书。Gateway没有可由Client修正后继续同一generation的业务rejection，因此不携带evaluation字段。

Leaf DER、chain、serial、SPKI hash、有效期、exact CSR及policy metadata作为一个grant原子持久化。签名成功但事务失败的leaf不得发送；commit后丢包由完整GatewayTarget重放同一DER，不重新签名。

### 6.3 Reconcile 与 ActualState

Gateway Reconciler完全不读取CSR input或Binding negotiation。Certificate缺失时先阻断数据面并报告current ID的`RESTORING`；certificate存在时必须验证：

- current generation private key与leaf SPKI匹配；
- leaf/chain终止于bootstrap Local Origin Trust Root；
- Device identity、Gateway hostname SAN、profile、EKU与validity符合Server policy；
- leaf仍在允许时间域内并可由固定TLS stack接受；
- artifact以严格权限、原子写入、fsync/rename和可恢复切换安装；
- Caddy validate/reload后实际加载所选leaf。

ActualState结构约束固定如下：

| GatewayState | credential_id | gateway_leaf_sha256 | 语义 |
|---|---|---|---|
| `ABSENT` | absent | absent | 尚未reconcile任何GatewayTarget |
| `RESTORING` | present | absent | 正在等待grant或执行Apply/Verify；不触发replacement |
| `RECOVERY_REQUIRED` | present | absent | exact target已不可恢复；IntentPolicy触发replacement |
| `BLOCKED` | present | required | matching leaf已加载，但策略要求阻断数据面 |
| `READY` | present | required | matching leaf已加载且数据面可用 |
| `UPSTREAM_UNHEALTHY` | present | required | matching leaf已加载，但upstream不健康 |

`gateway_leaf_sha256`只从Gateway/Caddy实际加载的完整leaf DER采样，不是磁盘文件、PEM、chain、SPKI或serial hash。错误hash长度与不可能的state/presence组合属于semantic malformed；stale ID是旧ActualState，只能`NoChange`。

Current target的最终state携带matching hash即credential收敛；合法但不匹配的hash触发`ReplaceGatewayCredential`。收敛不要求Gateway为`READY`：UNBOUND而有意`BLOCKED`或upstream故障时，只要hash匹配，credential仍收敛。

### 6.4 统一 Replacement

以下入口全部提交同一个`ReplaceGatewayCredential(old_id)`领域事务：

- ConcreteTarget形成前，GatewayCredentialInput为exact current ID且CSR absent；
- ConcreteTarget形成后，GatewayActualState为exact current ID的`RECOVERY_REQUIRED`；
- final ActualState的leaf hash与current grant不匹配；
- Server依据committed `not_after`和自身clock发现leaf过期。

该事务原子终态化old generation并创建唯一新`credential_id`。Commit前old仍是唯一head；commit后new是唯一head。重复旧Input、旧ActualState、expiry tick或transaction retry都是`NoChange`。

Client看到新Intent后必须生成全新private key与CSR，即使旧key仍可读也禁止复用。Replacement期间数据面保持BLOCKED。Server在generation非终态期间durable保留current grant并随完整Target重放；旧generation终态化后旧grant不再作为target出现。

Control-key replacement保留同一`device_id`的唯一非终态Gateway generation。Disable保留它但停止issuance/replacement；重新enable后若leaf过期，先走相同replacement。Revoke终态化Gateway credential并驱逐lease。Destructive reprovision创建新`device_id`，不继承旧Device的Gateway lifecycle。

## 7. Binding Resolve/Reconcile pipeline

Binding negotiation、领域allocation与本地contestant access是三个通过typed value连接的独立组件，不是一个请求/结果或一体化controller。

### 7.1 四平面 shape 与 UI

```text
BindingNegotiationIntent
  negotiation_id
  evaluation?

BindingInput
  negotiation_id
  submission_epoch
  seat_code

BindingAccessTarget
  bound?: BoundTarget

BindingAccessActualState
  assignment_state
  credential_state
  context?
```

每个UNBOUND generation恰有一个durable、Server-owned`BindingNegotiationIntent`：首次Active fresh barrier后原子创建initial negotiation；Unbind删除current Binding truth并在同一transaction创建全新`negotiation_id`；restart或重复编译保持ID；新Intent使所有旧Input失效。Bound时`ServerIntentState.binding`缺失，明确关闭InputProvider与UI。

Binding UI只消费Intent与本地Home/session条件：current Intent存在时自动显示。它不读取BindingAccessTarget或ActualState，不存在Operator prompt gate或`OPEN_BINDING_PROMPT`。

### 7.2 InputProvider

`submission_epoch`对每个negotiation从1开始严格单调，最大为`i64::MAX`。志愿者每次点击确认必须先把`{negotiation_id, next_epoch, canonical seat_code}` crash-safe更新为current BindingInput，再发布完整ClientStateSnapshot。网络重发不推进epoch；只有新的人工确认推进。

InputProvider看到新Intent后建立新per-negotiation counter；Intent消失后才清除已接受Input。旧UI action、旧negotiation或counter损坏必须fail closed。

### 7.3 Resolve、Reject 与 Accept

Binding Resolver只在current lease上处理current Intent/Input：

- same epoch + same seat：exact replay；已有current evaluation或accepted association时零写入；
- same epoch + different seat：`ProtocolViolation`，零业务写入并typed close；
- lower epoch或非current negotiation：stale，返回current Stable target；
- higher epoch：新的人工确认，进入领域一致性校验。

Seat existence、Seat→Account mapping、Device eligibility与Seat/Device唯一occupancy必须在同一领域事务的一致视图校验。

可修正业务拒绝返回`Commit(RecordBindingRejection)`，不改变Binding truth。Commit后current Intent携带bounded `BindingEvaluation{submission_epoch,error_code}`；message presence本身表示rejection。新higher input覆盖它，不保留proposal history、pending queue或独立result row。

Accept返回`Commit(AcceptBinding)`，该transaction必须原子：

1. 重检current lease、Intent、epoch和seat/account facts；
2. 消费negotiation；
3. 创建`binding_id` occupancy UUID；
4. 写current Binding truth与accepted`{negotiation_id, submission_epoch, seat_code}` replay association；
5. 写redacted audit。

Commit即产生Server authority；不等待ActualState promotion，不建立pending seat reservation或两阶段commit。Bound状态的exact accepted replay零写入；Unbind后旧association只用于审计，不对新Intent授权。

### 7.4 ConcreteTarget 与 Reconcile

Resolver从committed truth编译：

```text
BindingAccessTarget
  bound absent  -> assignment和credential都必须absent
  bound present -> BoundTarget
    context: BindingContext
      binding_id
      account_id
      seat_code
      domjudge_username
      credential_revision
    password: SecretBytes
```

Binding Access Reconciler只消费该Target，不读取negotiation/Input/evaluation。`BindingAccessActualState`共享一份非秘密`BindingContext`，不重复assignment/credential context：

```text
BindingAccessActualState
  assignment_state: BindingArtifactState
  credential_state: BindingArtifactState
  context?: BindingContext
```

Bound收敛要求两个state都是`APPLIED`且Actual context与Target exact；此时且仅此时context存在。Absent target收敛要求两个state都是`ABSENT`且context缺失。其他组合不携带context；冲突、部分可读或无法可靠采样时标记对应`FAILED`，具体path与旧值只留本地bounded diagnostic。

ActualState不promote Binding、不改变Seat occupancy，也不进入Binding Resolver。失败只让同一ConcreteTarget继续reconcile。

## 8. ConcreteTarget resources 与业务行为

### 8.1 Bound target、Import 与自动发布

Binding、Unbind与material Import commit都会改变committed truth；transaction commit后Server自动把Device标为dirty并重新Resolve完整ConcreteTarget，不需要人工publish或同步动作。

`BoundTarget` 必须从同一数据库一致性视图读取 `binding_id`、`account_id`、seat、username、`credential_revision` 和 exact current password。任何 join 缺失、vault 解密失败或 revision/context 不一致都不能发送部分或沿用旧 target；该 Device 保持 fail closed并产生 Server-side typed diagnostic。

Import不写Binding，但对Seat→Account、username、password或credential revision的committed变化会自然改变所有相关Device的完整BindingAccessTarget。No-op Import不制造resource epoch或target churn。

### 8.2 Password 与 credential artifact

Password 直接随 authenticated、encrypted、current-lease-fenced WSS 的完整 `BoundTarget` 发送。不存在 `SYNC_SECRET`、secret request 或 material request。

原始 password 只允许存在于：

- Server vault decrypt 到 Protobuf encode 的受控短生命周期内存；
- Client decode 到 reconcile 的受控短生命周期内存；
- 最终 daemon-owned、严格权限、原子写入并由固定 consumer读取的 credential artifact。

Client不得持久化包含原始password的通用snapshot、frame、journal、queue或LKG。可持久化的semantic ConcreteTarget必须先剥离secret，只保存binding/account/revision等非秘密context。Password不得进入日志、`Debug`、ActualState、ClientInput、audit、metrics、crash report、core dump、swapable diagnostic bundle或普通LKG metadata。Secret wrapper的生成类型必须默认redacted；daemon进程禁用core dump，并对临时buffer做有界生命周期与尽力清零。

Credential artifact必须绑定完整`{binding_id, account_id, credential_revision}`。只有原子写入、权限/owner校验和consumer可读性验证完成后，ActualState才能报告APPLIED。新ConcreteTarget context不匹配时立即使旧artifact不eligible；Absent target时必须删除contestant credential artifact，但不能删除control/Gateway material。

### 8.3 Runtime Config

Runtime Config 是全 fleet singleton。第一版唯一远程字段是 canonical DOMjudge HTTPS origin：scheme 固定 `https`，host/port canonical，无 userinfo、path、query 或 fragment。

```text
RuntimeConfigTarget
  domjudge_origin
```

只有一个字段时不再建立单用途 `DomjudgeTarget` wrapper；未来新增真正具有整体 presence、独立校验或复用语义的配置组时，再引入对应 typed aggregate。

以下值是部署期bootstrap，不能经ServerIntent或ConcreteTarget远程修改或轮换：

- control endpoint、IP 与 port；
- Control Root；
- fleet namespace；
- Local Origin Trust Root；
- Gateway hostname。

Runtime Config Resolver的ClientInput为Unit，直接从committed singleton intent产生latest-wins RuntimeConfigTarget。Reconciler应用新origin时先使数据面BLOCKED，再render、validate、原子激活并health-check。任一步失败都保持BLOCKED；绝不能继续代理旧origin或回退旧LKG。RuntimeConfigActualState明确报告target origin是否已应用及failure state，不携自由文本或path。

### 8.4 UNBOUND 与 Gateway data plane

`BindingAccessTarget.bound`缺失时的ConcreteTarget恒为：

- assignment absent；
- contestant credential absent；
- Gateway data plane `BLOCKED`；
- Binding UI只由BindingNegotiationIntent与Home/session本地条件派生为visible。

任何顺序中都必须先阻断数据面，再删除或替换 credential/context。Bound 时只有 Runtime Config、Gateway current leaf、完整 BoundTarget credential artifact和 Caddy actual config全部匹配，数据面才能 READY。

### 8.5 Session lock、terminate 与 Home

Lock 是持续的 Device-level Level target，不是一次性动作：

```text
SessionControlTarget
  lock: LOCKED | UNLOCKED
  terminate_epoch: optional uint64
```

当前eligible graphical session替换后，Client必须把同一lock target重新应用到新session。SessionControlActualState报告实际lock/session state；目标未满足就持续reconcile。

Terminate 是 Transition。看到新的 `terminate_epoch` 时，Client 在本地 durable transition record 中捕获当时唯一 eligible session identity，在任何 privileged effect 前重检该 identity。它只能终止被捕获的 session：

- 被捕获 session 已退出时可将该 epoch durable 标为完成；
- replacement 已成为 current 时不得 retarget；
- 无法证明被捕获session身份或结果时不推进completion，并保持typed failure ActualState。

Home reset 也是 Transition：

```text
HomeTarget
  reset_epoch: optional uint64

HomeActualState
  completed_reset_epoch: optional uint64
```

同epoch Prepare/Activate/Recover/GC可重入；完成record必须先crash-safe持久化，之后才发布`completed_reset_epoch`。执行较新epoch期间仍报告上一个完成值。ActualState超前、回退或本地完成记录损坏都fail closed。

Home reset只破坏contestant Home。它绝不能删除或重建control key、Gateway private key/CSR、Gateway certificate或daemon-owned credential artifact。Reset导致的Caddy、assignment、credential或session runtime drift由仍然current的ConcreteTarget重新恢复。

## 9. Server 统一处理循环

每个Device的authenticated Active事件在其静态typed lane中串行处理：

```text
on ClientStateSnapshot(frame):
  1. verify frame.session_id == current lease and socket owns that lease
  2. decode and validate the entire typed snapshot before any write
  3. persist fresh ClientInput + ActualState + receive-time + current session_id
  4. mark this lease's initial-snapshot barrier satisfied
  5. run static IntentPolicy against fresh ActualState and Server facts
       eligible transition -> commit with exact resource fencing
  6. re-read committed truth and build each current typed ServerIntent
  7. run each static Resolver with its current Intent/Input
       Stable(target) -> retain typed ConcreteTarget
       Commit(transition) -> commit, re-read truth, and resolve again
       ProtocolViolation -> zero business write, typed close, stop
  8. project complete public Intent + ConcreteTarget from that consistency view
  9. send ServerStateSnapshot only on the still-current lease
```

步骤2失败时不持久化本帧任何部分。Input/Actual持久化只记录untrusted current state，不能被当作领域transition已经提交。Binding业务拒绝只能通过`RecordBindingRejection`写current Intent evaluation；Gateway input/actual recovery和Server expiry都compare-and-commit同一个`ReplaceGatewayCredential`。若同帧多条原因竞争，第一条commit后其余旧ID自然stale，不得连续换代。

每个领域transaction commit后必须重新读取truth并重新Resolve；不得把transaction前在内存中推测的accept、grant、replacement或Target直接拼入ServerState。`ProtocolViolation`保持零业务写入。

Server的actor/mailbox只提供有界串行、capacity和fencing，不进入Resolver/Reconciler接口。数据库notification、actor dirty bit或wake-up channel都只是延迟优化；即使全部丢失，周期性从committed truth重新DeriveIntent/Resolve和Client完整snapshot仍能恢复一致。

编译password等secret失败时不得发送不完整snapshot，也不得继续发送缓存secret。Server记录redacted诊断，并用typed close或保持该lease无新ServerState的fail-closed路径终止连接。

## 10. Client 统一处理循环

Client把InputProvider与Reconciler拆开；所有Reconciler仍共享一个有界single-consumer effect executor。输入采集和实际I/O可以异步，但同一Device的安全激活按静态依赖顺序串行：

```text
on ServerStateSnapshot(snapshot):
  1. verify current session_id and validate complete Intent + ConcreteTarget
  2. atomically replace in-memory current ServerState
  3. split secret-bearing Target from persistable non-secret Target metadata
  4. notify static InputProviders of current ServerIntent
  5. let each Reconciler derive a plan from ConcreteTarget only
  6. enqueue one bounded effect pass

input pass:
  1. BindingInputProvider waits for a current Intent and human confirmation
  2. GatewayInputProvider creates/verifies generation-bound key and CSR
  3. persist changed ClientInput before publishing it

reconcile pass:
  1. enforce immediate fail-closed gates, especially UNBOUND/origin drift
  2. reconcile RuntimeConfigTarget
  3. reconcile BindingAccessTarget assignment/credential artifacts
  4. reconcile GatewayTarget certificate and Caddy actual state
  5. recover/apply Home transition
  6. apply Device-level lock and captured-session terminate transition
  7. before every irreversible or visible activation point, re-read current Target
     and compare the resource ID/context/epoch used by the plan
  8. Verify by resampling actual local facts into typed ActualState
  9. publish one complete ClientStateSnapshot
```

新ServerState replace-latest。尚未开始的陈旧plan直接取消；已完成Prepare的plan只能在确认ConcreteTarget仍current后Activate。无法安全取消的本地事务进入资源自己的Recover路径，不允许继续按陈旧target完成可见激活。

Binding人工输入和Gateway自动CSR都必须先durable保存为current ClientInput，再由完整snapshot发布。ActualState由filesystem、Caddy、logind与durable completion record重新采样，不能把apply函数返回成功直接复制为actual。

Client 启动、状态变化和低频周期都发布完整 snapshot。高频相同状态可以合并，但不能永久抑制周期性对账。

## 11. Crash safety、丢包与重连

### 11.1 Durable boundaries

Server 必须先 durable commit，后发布：

- Enrollment admission/review/activation；
- Gateway intent generation创建、CSR接受、leaf grant、replacement/revoke；
- Binding negotiation、evaluation、accept/unbind；
- Import、Runtime Config、lock target、terminate/Home epoch等ServerIntent或领域truth变更；
- redacted authority audit。

Client 必须先 durable commit，后发布：

- Enrollment attempt/control private key与 finalization marker；
- Gateway generation private key与 exact CSR input；
- Gateway/credential artifact activation metadata；
- Binding per-negotiation submission epoch与 seat input；
- terminate捕获记录与 Home completion record。

ActualState 只有在重新采样 durable artifact或实际 runtime 后才能发送。不能安全持久化或可靠采样时，不得发送“成功”、较新 completion epoch、`ABSENT` 或匹配 hash来掩盖失败。

### 11.2 Failure semantics

| 故障点 | 恢复语义 |
|---|---|
| ServerState 在网络中丢失 | Server dirty/周期重发完整 latest；Client保持最后已验证状态且不推断新 authority |
| ClientState 丢失 | durable ClientInput与重新采样的ActualState在下一完整snapshot重发 |
| Server领域commit后、发送前崩溃 | restart从 committed truth重新运行IntentPolicy/Resolver并编译；无“是否送达”状态 |
| Client Apply前崩溃 | restart从latest non-secret Target metadata、durable ClientInput和本地artifact重新计划；password仅从重新认证后的完整ServerState获得 |
| Client artifact写入中崩溃 | 原子文件/transaction Recover；未验证前ActualState不推进 |
| Gateway leaf grant包丢失 | durable grant随下一个完整GatewayTarget重放 |
| Binding accept后的ServerState丢失 | Client重放exact BindingInput；Server由current Binding和accepted association零写入识别 |
| Server restart | 所有 lease失效；新 SessionReady后重新要求第一份 fresh Client snapshot |
| Client reconnect | 新 lease fence旧 socket；Client先发完整fresh ClientInput与ActualState，再运行IntentPolicy/Resolve/Reconcile |
| notification/dirty hint丢失 | 低频 Server重编译与 Client周期全量上报恢复 |

断线不是业务失败，不生成通用 outcome。资源要么重放durable Input使Resolver恢复同一决策，要么继续对current Target执行Reconcile并重采样ActualState，要么由typed Close终止非法连接。

## 12. Device lifecycle

### 12.1 Disable

Disable transaction先把 Device标为 disabled，再 compare-and-evict准确 current lease。Gateway 唯一非终态 generation 和 Binding/audit facts保留，但停止任何新 Gateway issuance、grant或replacement。Client收到 close后使数据面 BLOCKED。重新 enable后必须重新 challenge/resume、获得新 lease并通过 fresh snapshot barrier，不能沿用旧 socket freshness；若保留 leaf 已过期，则通过标准 replacement transition 换代。

### 12.2 Revoke

Revoke 原子终止 current control authority、终态化 Gateway credential并驱逐 lease。旧 key、旧 Enrollment replay和旧 Gateway generation永久不再获得 authority。终态历史保留用于审计、证书撤销与恢复判断，不能因它不是 current fact而删除。

### 12.3 Reprovision 与 control-key replacement

Destructive reprovision 清除本机 identity-bound control/Gateway material，生成全新的 Enrollment attempt并经人工审核创建新 `device_id`。旧 Device row和审计保持终态；新 Device不继承 Binding或 Gateway credential。

Control-key replacement则保持同一 `device_id`、Binding以及 Gateway 唯一非终态 generation，只在 exact新 proof的 activation transaction commit时切换 control authority。

## 13. Audit、ActualState 与诊断边界

Audit 记录“谁改变了Server authority以及transaction结果”，包括Enrollment review/activation、Gateway intent generation/grant/replacement/revoke、Binding accept/unbind、Import、lifecycle和target epoch变更。内容必须typed/redacted；CSR只允许记录固定hash和credential ID，password/private key/leaf原文不进入audit。

ActualState 是每Device最新实际状态，不是append-only历史，不替代audit。重复周期snapshot通常只更新current actual与receive-time，不制造业务audit。Transition completion是Client durable evidence。ActualState不能直接改变authority或作为当前Resolver输入；它只能由独立IntentPolicy结合committed truth生成下一ServerIntent transition，后者仍须领域事务commit。

Binding rejection的current可见位置是`BindingNegotiationIntent.evaluation`。Audit可以保留一次人工提交被拒的redacted证据，但不得因此创建proposal/result operation table或让历史evaluation重新参与决策。Gateway没有可修正的同generation业务rejection，因而没有evaluation。

日志和metrics只允许稳定ErrorCode、resource kind、opaque ID、计数和延迟。禁止free-form peer detail、secret、CSR DER、certificate DER、本地path、username/password组合或可逆credential material。ClientClose是best-effort连接诊断，不改变业务transaction。

## 14. 逻辑数据模型方向

精确SQL在实施阶段冻结，但必须按领域静态建模，并满足以下方向：

| 逻辑实体 | 关键 current/durable facts | 关键约束 |
|---|---|---|
| `devices` | `device_id`、HWID、lifecycle state | 同一非 revoked HWID ownership；state封闭 |
| `enrollment_attempts` | ID、candidate control public key、evidence quality、state、resolved device、review/activation audit | immutable material；state仅pending_review/approved/active/denied；无TTL/Gateway字段 |
| `device_control_keys` | public key、device、status、originating enrollment | 每Device一个current；replacement activation原子切换 |
| `gateway_credentials` | credential ID、device、status、policy、exact CSR/hash、leaf/chain、serial、validity | 每Device至多一个非终态 generation；replacement 原子切换 head；leaf先durable；终态可审计/撤销 |
| `binding_negotiations` | current negotiation ID、device、latest submission/evaluation | UNBOUND恰一current；只保留当前evaluation |
| `device_bindings` | binding ID、device、seat、accepted negotiation/epoch | Seat/Device唯一occupancy；accept replay association |
| `accounts` + vault + mappings | account、username、revision、ciphertext、Seat mapping | BoundTarget在一致性视图中完整join；password不进target表 |
| `runtime_config` | fleet singleton DOMjudge HTTPS origin | 单一current value；无通用config JSON |
| typed Server intent/target facts | lock level、terminate epoch、Home epoch及各领域truth | typed列/typed表；Target按一致性视图解析，不建通用resource row |
| typed latest Client input | lease/session、Gateway CSR state、Binding submission | 一Device current row或静态typed子表；整份replace、可判fresh/stale |
| typed latest actual state | lease/session、receive-time、各资源actual/hash/completion | 一Device current row或静态typed子表；新lease freshness可证明 |
| `audit_events` | actor、action、resource、result、redacted detail | append-only evidence，不参与current Resolve |

ServerState snapshot默认不持久化为blob；Intent与ConcreteTarget从上述committed truth和current ClientInput解析、编译。Client只持久化durable Input、拆除password后的typed semantic Target LKG、资源artifact metadata和completion evidence。不得建立通用resource table、operation table、snapshot journal或自由格式payload。

`commands`、`credential_bundles`和与其绑定的delivery/ack字段不属于最终逻辑模型。Gateway certificate不再引用Enrollment；Enrollment表也不保存Gateway CSR、SPKI或certificate生命周期。

## 15. Proto 重构方向

Proto必须按静态领域职责拆分，并由一个exact WSS subprotocol选择整个generation。建议边界为：

```text
device_control.proto              envelopes only
device_control_common.proto       SecretBytes与双向Close等封闭值
device_control_handshake.proto    Challenge/Proof/Enrollment/Ready/SessionReady
device_control_state.proto        四平面aggregate与双向complete snapshots
device_control_gateway.proto      Gateway intent/input/target/actual
device_control_binding.proto      Binding intent/input与access target/actual
device_control_runtime.proto      Runtime Config target/actual
device_control_session.proto      lock/terminate/Home target/actual
```

方向性 shape 如下，字段号由descriptor实施批次统一压紧：

```proto
message ServerActiveEnvelope {
  bytes session_id = 1; // exactly 16-byte UUIDv7
  oneof body {
    ServerStateSnapshot server_state = 2;
    ServerClose server_close = 3;
  }
}

message ClientActiveEnvelope {
  bytes session_id = 1;
  oneof body {
    ClientStateSnapshot client_state = 2;
    ClientClose client_close = 3;
  }
}

message ServerStateSnapshot {
  ServerIntentState intent = 1;
  ConcreteTargetState target = 2;
}

message ServerIntentState {
  GatewayCredentialIntent gateway_credential = 1;
  BindingNegotiationIntent binding = 2;
}

message ConcreteTargetState {
  GatewayTarget gateway = 1;
  BindingAccessTarget binding_access = 2;
  RuntimeConfigTarget runtime_config = 3;
  SessionControlTarget session_control = 4;
  HomeTarget home = 5;
}

message ClientStateSnapshot {
  ClientInputState input = 1;
  ActualState actual = 2;
}
```

`ServerIntentState`中的字段可缺失，表示当前没有该Intent；Unit-input资源不发送空Intent。`ConcreteTargetState`和`ActualState`中的静态资源字段在semantic decode后全部必需；资源内部presence表达精确的absent target/state，绝不继承旧值。`ClientInputState`字段缺失清除previous input。

Presence、oneof default trap、enum `UNSPECIFIED`、UUID/hash长度、epoch域、canonical URL、secret wrapper和bounded ErrorCode都必须在consumer边界语义校验。Reassembled Protobuf message上限保持64 KiB；超限在decode前关闭。

本协议没有外部兼容基线。删除预发布字段时不写`reserved`，而是在一次breaking descriptor重写中压紧字段号、更新golden并同步所有peer。任何参与proof或semantic hash的canonical root及递归字段集合变化都必须切换WSS subprotocol generation。

## 16. Flag-day 实施顺序

以下步骤属于一次发布单元，顺序只表达实现依赖，不允许中间构建进入生产：

1. 冻结本文的四平面resource、authority、secret与crash invariants，建立新descriptor golden和协议正反例fixture。
2. 重写Active/Handshake Proto，移除所有Command、Result、Proposal、Bundle/Ack和通用payload；生成双端类型。
3. 重写初始migration与领域仓储：建立Enrollment、Gateway单一非终态generation、Binding negotiation/current、typed Server facts、latest ClientInput与ActualState；删除command/delivery/bundle关联结构。
4. 实现Server静态IntentPolicy、每资源Resolver、领域transaction、ConcreteTarget一致性compiler、single-current-lease fencing和fresh snapshot barrier。
5. 实现Client secret-splitting decode、每资源InputProvider、durable input/artifact stores、静态Reconciler和单一有界effect executor。
6. 接通Enrollment人工审核/activation replay，再接通Active Gateway、Binding、Runtime、credential、Session/Home资源。
7. 删除Operator command creation/publish surface及相关Panel语义；Binding/Import/Unbind和Intent mutation改为commit后自动dirty。
8. 原子切换唯一WSS subprotocol、descriptor、Server、Client、migration、fixtures、docs与deployment package；不存在双协议、双authority或fallback窗口。
9. 运行全矩阵fault injection、target image/Caddy/desktop evidence、secret scan和migration/descriptor clean-diff；所有gate通过后才允许部署。

Rollback只能回滚整个发布单元及其数据库备份，不能让一端或一种authority单独降级。

## 17. 验证矩阵

| 类别 | 必须覆盖的场景与断言 |
|---|---|
| Envelope | 每方向只有Snapshot/Close；缺body、未知enum、超64 KiB、错误session ID长度/version/variant、旧lease frame均在写入前拒绝 |
| Snapshot完整性 | Server intent/target或Client input/actual顶层缺失、target/actual缺任一必需资源、partial merge、乱序replace、重复snapshot、编译中truth变化；只接受完整一致视图且latest-wins |
| Fresh barrier | SessionReady后首帧不是ClientState、首帧malformed、数据库只有last-known input/actual、首帧完整replace；barrier前零IntentPolicy transition、Resolver write或convergence决策 |
| Lease fencing | 两连接竞争、新lease替换旧lease、旧socket晚到snapshot/close、Server restart；始终只有current lease可写 |
| Resource pipeline | 每资源`Resolve(Intent, Input)`确定性、Unit input无wire字段、Stable target只来自committed truth、Commit后重读再Resolve、ProtocolViolation零业务写入；Reconciler只读Target，Level exact match与Transition completion epoch均可重入 |
| Plane isolation | ClientInput不被当作authority/completion，ActualState不进入current Resolve，IntentPolicy不能直接伪造Actual，Binding UI/InputProvider不读取access target，Reconciler不读取Intent/Input |
| Enrollment admission | 窗口开/关、新attempt、same-ID exact replay、same-ID different material、MEDIUM/STRONG、candidate key与proof key不匹配 |
| Enrollment review | 所有新attempt人工pending、deny稳定、在线approve原子active、离线approved后exact re-proof、pending/approved跨比赛不expiry |
| Enrollment crash | admission/approval/activation各commit前后、Activated/Ready/SessionReady丢失、Client manifest写入失败、replacement旧authority保留到activation cut |
| Enrollment scope | attempt不含Gateway/Binding/config；window关闭不阻断exact recovery、Resume或Active CA |
| Gateway intent | 新Device fresh state后IntentPolicy自动创建唯一非终态generation；重复snapshot/restart不重复创建；与Binding无关 |
| Gateway input | private key+CSR durable-before-publish；input absent为pending；same ID/same CSR replay；same ID/different CSR protocol violation；same ID/CSR absent请求replacement；stale/foreign generation不改变current authority |
| Gateway policy | CSR signature/SPKI、Server-derived identity、hostname SAN、profile、EKU、validity、chain到Local Origin Trust Root；CSR requested字段不授予authority |
| Gateway resolve crash | CA签名/DB commit/ServerState send的每个cut；任何wire leaf都有durable row；Commit后重读返回同一grant，exact replay不重签或重复generation |
| Gateway reconcile | certificate absent先BLOCKED并报告current ID/RESTORING；certificate present后校验key/SPKI、chain、SAN/profile/EKU/validity并原子安装；不可恢复失败报告same ID/RECOVERY_REQUIRED，不复用旧leaf伪装成功 |
| Gateway convergence | ABSENT无ID/hash；RESTORING/RECOVERY_REQUIRED有ID无hash；final state有ID和实际loaded leaf hash；exact current ID/hash匹配时收敛，stale ID无写入 |
| Gateway replacement | CSR-input不可恢复、Actual RECOVERY_REQUIRED、loaded leaf hash不匹配与leaf expiry均走同一原子replacement；旧generation终态化并恰一新ID；Client生成全新key/CSR；重复Input/Actual/expiry tick幂等 |
| Gateway runtime health | 已加载matching leaf时，UNBOUND阻断或upstream故障只改变Gateway state而不换代；Caddy未能加载matching leaf属于completed Apply/Verify失败并触发replacement |
| Gateway lifecycle | control-key replacement保留、disable保留并停issuance、revoke撤销驱逐、reprovision新device不继承 |
| Binding intent | 首次Active创建initial negotiation、restart保持ID、Unbind同事务新ID、旧negotiation彻底失效、UI仅按Intent+本地条件自动显示 |
| Binding input | first epoch、单调递增、durable-before-publish、same epoch/same seat replay、same epoch/different seat protocol violation、lower stale、higher新确认、上界拒绝 |
| Binding resolve | unknown seat、missing mapping、ineligible、occupancy conflict提交bounded evaluation；accept原子消费negotiation、创建binding/occupancy/accepted association/audit；Commit后重读并由truth生成target |
| Binding access | Intent消失与Bound target形成互不混用；assignment/credential共享一个BindingArtifactState与coherent BindingContext；Absent exact、Bound exact、冲突或部分可读时FAILED且不上传拼凑context |
| Import/Binding view | Binding/Import/Unbind commit自动重编完整ConcreteTarget；BoundTarget完整context与password来自同一一致性视图；缺join或vault失败不发送错配/部分target |
| Secret | password每次完整ServerState传输；Server/Client日志、Debug、snapshot journal、LKG、ActualState、ClientInput、audit、metrics、core dump扫描；只允许内存和最终严格权限artifact |
| Credential context | binding/account/revision完整匹配、原子写与verify-before-observe、UNBOUND清除contestant artifact、不触碰control/Gateway material |
| Runtime Config | 只接受canonical HTTPS origin；bootstrap字段不在wire；新origin应用各失败点均BLOCKED且绝不代理旧origin；重复latest no-op |
| UNBOUND/data plane | assignment/credential absent、先BLOCKED后清理、UI derived visible；Bound依赖未齐全时仍BLOCKED |
| Lock | Device-level持续LOCKED/UNLOCKED、session replacement后重新收敛、零/多eligible session fail closed、无per-user state |
| Terminate | epoch单调、capture当前session、effect前重检、replacement不被误伤、captured已退出可完成、crash recovery不retarget |
| Home | same epoch可重入、completion write-before-publish、in-progress报告旧值、ahead/regression/corrupt record fail closed、reset不删除control/Gateway/credential artifact、drift由Target恢复 |
| Server crash | 每个领域commit前后、compile/send cut、dirty hint丢失、周期重编译；无delivery状态且恢复后要求新fresh snapshot |
| Client crash | Input持久化cut、artifact atomic write cut、plan activation前Target替换、Actual重新采样、无secret LKG仍可安全BLOCKED恢复 |
| Lifecycle | disable/re-enable fresh barrier、revoke永久拒绝旧authority、reprovision新device ID、late旧lease snapshot零写入 |
| Audit/Actual | authority mutation有redacted audit；周期ActualState不制造audit；evaluation history不参与current Resolve；所有secret/DER/path/free-text泄漏测试 |
| Data/Proto | FK/UNIQUE/CHECK、每Device一个非终态Gateway generation/current Binding negotiation/control lease、无generic resource/operation表、无单用途同义wrapper、无Command/Bundle符号、descriptor与generated code clean diff |
| Periodic convergence | 双向通知任意丢失、长时间无变化、Server/Client各自重启；低频完整对账最终恢复同一Intent/Input/Target/Actual状态 |

## 18. 完成判据

只有同时满足以下条件，Resolve/Reconcile control plane才算完成：

- Server唯一authority来自committed truth和current ServerIntent；
- Active wire每方向只有一个完整typed snapshot和typed close；
- ServerIntent、ClientInput、ConcreteTarget与ActualState在类型、authority和持久化上明确分离；
- Gateway、Binding、Runtime、Session与Home都服从`Target = Resolve(Intent, Input)`和`Actual = Reconcile(Target)`，Unit input不形成另一类资源；
- 每个Resolver和Reconciler静态独立组合；ActualState不能进入current Resolve，只能经IntentPolicy形成下一次committed Intent transition；
- Level与Transition都由Reconcile/Verify持续恢复，无投递历史或通用ack/outcome；
- Enrollment、lease、secret、certificate、Binding和epoch的每个成功barrier都位于相应durable commit之后；
- 丢包、重连和任一端crash只导致暂时drift，不导致双authority、重复allocation、错配credential、旧session副作用或secret持久化扩大；
- 全部验证矩阵在descriptor、领域测试、fault injection、真实Caddy和target desktop evidence中通过。
