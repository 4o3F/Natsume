# Phase 4 状态

> 状态：`ACTIVE IMPLEMENTATION`
> 最后更新：2026-08-23
> G4：`OPEN`（WP1–WP4 已完成；WP5 部分完成；WP6 尚未收口）

> **2026-08-19 ADR-0038 note；2026-08-23 transaction 修订**：既有 Phase 4 evidence 证明的是 Device Token、Bearer-before-101 runtime。当前 wire 为定向 handshake/Active envelopes（Server Challenge|Bundle|Activated|SessionReady；Client Proof|Ack|Ready），Proof 使用 EnrollmentAttempt/ResumeSession oneof purpose，并以 durable `enrollment_id` 跨连接重放。旧 descriptor hash 不能冒充当前 evidence，必须重新验证。无 `ClientInit`、无 `ControlEnvelope`、无 Hello。Private ordinary-WSS listener仍只证明 TLS 1.3 + 101 + Challenge/Ed25519 Proof seam，不补交WP5/WP6 evidence，不关闭G4，也不改写下方历史完成度。
>
> **2026-08-20 修订（Command 投递二分）**：下方 WP4/WP5 历史记录描述的 Device journal、重连重放全部 7 kind、宽 Observed 不再是目标契约。目标：Converge 按领域键重推；Oneshot 仅 live socket；无 Device command journal；Observed slim；keep-alive 为 WS ping/pong。历史完成度不改写。
>
> **2026-08-21 修订（OPEN_BINDING_PROMPT 空 body）**：`open_binding_prompt` 空 body，无 TTL，无 `prompt_message_id`；打开 binding-prompt screen 即 `CommandStatus` `SUCCEEDED`；确认/拒绝走 `BindingRequest`。`CommandState` 只有 `SUCCEEDED` | `FAILED`。下方「`expired` 无 deadline 写者」仍是 Phase 4 HTTP/DB 交付事实，不是 Device `CommandState` 变体。

Batch 0 local preflight（**不是 G4 PASS evidence**）必须在本地提交前运行：isolated feasibility test、current protocol/WSS/Enrollment regressions、`just ci-rust`、`just ci-contracts`、direct policy scan、docs/Mermaid validation 与两个 `cargo deny` dependency graph。结果只存在于执行会话，未绑定 commit SHA、CI run 或 retained artifact，不得登记为 Gate 通过。当前开发环境缺少 `shfmt` 与 `gitleaks`，因此 `ci-policy` 聚合 recipe 与 `secret-scan` 必须由后续 CI 补齐。Private probe 不替代 production regression，也不证明 production rejection/capacity/runtime cutover。

> **Batch 1 foundation boundary**：预发布 control protocol 已原位拆成单一 `natsume.device.control` package/descriptor，subprotocol 同步改为 `natsume.control`；定向 handshake/Active envelopes、strict signature transcript、Prost typed canonicalization 与 dormant schema 可以存在，但当前 authority 仍是 Token/Bearer。这不是 G4 PASS，也不得登记为 key-auth runtime 已完成。仓库策略明确禁止并行 `control_v2`、ProcessLock/flock/pidfile 及 SQLite 外层 Semaphore/Mutex 写锁。

Phase 4（Control Channel & Command Runtime）启动分解。条目通过需可定位 evidence；partial pass 记为未通过。范围以 [roadmap](../roadmap.md) §Phase 4、[契约](../contracts.md) §3.5/§3.6.5/§5/§6/§9/§12、[状态与执行模型](../state-and-execution.md) §2 与 [ADR-0033](../adr/0033-enrollment-and-device-control-boundary.md)/[ADR-0034](../adr/0034-state-execution-and-data-plane-boundary.md) 为准；wire 契约（proto envelope、`commands`/`observed_device_states` DDL、putCommand OpenAPI 声明、控制类稳定码）已全部在 Phase 0 冻结，本阶段是为冻结面接入真实 runtime，不再新增 wire surface。

## 工作包分解（启动定义，2026-08-16）

