# Phase 4 状态

> 状态：`DRAFT-STEP0`
> 最后更新：2026-08-16
> G4：`OPEN`（启动分解已冻结；实现未开始）

Phase 4（Control Channel & Command Runtime）启动分解。条目通过需可定位 evidence；partial pass 记为未通过。范围以 [roadmap](../roadmap.md) §Phase 4、[契约](../contracts.md) §3.5/§3.6.5/§5/§6/§9/§12、[状态与执行模型](../state-and-execution.md) §2 与 [ADR-0033](../adr/0033-enrollment-and-device-control-boundary.md)/[ADR-0034](../adr/0034-state-execution-and-data-plane-boundary.md) 为准；wire 契约（proto envelope、`commands`/`observed_device_states` DDL、putCommand OpenAPI 声明、控制类稳定码）已全部在 Phase 0 冻结，本阶段是为冻结面接入真实 runtime，不再新增 wire surface。

## 工作包分解（启动定义，2026-08-16）

| WP | 内容 | 状态 |
|---|---|---|
| WP1 | Command 持久化与 PUT HTTP 面：RFC 8785 JCS（`serde_json_canonicalizer`）+ fingerprint v1 实现与 golden 向量、per-kind payload schema v1 冻结、`db::command` + `application::command`、`commands.state` CHECK 收口 + 调度索引（直接改初始 migration）、`PUT /api/v2/commands/{command_id}` handler（`201/200/400/409`、同事务创建审计、conflict 审计）、OpenAPI 挂载与 descriptor/TS golden 再生成 | `DONE`（见下方 WP1 落地记录） |
| WP2 | Ingress hardening 定案落地：以 `hyper::server::conn::http1::Builder` 自建 accept loop（`max_headers`、header read timeout、`with_upgrades`、graceful shutdown、保留 `ClientAddress` connect info），连接容量常量与 accept 层 semaphore，431/slow-header 负向测试，[契约](../contracts.md) §3.6.5 以 dated revision 收口该 gap | `DONE`（见下方 WP2 落地记录） |
| WP3 | WSS 服务端：upgrade 路由 `/api/v2/device/control`、subprotocol `natsume.v1` 协商失败拒绝、Bearer Device Token 常数时间认证（401-before-decode）、失败认证 IP 限流、Hello 交换（connection_epoch、协商 limits）、连接注册表（同 device 新连接置换旧连接、token 吊销即断、credential replacement 旧连接 anomaly audit——吸收 Phase 3 WP2b 挂账）、oversized frame/未知版本/非法 oneof 关闭连接 | `OPEN` |
| WP4 | Dispatcher 与 CommandStatus 回写：frozen payload → wire Command 确定性渲染（byte-identical golden）、创建与重连双触发投递、状态机单调回写（terminal 不可被 transport error 覆写、重复 terminal 合并安全）、same-ID replay 全链 | `OPEN` |
| WP5 | Device 客户端运行时：WSS client（tokio-tungstenite + rustls，信任根同 enrollment）、Enrolled 驻留点接入连接循环与重连收敛、durable journal（文件式、同 ID frame bytes 比对、不同即 `COMMAND_PAYLOAD_CONFLICT`）、receipt-after-durable、Observed snapshot（变化触发 + 低频兜底、单调 sequence 原子持久化） | `OPEN` |
| WP6 | G4 证据收口：缩比容量探针（≥50–100 条模拟 WSS 连接携 Observed 上报压 SQLite 单写者路径）、INV-CERT-01 WSS 条款（operator session 不可建立 WSS）激活为真实测试、ErrorCode 跨 transport 一致性、G4 evidence 登记 | `OPEN` |

依赖序：WP1 → WP2 → WP3 → WP4 → WP5 → WP6（WP2 先于 WP3 是因为 WSS upgrade 必须运行在最终 accept loop 上，避免 listener 路径二次返工）。沿 Phase 3 惯例，每个 WP 开包时冻结启动细目，本文件只冻结边界与跨切决策。

## 启动时冻结的跨切决策（2026-08-16）

- **`commands.state` 值集**：`created`、`received`、`running`、`succeeded`、`failed`、`cancelled`、`expired`、`manual_intervention_required`（wire `CommandState` 的 lower-snake 投影 + Server-only 前置态 `created`）。CHECK 直接改初始 migration 落地——预发布阶段单一 migration 策略（owner 决定，2026-08-16），不做增量表重建。转移单调：`created → received → running → terminal`；terminal 五态互不可达、不可回退、不可被后续 transport error 覆写；重复 terminal 上报合并安全（幂等）。Phase 4 的写者只产生 `created`（PUT）、`received`/`running`/device 上报 terminal（CommandStatus 回写）；`cancelled` 无 Server 侧触发面、`expired` 无 deadline 写者，两者在 Phase 4 不可达，登记于本表随后续 Phase 落地。
- **deadline**：PUT request 无 deadline 字段（Phase 0 冻结面），`deadline_at` 在 Phase 4 无写者保持 NULL，wire `deadline_unix_ms` 渲染为 0。
- **`sync_secret` 的秘密边界**：payload schema v1 不含 password（秘密不得进入 `frozen_payload_json`/DB，[契约](../contracts.md) §6）；wire `SecretBytes` 由渲染时从 vault 注入，该注入属 Phase 5 `SYNC_SECRET` 语义。Phase 4 接受并持久化 `sync_secret` Command，但 dispatcher 对其不渲染不投递（typed 内部 hold，零 wire 效果），登记为 Phase 5 接线 hook。
- **payload JSON ↔ proto 映射**：per-kind payload schema v1 = proto body message 的封闭 JSON 投影（snake_case 字段名、deny unknown fields）；`uint64` 字段验证上限 2^53−1（JCS/ES6 数字安全域，越界拒绝 `COMMAND_PAYLOAD_INVALID`→HTTP 面为 `INVALID_REQUEST` 族的 payload 校验失败）；`bytes` 字段以 lowercase hex 字符串表示。验证后的 JCS 规范形即存储形（[契约](../contracts.md) §3.5）。
- **WSS 端点**：路径 `/api/v2/device/control`（GET upgrade，同端口同 router）；subprotocol token 冻结为 `natsume.v1`。认证先于协商：无/错 token → `401`（Protobuf decode 之前），token 合法但 subprotocol 不匹配 → `400` + `PROTOCOL_VERSION_UNSUPPORTED`。
- **ingress 定案方向**：自建 accept loop（选项一），不走「评审接受」——离线赛场拓扑产不出该选项要求的部署证据，且 hyper 明示其默认 limit 不稳定，不能作为冻结契约的载体。header count/size、slow-header timeout、连接容量全部以硬编码 Rust 常量落地（数值在 WP2 开包冻结），按 §3.6.5「文档化常量」纪律记入契约。
- **授权**：`putCommand` 为 `admin` 角色 operator action（viewer 拒绝 `403`），复用既有 session 中间件；Device WSS 面零 operator 语义，两面不共享认证通道（INV-CERT-01）。

