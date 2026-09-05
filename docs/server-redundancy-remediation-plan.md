# Server 冗余修正方案

> 状态：待实施
>
> 性质：临时执行方案，不是第二份目标架构文档。目标架构仍以
> `docs/architecture.md` 为准；本方案验收完成后，把最终所有权和目录变化同步回
> `docs/architecture.md`，随后删除本文件，由 Git 历史保留实施记录。

## 1. 目的与范围

本方案修正已确认的四类 Server 冗余：

1. `DeviceControl` 同时承担运行时协调、跨组件应用编排和 HTTP response model，且其
   方法反向接收拥有它的 `ServerState`；
2. Contest 三个只读查询被拆成多层、多个单函数文件；
3. HTTP handler 和 `ApiError` 重复手工序列化 JSON；
4. 未使用依赖、不可达错误、组件内部 row getter 和错误分类 getter 等失效或微型包装。

本方案不处理配置模块和遗留公开命令入口，也不借清理增加新功能。

## 2. 必须保持的行为与边界

修正只改变代码所有权和内部表达，不改变以下契约：

- HTTP route、method、status、JSON 字段、枚举字符串、错误码、Cookie 和 Content-Type；
- OpenAPI 和生成的 Web schema；
- Proto wire format、Device snapshot 内容和处理顺序；
- Device 的单 current lease、旧 session fencing、bounded mailbox 和 outbound backpressure；
- `ClientState` 完整校验先于任何组件写入；
- authority mutation 的 commit → fence → evict 顺序，以及请求取消后任务继续完成的语义；
- Dirty 只在业务提交后发送，且不携带业务状态；
- 各组件的数据库 mutation ownership、transaction 和错误边界；
- Contest 查询的排序、字段和值域检查；
- 非 canonical Device ID 继续由 HTTP path parser 返回
  `device_id_not_canonical_uuid_v7`。

禁止为了本次修正引入：

- `Service`、`Repository`、`UnitOfWork` 或 DI trait；
- 第二个依赖容器、`Components` wrapper 或 service locator；
- 通用 response builder、JSON helper、row trait 或 error-classification trait；
- `common`、`utils`、`shared-models` 等新模块；
- 兼容 adapter、feature flag 或双实现迁移期。

## 3. 修正后的所有权

```text
HTTP / WSS
    │
    ▼
ServerState application methods
    ├── concrete business components
    └── DeviceControl runtime state
            ├── DeviceRegistry
            ├── enrollment approval mutex
            └── authority fence / eviction

DeviceActor
    ├── 启动时取得 Weak<ServerState>
    └── mailbox 只传 Attach / ClientState / Dirty / Evict / Disconnect 等事件数据
```

职责划分如下：

| 所有者 | 保留职责 | 不再承担 |
|---|---|---|
| `ServerState` | 跨组件查询、mutation 后通知、authority mutation 与 runtime fencing 的应用编排 | HTTP schema 和 JSON 序列化 |
| `DeviceControl` | registry、审批串行门、attach/connection observation、fence/evict 原语 | 接收 `ServerState` 的公开应用方法 |
| `DeviceActor` | lease 事件串行化、snapshot reconcile、Dirty materialize、observation 保存 | 在每条事件中接收 `Arc<ServerState>` |
| `device_control` read model | 已校验 Actual、target、connection 和 convergence 的中立 crate 内模型 | `Serialize`、`ToSchema`、`*Response` 命名及 HTTP 类型依赖 |
| HTTP device handler | response DTO、`Serialize`、`ToSchema` 和从中立 read model 到 response 的转换 | 组件查询和 convergence 业务编排 |

`DeviceControl` 仍是有明确状态所有权的 concrete runtime object，不删除它，也不把 registry
暴露给 transport。需要删除的是“`DeviceControl` 方法再接收其父容器”的双重 façade。

## 4. 实施批次