| WP | 内容 | 状态 |
|---|---|---|
| WP1 | Command 持久化与 PUT HTTP 面：RFC 8785 JCS（`serde_json_canonicalizer`）+ fingerprint v1 实现与 golden 向量、per-kind payload schema v1 冻结、`db::command` + `application::command`、`commands.state` CHECK 收口 + 调度索引（直接改初始 migration）、`PUT /api/v2/commands/{command_id}` handler（`201/200/400/409`、同事务创建审计、conflict 审计）、OpenAPI 挂载与 descriptor/TS golden 再生成 | `DONE`（见下方 WP1 落地记录） |
| WP2 | Ingress hardening 定案落地：以 `hyper::server::conn::http1::Builder` 自建 accept loop（`max_headers`、header read timeout、`with_upgrades`、graceful shutdown、保留 `ClientAddress` connect info），连接容量常量与 accept 层 semaphore，431/slow-header 负向测试，[契约](../contracts.md) §3.6.5 以 dated revision 收口该 gap | `DONE`（见下方 WP2 落地记录） |
| WP3 | WSS 服务端：upgrade 路由 `/api/v2/device/control`、subprotocol `natsume.control` 协商失败拒绝、Bearer Device Token 常数时间认证（401-before-decode）、失败认证 IP 限流、连接注册表（同 device 新连接置换旧连接、token 吊销即断、credential replacement 旧连接 anomaly audit——吸收 Phase 3 WP2b 挂账）、oversized frame/未知版本/非法 oneof 关闭连接。历史交付曾含 Hello 交换；当前 wire 无 Hello | `DONE`（见下方 WP3 落地记录） |
| WP4 | Dispatcher 与 CommandStatus 回写：frozen payload → wire Command 确定性渲染（byte-identical golden）、创建与重连双触发投递、状态机单调回写（terminal 不可被 transport error 覆写、重复 terminal 合并安全）、same-ID replay 全链 | `DONE`（见下方 WP4 落地记录） |
| WP5 | Device 客户端运行时：WSS client（tokio-tungstenite + rustls，信任根同 enrollment）、Enrolled 驻留点接入连接循环与重连收敛、durable journal（文件式、同 ID frame bytes 比对、不同即 `COMMAND_PAYLOAD_CONFLICT`）、receipt-after-durable、Observed snapshot（变化触发 + 低频兜底、单调 sequence 原子持久化） | `IN PROGRESS`（WP5a 已完成，见下方记录；Observed 尚未落地） |
| WP6 | G4 证据收口：缩比容量探针（≥50–100 条模拟 WSS 连接携 Observed 上报压 SQLite 单写者路径）、INV-CERT-01 WSS 条款（operator session 不可建立 WSS）激活为真实测试、ErrorCode 跨 transport 一致性、G4 evidence 登记 | `OPEN` |

依赖序：WP1 → WP2 → WP3 → WP4 → WP5 → WP6（WP2 先于 WP3 是因为 WSS upgrade 必须运行在最终 accept loop 上，避免 listener 路径二次返工）。沿 Phase 3 惯例，每个 WP 开包时冻结启动细目，本文件只冻结边界与跨切决策。

## 启动时冻结的跨切决策（2026-08-16）

- **`commands.state` 值集**：`created`、`received`、`running`、`succeeded`、`failed`、`cancelled`、`expired`、`manual_intervention_required`（wire `CommandState` 的 lower-snake 投影 + Server-only 前置态 `created`）。CHECK 直接改初始 migration 落地——预发布阶段单一 migration 策略（owner 决定，2026-08-16），不做增量表重建。转移单调：`created → received → running → terminal`；terminal 五态互不可达、不可回退、不可被后续 transport error 覆写；重复 terminal 上报合并安全（幂等）。Phase 4 的写者只产生 `created`（PUT）、`received`/`running`/device 上报 terminal（CommandStatus 回写）；`cancelled` 无 Server 侧触发面、`expired` 无 deadline 写者，两者在 Phase 4 不可达，登记于本表随后续 Phase 落地。
- **deadline**：PUT request 无 deadline 字段（Phase 0 冻结面），`deadline_at_unix_ms` 在 Phase 4 无写者保持 NULL，wire `deadline_unix_ms` 渲染为 0。
- **`sync_secret` 的秘密边界**：payload schema v1 不含 password（秘密不得进入 `frozen_payload_json`/DB，[契约](../contracts.md) §6）；wire `SecretBytes` 由渲染时从 vault 注入，该注入属 Phase 5 `SYNC_SECRET` 语义。Phase 4 接受并持久化 `sync_secret` Command，但 dispatcher 对其不渲染不投递（typed 内部 hold，零 wire 效果），登记为 Phase 5 接线 hook。
- **payload JSON ↔ proto 映射**：per-kind payload schema v1 = proto body message 的封闭 JSON 投影（snake_case 字段名、deny unknown fields）；`uint64` 字段验证上限 2^53−1（JCS/ES6 数字安全域，越界拒绝 `COMMAND_PAYLOAD_INVALID`→HTTP 面为 `INVALID_REQUEST` 族的 payload 校验失败）；`bytes` 字段以 lowercase hex 字符串表示。验证后的 JCS 规范形即存储形（[契约](../contracts.md) §3.5）。
- **WSS 端点**：路径 `/api/v2/device/control`（GET upgrade，同端口同 router）；subprotocol token 冻结为 `natsume.control`。认证先于协商：无/错 token → `401`（Protobuf decode 之前），token 合法但 subprotocol 不匹配 → `400` + `PROTOCOL_VERSION_UNSUPPORTED`。
- **ingress 定案方向**：自建 accept loop（选项一），不走「评审接受」——离线赛场拓扑产不出该选项要求的部署证据，且 hyper 明示其默认 limit 不稳定，不能作为冻结契约的载体。header count/size、slow-header timeout、连接容量全部以硬编码 Rust 常量落地（数值在 WP2 开包冻结），按 §3.6.5「文档化常量」纪律记入契约。
- **授权**：`putCommand` 为 `admin` 角色 operator action（viewer 拒绝 `403`），复用既有 session 中间件；Device WSS 面零 operator 语义，两面不共享认证通道（INV-CERT-01）。