## WP1 落地记录（2026-08-16）

- 交付：`application::command`（校验 + JCS + fingerprint v1）、`db::command`（BEGIN IMMEDIATE 同事务创建审计 + UniqueViolation 竞态救援重分类）、`audit::command`（`command_create` 词汇已先行注册于契约 §3.6.4 三表）、`http::handler::command`（admin-only、路径先于 body 校验、16 KiB route 上限、typed cause 逐项映射）、`putCommand` OpenAPI 挂载（path object 与 Phase 0 声明逐项相等，仅 mounted 簿记变化）。
- 三条 fingerprint golden 向量经带外（Python 独立实现）逐字节复算吻合；opus 对抗审查 2 阻断均为契约登记滞后（本次随包补齐），代码零阻断。
- **`deadline_at` 由 NOT NULL 放宽为可空**：Phase 0 的 DDL（NOT NULL）与 Phase 0 冻结的 PUT 面（无 deadline 字段）互相矛盾，Phase 4 无 deadline 写者，向可空解决并由插入路径钉死 NULL。
- 审查非阻断挂账：(a) conflict 审计行顶层列回显请求的 `group_correlation_id`（契约允许——分组即其用途；禁令作用域为 `redacted_detail_json`），待 owner 如认为不妥再收紧；(b) golden 向量 (c) 的非 ASCII JCS 分支在生产不可达（payload 全字段 printable-ASCII 校验），属理论加固；(c) 嵌套层重复键无独立用例（结构上由每层 serde struct 的 `duplicate_field` 保证，7 kind 全部字段为 typed struct、无 `Value` 子树）；(d) `deadline_at` 可空性仅由插入路径隐式钉住（schema 契约测试不 pin notnull 维度，全表一致）；(e) 未认证 + 超大 body 返 `401` 而非 `413`（与 import commit 路由层序逐字一致，认证短路先于 body 读取，资源上更优）；(f) fingerprint v2 引入时须按「旧版本永久有效」重审 replay 判定中的版本比较。

## WP2 落地记录（2026-08-16）

- 交付：`server/src/serve.rs`（四常量 + permit-先于-accept semaphore + 饱和 `warn!` + hyper http1 Builder limits + `with_upgrades` + 排空前释放 listener + graceful 排空）；`commands::run_until` 换用该 loop；三条真 TLS 负向/回归测试（431 超 header 且新连接恢复、slow-header 关闭上界钉至 15s、graceful shutdown 排空在飞请求）。回归网 = `client_enrollment`（7 条真 TLS 场景，经 `commands::run_until` 驱动新 loop）+ `commands` 套件；`tls.rs` 单测 harness 仍走 `axum::serve`，不覆盖新 loop（登记为已知界限）。
- hyper-util 0.1.20 未为 http1 `UpgradeableConnection` 实现 `GracefulConnection`，以每任务 `GracefulShutdown::watcher()` guard + watch 通道显式触发 hyper 排空替代，语义等价。
- 契约 [§3.6.5](../contracts.md) 已以 dated revision 收口五项 ingress 决策（含超缓冲 431、10s 派生 keep-alive idle、排空无界依赖 systemd 三条行为边界）。
- 审查非阻断挂账：(a) `source_ip` 落库值链路无端到端回归（仅单测 harness 注入）；(b) graceful-drain 测试以 250ms/100ms sleep 硬等在飞状态，负载下有 flake 风险，事件化改造登记待办；(c) 容量 semaphore 无 e2e（2048 连接不宜在 CI 制造），行为由代码审查 + G4 缩比探针（WP6）背书。

## 已登记待办

- G4 证据登记需含各包 head 的全绿 CI run（待 owner push）。
- Phase 2 状态页的负向断言（import 路径不触碰 `commands`）在 WP1 落地后复核其表述仍然成立。
- `cancelled`/`expired` 的触发面（cancel API、deadline 语义与 sweeper）不属 Phase 4 冻结范围，留待后续 Phase 定义；G4 不以其为通过条件。
- Web Panel 的 Command mutation UI 不在 Phase 4（roadmap 未列；PUT 面的消费者在本阶段为测试与后续 Phase 的 Panel）。