批次按依赖和风险排序。每个批次独立编译、测试和审查；前一个批次未通过时不进入下一个。

### R0：冻结当前契约

实施前记录当前工作区，避免覆盖无关改动，尤其是已有的 `Cargo.lock` 和
`docs/architecture.md` 修改。

先运行：

```bash
cargo test -p natsume-server --all-features --locked
cargo clippy -p natsume-server --all-targets --all-features --locked -- -D warnings
```

确认现有测试至少覆盖：

- health 成功响应的 status、body 和 Content-Type；
- `ApiError` 的 status、body、Content-Type 和日志分级；
- Device 单条与批量 convergence 的一致性；
- authority mutation、fence、evict 和 request cancellation；
- Dirty、ClientState、stale lease 和 outbound backpressure；
- Contest 三个列表的字段映射和确定性排序。

若最后一项缺少行为测试，只补三个现有查询的最小 characterization test，不预建通用
Contest 测试框架。

### R1：删除失效依赖和不可达错误

#### R1.1 `serde_json_canonicalizer`

改动：

- 删除 workspace `Cargo.toml` 中的 `serde_json_canonicalizer`；
- 删除 `server/Cargo.toml` 中的对应 workspace dependency；
- 让 Cargo 按实际依赖关系更新 `Cargo.lock`，不手工假设 lockfile 中对应 package 一定消失；
- 修改前后都先检查 `Cargo.lock` 的已有 staged/unstaged diff，禁止覆盖用户改动。

验收：

```bash
rg -n 'serde_json_canonicalizer' --glob '!Cargo.lock' .
cargo tree -p natsume-server | rg 'serde_json_canonicalizer'
```

两条检查都应无输出。

#### R1.2 `DeviceError::InvalidDeviceId`

改动：

- 删除 `server/src/component/device/types.rs` 中从未构造的
  `DeviceError::InvalidDeviceId`；
- 删除 `ApiError::from_device` 对该 variant 的映射；
- 删除只验证这条不可达映射的 HTTP error test case；
- 保留 `http/handler/device.rs` 的 path-level `DeviceId::parse` 和
  `device_id_not_canonical_uuid_v7` 响应。

验收：

```bash
rg -n 'InvalidDeviceId|device_invalid_device_id' server/src
```

结果必须为空，同时非法 Device path 的 HTTP 测试继续通过。

### R2：删除组件内部微型包装

#### R2.1 Gateway 和 Binding persisted row

这些 row 仍是 DB adapter 与组件领域转换之间的边界，不能泄漏到 HTTP、Actor 或其他组件；
只删除没有不变量的 constructor/getter façade。

改动：

- `PersistedGatewayRow`、`PersistedNegotiationRow`、
  `PersistedRejectedSubmissionRow`、`PersistedSubmissionSeatRow`、
  `PersistedBoundTargetRow` 和 `PersistedBoundContextRow` 保持组件级可见；
- row 字段使用恰好允许 owning component 读取的 `pub(super)`；
- DB query 直接使用 struct literal 构造 row；
- 领域转换直接读取字段，并在消费位置调用 `as_deref`、整数转换和值域检查；
- 删除单纯返回字段、slice 或布尔值的 getter；
- 保留 `from_persisted` 等真正执行 UUID、revision、文本和组合完整性验证的转换。

不把 row 改成全 crate 公共类型，不把 persisted row 直接作为 component 或 HTTP 返回值。

#### R2.2 Gateway error 分类

改动：

- 删除 `GatewayIssuerError::is_invalid_csr`；
- 删除 `GatewayIssuerError::is_trust_root_mismatch`；
- 删除 `GatewayLoadError::is_trust_root_mismatch`；
- `map_persisted_issuer_error`、`map_load_error` 和
  `map_gateway_load_error` 改为对 enum 的穷举 `match`；
- issuer 测试使用 enum equality 或 `matches!`，不恢复分类 getter。

穷举映射必须保持当前语义：