## WP1 落地记录（2026-08-16）

- 交付：`application::command`（校验 + JCS + fingerprint v1 + 首次持久化 `DeviceState::Enrolled` gate）、`db::command`（BEGIN IMMEDIATE 同事务创建审计 + lifecycle 竞态串行化）、`audit::command`（`command_create` 词汇已先行注册于契约 §3.6.4 三表）、`http::handler::command`（admin-only、路径先于 body 校验、16 KiB route 上限、typed cause 逐项映射）、`putCommand` OpenAPI 挂载（operation/status/component 不变，404 description 覆盖 missing 或 non-enrolled Device）。
- 三条 fingerprint golden 向量经带外（Python 独立实现）逐字节复算吻合；opus 对抗审查 2 阻断均为契约登记滞后（本次随包补齐），代码零阻断。
- **`deadline_at_unix_ms` 由 NOT NULL 放宽为可空**：Phase 0 的 DDL（NOT NULL）与 Phase 0 冻结的 PUT 面（无 deadline 字段）互相矛盾，Phase 4 无 deadline 写者，向可空解决并由插入路径钉死 NULL。2026-08-20 起该列为 INTEGER UTC epoch milliseconds，不再是 RFC 3339 TEXT。
- 审查非阻断挂账：(a) conflict 审计行顶层列回显请求的 `group_correlation_id`（契约允许——分组即其用途；禁令作用域为 `redacted_detail_json`），待 owner 如认为不妥再收紧；(b) golden 向量 (c) 的非 ASCII JCS 分支在生产不可达（payload 全字段 printable-ASCII 校验），属理论加固；(c) 嵌套层重复键无独立用例（结构上由每层 serde struct 的 `duplicate_field` 保证，7 kind 全部字段为 typed struct、无 `Value` 子树）；(d) `deadline_at_unix_ms` 可空性仅由插入路径隐式钉住（schema 契约测试不 pin notnull 维度，全表一致）；(e) 未认证 + 超大 body 返 `401` 而非 `413`（与 import commit 路由层序逐字一致，认证短路先于 body 读取，资源上更优）；(f) fingerprint v2 引入时须按「旧版本永久有效」重审 replay 判定中的版本比较。

## WP2 落地记录（2026-08-16）

- 交付：`server/src/serve.rs`（四常量 + permit-先于-accept semaphore + 饱和 `warn!` + hyper http1 Builder limits + `with_upgrades` + 排空前释放 listener + graceful 排空）；`commands::run_until` 换用该 loop；三条真 TLS 负向/回归测试（431 超 header 且新连接恢复、slow-header 关闭上界钉至 15s、graceful shutdown 排空在飞请求）。回归网 = `client_enrollment`（7 条真 TLS 场景，经 `commands::run_until` 驱动新 loop）+ `commands` 套件；`tls.rs` 单测 harness 仍走 `axum::serve`，不覆盖新 loop（登记为已知界限）。
- hyper-util 0.1.20 未为 http1 `UpgradeableConnection` 实现 `GracefulConnection`，以每任务 `GracefulShutdown::watcher()` guard + watch 通道显式触发 hyper 排空替代，语义等价。
- 契约 [§3.6.5](../contracts.md) 已以 dated revision 收口五项 ingress 决策（含超缓冲 431、10s 派生 keep-alive idle、排空无界依赖 systemd 三条行为边界）。
- 审查非阻断挂账：(a) `source_ip` 落库值链路无端到端回归（仅单测 harness 注入）；(b) graceful-drain 测试以 250ms/100ms sleep 硬等在飞状态，负载下有 flake 风险，事件化改造登记待办；(c) 容量 semaphore 无 e2e（2048 连接不宜在 CI 制造），行为由代码审查 + G4 缩比探针（WP6）背书。

