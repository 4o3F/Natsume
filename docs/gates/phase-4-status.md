# Phase 4 状态

> 状态：`DRAFT-STEP0`
> 最后更新：2026-08-16
> G4：`OPEN`（启动分解已冻结；实现未开始）

Phase 4（Control Channel & Command Runtime）启动分解。条目通过需可定位 evidence；partial pass 记为未通过。范围以 [roadmap](../roadmap.md) §Phase 4、[契约](../contracts.md) §3.5/§3.6.5/§5/§6/§9/§12、[状态与执行模型](../state-and-execution.md) §2 与 [ADR-0033](../adr/0033-enrollment-and-device-control-boundary.md)/[ADR-0034](../adr/0034-state-execution-and-data-plane-boundary.md) 为准；wire 契约（proto envelope、`commands`/`observed_device_states` DDL、putCommand OpenAPI 声明、控制类稳定码）已全部在 Phase 0 冻结，本阶段是为冻结面接入真实 runtime，不再新增 wire surface。

## 工作包分解（启动定义，2026-08-16）

| WP | 内容 | 状态 |
|---|---|---|
| WP1 | Command 持久化与 PUT HTTP 面：RFC 8785 JCS（成熟库）+ fingerprint v1 实现与 golden 向量、per-kind payload schema v1 冻结、`db::command` + `application::command`、迁移 2（`commands.state` CHECK 收口 + 调度索引）、`PUT /api/v2/commands/{command_id}` handler（`201/200/400/409`、同事务创建审计、conflict 审计）、OpenAPI 挂载与 descriptor/TS golden 再生成 | `OPEN` |
| WP2 | Ingress hardening 定案落地：以 `hyper::server::conn::http1::Builder` 自建 accept loop（`max_headers`、header read timeout、`with_upgrades`、graceful shutdown、保留 `ClientAddress` connect info），连接容量常量与 accept 层 semaphore，431/slow-header 负向测试，[契约](../contracts.md) §3.6.5 以 dated revision 收口该 gap | `OPEN` |
| WP3 | WSS 服务端：upgrade 路由 `/api/v2/device/control`、subprotocol `natsume.v1` 协商失败拒绝、Bearer Device Token 常数时间认证（401-before-decode）、失败认证 IP 限流、Hello 交换（connection_epoch、协商 limits）、连接注册表（同 device 新连接置换旧连接、token 吊销即断、credential replacement 旧连接 anomaly audit——吸收 Phase 3 WP2b 挂账）、oversized frame/未知版本/非法 oneof 关闭连接 | `OPEN` |
| WP4 | Dispatcher 与 CommandStatus 回写：frozen payload → wire Command 确定性渲染（byte-identical golden）、创建与重连双触发投递、状态机单调回写（terminal 不可被 transport error 覆写、重复 terminal 合并安全）、same-ID replay 全链 | `OPEN` |
| WP5 | Device 客户端运行时：WSS client（tokio-tungstenite + rustls，信任根同 enrollment）、Enrolled 驻留点接入连接循环与重连收敛、durable journal（文件式、同 ID frame bytes 比对、不同即 `COMMAND_PAYLOAD_CONFLICT`）、receipt-after-durable、Observed snapshot（变化触发 + 低频兜底、单调 sequence 原子持久化） | `OPEN` |
| WP6 | G4 证据收口：缩比容量探针（≥50–100 条模拟 WSS 连接携 Observed 上报压 SQLite 单写者路径）、INV-CERT-01 WSS 条款（operator session 不可建立 WSS）激活为真实测试、ErrorCode 跨 transport 一致性、G4 evidence 登记 | `OPEN` |

依赖序：WP1 → WP2 → WP3 → WP4 → WP5 → WP6（WP2 先于 WP3 是因为 WSS upgrade 必须运行在最终 accept loop 上，避免 listener 路径二次返工）。沿 Phase 3 惯例，每个 WP 开包时冻结启动细目，本文件只冻结边界与跨切决策。

## 启动时冻结的跨切决策（2026-08-16）

- **`commands.state` 值集**：`created`、`received`、`running`、`succeeded`、`failed`、`cancelled`、`expired`、`manual_intervention_required`（wire `CommandState` 的 lower-snake 投影 + Server-only 前置态 `created`）。迁移 2 以 SQLite 表重建加 CHECK。转移单调：`created → received → running → terminal`；terminal 五态互不可达、不可回退、不可被后续 transport error 覆写；重复 terminal 上报合并安全（幂等）。Phase 4 的写者只产生 `created`（PUT）、`received`/`running`/device 上报 terminal（CommandStatus 回写）；`cancelled` 无 Server 侧触发面、`expired` 无 deadline 写者，两者在 Phase 4 不可达，登记于本表随后续 Phase 落地。
- **deadline**：PUT request 无 deadline 字段（Phase 0 冻结面），`deadline_at` 在 Phase 4 无写者保持 NULL，wire `deadline_unix_ms` 渲染为 0。
- **`sync_secret` 的秘密边界**：payload schema v1 不含 password（秘密不得进入 `frozen_payload_json`/DB，[契约](../contracts.md) §6）；wire `SecretBytes` 由渲染时从 vault 注入，该注入属 Phase 5 `SYNC_SECRET` 语义。Phase 4 接受并持久化 `sync_secret` Command，但 dispatcher 对其不渲染不投递（typed 内部 hold，零 wire 效果），登记为 Phase 5 接线 hook。
- **payload JSON ↔ proto 映射**：per-kind payload schema v1 = proto body message 的封闭 JSON 投影（snake_case 字段名、deny unknown fields）；`uint64` 字段验证上限 2^53−1（JCS/ES6 数字安全域，越界拒绝 `COMMAND_PAYLOAD_INVALID`→HTTP 面为 `INVALID_REQUEST` 族的 payload 校验失败）；`bytes` 字段以 lowercase hex 字符串表示。验证后的 JCS 规范形即存储形（[契约](../contracts.md) §3.5）。
- **WSS 端点**：路径 `/api/v2/device/control`（GET upgrade，同端口同 router）；subprotocol token 冻结为 `natsume.v1`。认证先于协商：无/错 token → `401`（Protobuf decode 之前），token 合法但 subprotocol 不匹配 → `400` + `PROTOCOL_VERSION_UNSUPPORTED`。
- **ingress 定案方向**：自建 accept loop（选项一），不走「评审接受」——离线赛场拓扑产不出该选项要求的部署证据，且 hyper 明示其默认 limit 不稳定，不能作为冻结契约的载体。header count/size、slow-header timeout、连接容量全部以硬编码 Rust 常量落地（数值在 WP2 开包冻结），按 §3.6.5「文档化常量」纪律记入契约。
- **授权**：`putCommand` 为 `admin` 角色 operator action（viewer 拒绝 `403`），复用既有 session 中间件；Device WSS 面零 operator 语义，两面不共享认证通道（INV-CERT-01）。

## 已登记待办

- G4 证据登记需含各包 head 的全绿 CI run（待 owner push）。
- Phase 2 状态页的负向断言（import 路径不触碰 `commands`）在 WP1 落地后复核其表述仍然成立。
- `cancelled`/`expired` 的触发面（cancel API、deadline 语义与 sweeper）不属 Phase 4 冻结范围，留待后续 Phase 定义；G4 不以其为通过条件。
- Web Panel 的 Command mutation UI 不在 Phase 4（roadmap 未列；PUT 面的消费者在本阶段为测试与后续 Phase 的 Panel）。