- persisted `InvalidCsr` → `GatewayError::InvalidPersistedFacts`；
- 其他 issuer failure → `GatewayError::IssuanceFailed`；
- load-time `TrustRootMismatch` 单独映射；
- 其他 load failure 统一映射为 Origin CA failure。

验收：新增 error variant 时上述 `match` 必须触发非穷举编译错误；组件测试保持通过。

### R3：合并 Contest 只读路径

目标目录：

```text
server/src/component/
  contest.rs
  contest/db.rs
```

删除：

```text
server/src/component/contest/account.rs
server/src/component/contest/binding.rs
server/src/component/contest/seat.rs
server/src/component/contest/db/accounts.rs
server/src/component/contest/db/device_bindings.rs
server/src/component/contest/db/seats.rs
```

改动：

- `SeatFacts`、`AccountFacts`、`BindingFacts` 及其 transport-facing
  `into_parts` 保留在 `contest.rs`；
- `ContestComponent::list_seats/list_accounts/list_bindings` 直接调用
  `db::list_seats/list_accounts/list_bindings`；
- 三个 Diesel 查询合并到 `contest/db.rs`，函数名表达所查询的集合；
- `ContestError` 和 `PersistenceError` 映射继续留在组件父文件；
- `device_id` 的 persisted-data 校验继续发生在 DB row → component facts 边界；
- 所有 test-only 代码保持在 `contest.rs` 唯一的文件级 `#[cfg(test)] mod tests` 中。

不得把 Contest 查询移到 HTTP，不增加 Repository，不把三个查询合并为带参数的通用查询器。

验收：

- 三个 endpoint 的 JSON、排序和错误映射不变；
- 目标目录只剩 `contest.rs` 与 `contest/db.rs`；
- HTTP 和其他组件仍只能通过 `ContestComponent` 读取 Contest facts。

### R4：统一使用 Axum `Json`

#### R4.1 成功响应

改动：

- health 直接返回 `Json(HealthResponse { status: "ok" })`；
- Contest 三个列表直接返回 `Json(response)`，删除
  `current_facts_response`；
- Import 使用 `(StatusCode::CREATED, Json(response))` 和
  `Json(ImportPendingResponse { ... })`，删除 `json_response`；
- Session 先对 `Json(SessionResponse)` 调用 `into_response()`，再插入可选
  `Set-Cookie`；
- 删除随手工序列化存在的 `serde_json::to_string/to_vec`、显式 Content-Type 和
  serialization panic。

#### R4.2 错误响应

改动：

- `ApiError::into_response` 保留当前 4xx `warn`、5xx `error` 和 redacted `cause`；
- 日志完成后返回 `(self.status, Json(ErrorResponse { ... }))`；
- 删除错误响应的手工 `to_vec`、Content-Type 和 panic；
- 删除与 `ApiError::new` 字段完全相同的 `import_error`，Import 映射直接调用
  `new`；
- 不新建项目级 JSON response helper。

验收必须逐项比较改动前后：

- health：`200`、`{"status":"ok"}`、`application/json`；
- Contest：`200`、数组根节点、字段和顺序；
- Import create：`201`；Import read：`200`；
- Session login：`200` 且保留唯一 `Set-Cookie`；session read 不新增 Cookie；
- `ApiError`：status、`title/status/code` body、Content-Type 和日志等级；
- `just api` 后 OpenAPI 与 Web generated schema 无语义 diff。

### R5：修正 Device Control 边界

这是唯一涉及运行时所有权的批次，必须在前述机械清理全部验收后单独实施和审查。

#### R5.1 分离中立 read model 与 HTTP DTO

`device_control` 保留：

- 完整 Actual 的 wire validation；
- 当前 lease 的已验证 Actual observation；
- target/actual comparison 和 convergence status 计算；
- 单 Device 与批量 Device status 查询的一致逻辑；
- component error 的中立枚举。