## WP3 落地记录（2026-08-16）

- 交付：`server/src/http/device_control.rs`（九项冻结常量、`route_layer` 前置 Bearer 认证与 IP 限流、`natsume.control` 强制选择、稳态循环、进程级单调 `connection_epoch`、连接注册表与驱逐）。历史落地含 Hello 交换；当前 proto 无 Hello，目标 handshake 为 Challenge → Proof → Bundle/Ack → Activated/Ready → SessionReady，或 Challenge → Resume Proof → SessionReady。Device lifecycle application use case 的 revoke/disable 在 DB 变更成功后（包括 noop）恰调用一次显式 evictor，HTTP handler 只传入 registry、不再直接驱逐；enrollment replacement 路径维持事务内驱逐与 `evicted_live_connection` 审计布尔（词汇已先注册于契约 §3.6.4，Phase 3 移交项闭环）；历史真 TLS + 真 WS 客户端测试不构成新 transaction wire 的 evidence。
- **注册时机（审查后修正）**：连接在首帧等待**之前**注册，使「已认证未完成 handshake」的连接对撤销/替换驱逐可见；原实现在首帧之后注册，存在最长 10s 的撤销绕过窗口（WP4 dispatch 挂同一 registry 后将成为静默绕过）。同时 handshake 窗口放行 Ping/Pong 而非判为协议错误。
- **认证顺序**：limiter → shape gate → SHA-256 → DB 查找（含 resolved Device state）→ `ConstantTimeEq` 复核，全部先于 upgrade 与任何 protobuf decode；缺失/格式错/未知/no-row/disabled/revoked 走同一 401 body/cause 与同一 IP limiter，无 oracle。只有 `DeviceState::Enrolled` 可建立 WSS；非法持久化 state 保持 500 corruption。
- **2026-08-18 state-gate 证据**：`integration-tests/tests/wss_control/auth.rs::disabled_and_revoked_tokens_share_the_normalized_wss_authentication_failure` 以真 TLS + 真 WS 覆盖 enrolled 成功、首帧后 disable 驱逐、保留 token=1/active cert、同 token 重连 401、第二台 enrolled Device 不受影响、五类失败归一化与合并 limiter 计数、零 auth audit；`invalid_persisted_device_state_remains_an_internal_wss_failure` 钉死 corruption 500。
- **依赖事实（2026-08-17 修正）**：启用 axum `ws` 与 daemon 客户端 WSS 后，`tungstenite 0.29` 的 `rand 0.9` / `getrandom 0.3` 熵栈与 `sha1`（RFC 6455 `Sec-WebSocket-Accept` 用，非安全原语）进入**生产 server 与生产 daemon 二进制**；workspace 精确 pin 有意使用私有 `__rustls-tls` feature 以避开平台 TLS / 公共根 feature，`deny.toml` 两条精确版本 skip 因此必需。
- 审查非阻断挂账：(a) eviction 发生在签发事务内、commit 之前（`device_pk` 只有进入事务读到 device 行后才可知，application 层无法先问 registry；回滚时表现为一次多余断连，安全侧 fail-safe，审计 bool 与凭据行仍同事务原子）；(b) 「missing pong」未强制——任何入站帧刷新 idle 计时，持续发应用帧但从不回 pong 的客户端可长存（真正静默者仍 60s 关闭）；(c) 限流测试不能证明限流早于 DB 查询；(d) handshake timeout / idle timeout / ping 周期三项无测试；(e) token 零化装饰性（SHA-256 中间值与 HeaderMap 原字节不清，与其他路由现状一致）。认证先于协商的「无 token + 无 subprotocol」组合已由 `upgrade_authentication_precedes_subprotocol_and_excludes_operator_sessions` 补齐。

## WP4 落地记录（2026-08-16）