但其中的类型：

- 不使用 `Serialize` 或 `ToSchema`；
- 不以 `Response` 命名；
- 不导入 `crate::http`；
- 不复用 `SessionControlTargetResponse` 等 handler DTO。

`http/handler/device/convergence.rs` 及其按资源拆分的子文件拥有：

- `DeviceConvergenceResponse` 和五个资源的 response DTO；
- `Serialize`、`ToSchema`、serde rename/deny rules；
- 从中立 convergence read model 到 HTTP response 的纯转换；
- `DeviceConvergenceError` 到 `ApiError` 的 transport 映射。

中立 read model 的 crate-visible 字段本身就是 transport 所需的现有契约，不再为每个字段
增加一层无不变量 getter。转换只改变表示，不重新查询组件或重新计算 convergence。

#### R5.2 把跨组件用例归还 `ServerState`

HTTP、WSS 和其他 caller 改为直接调用 `ServerState` application method，例如：

- `read_device_convergence`；
- `read_device_status` / `read_all_device_statuses`；
- `dirty_device` / `dirty_all_devices`；
- `disable_device` / `revoke_device`；
- `approve_enrollment`。

具体命名以现有用例词汇为准，不建立统一 command dispatcher。

为避免扩大 `server_state.rs`，跨组件 Device 用例实现在
`server/src/server_state/device_control.rs` 的 `impl ServerState` 中。该文件是组合根的实现
分片，不定义第二个 state、trait 或 façade。`ServerState.device_control` 字段保持私有；删除
供 transport 使用的 `device_control()` accessor。

修正后不得再出现：

```rust
state.device_control().operation(&state, ...)
```

`DeviceControl` 的应用操作只处理自己的 registry、mutex、fence 和 observation，不接收
`&ServerState` 或 `Arc<ServerState>`。唯一例外是 actor 首次创建原语接收一次
`Weak<ServerState>` 作为进程生命周期绑定；它不能被保存为强引用，也不能出现在后续
mailbox event 中。

#### R5.3 从 Actor event 中移除容器

Actor 第一次由 registry 创建时接收 `Weak<ServerState>`，在处理需要组件的事件时临时
upgrade；upgrade 失败表示进程组合根已销毁，Actor 结束。不得让 Actor 持有长期
`Arc<ServerState>`，否则会形成 `ServerState → DeviceControl → Registry → Actor →
ServerState` 的强引用环。

事件调整为：

- `ClientState` 只携带 `session_id`、snapshot 和接收时间；
- `Dirty` 不携带 state 或业务数据；
- `Attach`、`Evict`、`ConnectionState`、`Disconnected` 保持现有语义；
- Actor 仍在 authority fence 锁内完成 current ClientState/Dirty 的组件工作；
- 只有 Actor 自己保存最新的已验证 observation。

`tokio::spawn` 包装的 disable/revoke/approve 任务在 task 内持有必要的
`Arc<ServerState>`，保持请求取消后继续完成，但不再通过
`state.device_control().*_inner(&state, ...)` 回绕。

#### R5.4 更新消费者和测试

必须搜索并逐个迁移：

- Enrollment approve；
- Device lifecycle disable/revoke；
- Binding、Session、Home mutation 后的 Dirty；
- Import commit 后的 Dirty All；
- WSS attach、ClientState 和 disconnect；
- Device convergence/list/get；
- `device_control/tests.rs` 中所有直接 accessor 调用。

禁止保留旧 accessor 作为兼容入口。迁移结束后：

```bash
rg -n '\.device_control\(\)' server/src
rg -n 'crate::http|serde::Serialize|utoipa::ToSchema' server/src/device_control.rs server/src/device_control
rg -n 'state: Arc<ServerState>|Dirty \{ state|ClientState \{[^{]*state' server/src/device_control/actor.rs
```

三组检查均应无输出。测试代码也必须使用正式的 `ServerState` application method，避免旧
边界被测试永久保活。

## 5. 验收矩阵

| 问题 | 完成证据 |
|---|---|
| Device Control 双重 façade | transport 不再取得 `DeviceControl`；应用方法不接收父容器；Actor event 不携带 state |
| HTTP DTO 反向依赖 | `device_control` 对 `crate::http`、Serde response derive 和 Utoipa 零引用 |
| Contest 多层转发 | 只剩 `contest.rs` 和 `contest/db.rs`，三个 endpoint 行为不变 |
| JSON 手工包装 | 指定 handler 与 `ApiError` 不再手工编码 response JSON，也没有 serialization panic |
| 未使用依赖 | manifests 和 `cargo tree` 均无 `serde_json_canonicalizer` |
| 不可达错误 | `InvalidDeviceId` 及旧映射为零引用，path-level 错误契约仍通过 |
| Row 微型 getter | row 保持组件内，字段直接读取，领域校验函数保留 |
| Error 分类 getter | 使用穷举 `match`，不存在 `is_invalid_csr/is_trust_root_mismatch` |

## 6. 验证命令

每个批次运行：

```bash
cargo fmt --all -- --check
cargo clippy -p natsume-server --all-targets --all-features --locked -- -D warnings
cargo test -p natsume-server --all-features --locked
git diff --check
```

R4、R5 额外运行：

```bash
just api
git diff --exit-code -- web/openapi/natsume.openapi.json web/src/api/generated/schema.d.ts
bash tools/architecture-scan.sh
```

若 `just api` 首次产生 diff，先区分排序/生成器噪声与真实契约变化；本方案不接受通过提交
生成物变化来掩盖 response ownership 重构。

## 7. 风险与控制

### 7.1 Actor 生命周期

风险：错误使用强 `Arc` 形成引用环，或 `Weak` upgrade 时机导致正常事件提前退出。

控制：Actor 只保存 `Weak`；每个需要组件的事件开始时 upgrade 一次并持有到该事件完成；
增加 ServerState drop 后 Actor 可退出的测试，不增加 shutdown protocol。

### 7.2 Authority fencing

风险：移动 disable/revoke/approve orchestration 时改变锁的持有范围。

控制：以现有 sequence test 为准，保持“取得 fence → component commit → 标记 fenced → 释放锁
→ await evict”；component commit 失败时不得 fence 或 evict。

### 7.3 Convergence 表示

风险：中立 read model 与 HTTP DTO 分离后，字段漏映射或 enum rename 改变。

控制：保留当前 serde 属性和 OpenAPI schema 名；对所有 connection/convergence/resource state
做 response conversion matrix；单条和批量 read 继续共享同一个 convergence builder。

### 7.4 `Cargo.lock` 并发改动

风险：删除 dependency 时覆盖当前已有 lockfile 修改。

控制：实施前后分别检查 staged 与 unstaged diff，只让 Cargo 合并本依赖产生的最小变化；若
无法与现有修改可靠区分，则暂停该子项，不重写 lockfile。

## 8. 明确不做

- 不处理 `config.rs` 的 dead-code surface；
- 不处理 `commands::router/run_until` 等公共入口；
- 不改变 HTTP route 风格或 action endpoint；
- 不改变 Device Control 协议、数据库 schema 或 migration；
- 不顺带重命名所有 component/read model；
- 不为 future Device 数量、并发或多进程部署预建抽象；
- 不把低价值 getter 清理扩展到有验证、格式转换或所有权语义的领域方法。

## 9. 完成与文档收口

全部验收通过后：

1. 更新 `docs/architecture.md` 的 `ServerState`、Device Control ownership、Actor event 和
   Server 目标目录描述；
2. 架构文档只写最终状态，不记录迁移过程；
3. 删除本临时方案文档；
4. 最终 diff 中不得包含协议、数据库、OpenAPI 或 Web generated schema 的非预期变化。