- 交付：`http/device_control/render.rs`（frozen payload → wire Command 的全函数确定性渲染，全部穷举结构体字面量——proto 增删字段会编译失败；时间戳走 `time` crate RFC 3339）；`db::command::list_dispatchable_commands`（DB-as-truth，无内存队列，`(created_at_unix_ms, command_id)` 定序，`DISPATCH_BATCH_LIMIT = 256`）；创建与重连双触发投递（trait 定义在 application 层，db/application 不引用 WSS 类型）；`writeback_command_status` 单调状态机（事务内重读、终态不可覆写、重复终态零写入）与 `command_terminal` 终态审计（词汇先注册后写入器）。**游标已删除**：WP4 曾实现 `devices.terminal_result_cursor`（journal GC ack），因 Phase 4 无终态生产者且高水位标记在乱序终态下会静默丢结果而整体移除（proto 三字段、列、推进逻辑与类型），替代确认机制曾登记为 [Phase 5 计划](../planning/phase-5-plan.md) §6.1 的 D12；**2026-08-20 起 D12 关闭**——无 Device command journal，不再设计 GC/ack。
- **投递不改变 state**：状态只随 Device 上报的 CommandStatus 前进；`sync_secret` 在渲染前按 kind 跳过（Phase 4 零 wire 效果，密码属 Phase 5 vault 注入）。
- 审查后修正：(1) 游标相关的越界修正随游标整体删除而作废（见上）；(2) 投递批次内每帧发送与驱逐信号做 `select`，避免设备停住 TCP 接收窗把「token 吊销即断」推迟到 TCP 超时；(3) 终态写回成功后触发一次重新投递，使批次上限之后排队的行立即可见，不必等下一次 PUT 或重连；(4) 投递**查询**失败不再杀连接（冻结规范只要求 send 失败结束连接）。
- 审查非阻断挂账：(a) 每次唤醒重发全部可派发行而非增量（契约允许 byte-identical 重投递，存在操作面放大）；(b) 同一设备 ≥256 条长期处于 `running` 时，第 257 条在其中之一终态化前不可见（FIFO 窗口的容量边界，非停滞）；(c) `command_terminal` 审计的 `result` 由 `&'static str` 重新推导且带兜底分支，正确性依赖唯一调用点，建议改接 typed 状态；(d) 冻结规格的文件清单曾漏列当时的 Enrollment application/DB facade 与 `handler/command/tests.rs` 三处机械改动（游标随 token lookup 取回、通知器穿线、positional INSERT 列数）；Device-first 迁移后的对应路径为 `application/device/enrollment.rs` 与 `db/device/enrollment.rs`。原 (e) 测试热轮询已于 WP5a harness 收口为有界 interval poll。
- **db 层单表偏差**：已由下方 2026-08-17 的 B1–B5 专项重构记录闭环，不再保留 transition exception。

## WP5a 落地与对抗审查记录（2026-08-17）

- 交付：daemon 以 rustls + WSS 在 Enrolled 驻留点运行控制循环，Command 先写文件 journal 再回 `received`，同 ID 不同 frame 回 `COMMAND_PAYLOAD_CONFLICT`；Unauthorized 保留现有凭据、不触发自动 re-enrollment，并仅按最大 backoff 重试。控制客户端按 connect / hello / session / backoff / fixture seam 拆分，测试 harness 按 server / client / PKI seam 拆分。
- 三项阻断修正：(1) `ServerDrain` 的对端时间戳钳制到 30s reconnect 上限并记录 debug；(2) 稳态拒绝原因均以 typed、redacted warning 诊断，协商 frame 下限拒绝协议漂移陷阱，backoff 只在真实 Command 进展后复位；(3) `/var/lib/natsume/journal` 进入 tmpfiles 清单，runtime fallback 以 `0750` 原子创建目录。
- dead-connection 收口：协商 idle timeout 经闭区间钳制后成为稳态 read deadline；Pong 与 CommandStatus 两条稳态 send 同样以不大于该 idle timeout 的 deadline 包裹。静默真实 socket 覆盖 read deadline；send deadline 由可控不读 sink + paused Tokio time 覆盖，不宣称真实 TCP 满窗端到端覆盖。
- 共享面：subprotocol、wire version、frame limit、handshake timeout、canonical command-id predicate 与 Device Token encoded-shape predicate 统一由 `device-protocol` 提供；server 仍独立完成 token base64url decode、32-byte 长度检查与不可区分 401。生产 client 配置/信任根读取移入 daemon 共享模块，控制面不再依赖 enrollment domain。
- **生产 feature 隔离**：workspace-wide release build 曾把 integration crate 的 `fixture` feature 统一进生产 daemon。package smoke 现只在隔离的 `CARGO_TARGET_DIR` 中显式构建四个入包 Rust package，把该 release 目录交给 nFPM，并以 `cargo rustc --bin natsume-device-daemon -- --print cfg` 回归断言拒绝 `feature="fixture"`。
- 剩余非阻断项：WP5 的 Observed snapshot、sequence 持久化与后续执行面仍未落地；send timeout 尚无真实 TCP 接收窗耗尽测试。以上不改变 WP5a receipt/reconnect 边界，也不提前关闭完整 WP5。

## DB 单表重构完成记录（2026-08-17）

- 专项重构按 B1–B5 五批完成：生产数据库入口统一为 `Database::read` / `Database::write`，application 组合不透明 `db::Transaction<'_>` 上的单表 adapter；旧 `Database::interact`、db 自开事务与跨表 mutation 函数均已删除。
- `module-dependency-scan` 的 database-boundary transition allowlist 为零。多表 JOIN 只允许在四个已评审的纯读 query/read-model 模块中；每个模块都有索引/query-plan 测试，read-model 写入和 `db.rs` 外开启事务均由 canary 与全树扫描拒绝。
- 回滚证据覆盖 Command、Operator、Device lifecycle、Enrollment、Provisioning 与 Import：注入 audit/table/CAS 失败后，业务行、审计行、凭据与 revision 的前后 snapshot 保持相等，证明事务组合上移后原子性未退化。
- Enrollment 的证书签名目前仍在 write lock 内完成；这是保持原子 guard 的安全实现，不阻断本次架构闭环。把签名移出写锁只登记为非阻断性能优化，必须另行设计并复核重验证窗口。

## Device-first 架构闭环记录（2026-08-17）

- Device lifecycle、query、Token/Gateway credential 与 Enrollment request workflow 的 application、DB、HTTP、audit seam 已统一归入 `device`；provisioning 窗口独立归 `provisioning`，Seat、Account 与 Binding 继续归 `contest`。
- 旧顶层 enrollment 与 contest-owned Device compatibility surface 已删除；application 不再携带 `utoipa` schema，公开 DTO 由 HTTP adapter 持有。
- 本次仅移动内部 owner 与类型转换边界；HTTP operationId、status/component、内部 cause string、audit vocabulary、OpenAPI/TypeScript snapshot与 protocol golden 均保持不变，无 wire behavior change。

## Command eligibility 与 lifecycle eviction ownership 加固（2026-08-18）

- `putCommand` 现在按 `validate → BEGIN IMMEDIATE → typed DeviceState lookup → existing fingerprint → first-persist eligibility → created audit + insert` 的固定顺序执行。只有 `enrolled` 可首次持久化；disabled/revoked 返回内部 `DeviceNotEnrolled`，公开面与 missing Device 同为 `404 RESOURCE_NOT_FOUND` 且零写入/审计/通知。既有 ID 的 replay/conflict 在之后 disable/revoke 仍保持 `200`/`409`，conflict 继续使用既有独立 audit，无新稳定码、route、schema 或 proto。
- `DeviceConnectionEvictor` 与 test-only crate-visible `NoLiveDeviceConnections` 从 credential owner 移到 `application::device::connections` 并由 device parent re-export。`revoke_device` / `disable_device` 的 canonical signature 强制显式 evictor reference；成功（含 noop）一次驱逐，任意错误零驱逐。HTTP lifecycle handler 已移除直接 `.evict()`，Enrollment replacement 的事务内时机与 audit bool 不变。
- 回归覆盖 disabled/revoked first PUT、unknown Device、post-disable replay/conflict、disable-vs-new-ID 并发串行结果、HTTP 公开归一化、`from_command`/WSS frame 穷举 match，以及 lifecycle success/noop 与 notfound/audit/token/certificate/CAS failure 的 eviction 次数和回滚事实。

## 已登记待办

- G4 证据登记需含各包 head 的全绿 CI run（待 owner push）。
- Phase 2 状态页的负向断言（import 路径不触碰 `commands`）在 WP1 落地后复核其表述仍然成立。
- `cancelled`/`expired` 的触发面（cancel API、deadline 语义与 sweeper）不属 Phase 4 冻结范围，留待后续 Phase 定义；G4 不以其为通过条件。`OPEN_BINDING_PROMPT` 空 body、无 TTL；Device `CommandState` 只有 `SUCCEEDED` | `FAILED`。
- Web Panel 的 Command mutation UI 不在 Phase 4（roadmap 未列；PUT 面的消费者在本阶段为测试与后续 Phase 的 Panel）。
