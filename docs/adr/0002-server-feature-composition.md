# ADR 0002：Server 按 Feature 组装与运行期所有者

- 状态：Accepted
- 日期：2026-11-02
- 架构依据：[docs/architecture.md §10.4](../architecture.md#104-feature-组装)、[§10.6](../architecture.md#106-分层调用路径与事件归属)、[§11.1](../architecture.md#111-composition-root)。本文记录决策背景与取舍，不建立平行架构权威；最终结构以架构文档为准。

## 背景

Server 采用手写构造注入，但 `ServerState` 同时承担四项职责：加载进程资源（Vault、Origin CA、Print/Photo 存储、Journal writer）、连线业务依赖、作为 HTTP 唯一状态入口，以及构造测试环境。每接入一个 feature 都要同时修改 `server_state.rs`（字段、构造、accessor、错误变体）、`commands/error.rs` 的逐变体映射、`test_error.rs`、`http.rs` 和测试构造器。Photo 接入改动了 7 个中心文件，Journal 接入改动了 9 个；`server_state.rs` 在 2026-10-19 至 10-30 间由 208 行增至 340 行。

由此产生了三类具体后果：

- `from_parts` 用 `(logos, photo)` 元组控制参数数量，`DeviceControl::new` 依赖 `allow(clippy::too_many_arguments)`；
- 13 个 handler 文件中有 10 个只使用一个组件，却都能取得全部组件和 Journal writer 的所有者，函数签名看不出真实依赖；
- 局部测试为构造完整 `ServerState` 而初始化 Vault、Gateway、Print 与 Photo 存储。

此外，关闭编排直接写在 `serve::run_until` 中，并存在两个缺陷：future 被取消时 HTTP serving task 不会停止；HTTP 与后台同时失败时，后台错误既不返回也不记日志，违反 §11.1。

## 决策

| 决策 | 采纳结果 |
| --- | --- |
| D1 Feature 组装层 | 新增 `feature/` 模块，位于组合层。每个 `<Name>Feature` 调用组件自己的 concrete constructor，构造该 feature 的 HTTP 窄状态，并调用 `http::handler::<name>` 的路由函数得到已绑定状态的路由表（`Vec<(&'static str, MethodRouter)>`）。组件层不依赖 `feature/` 或 `http/`，依赖只向下 |
| D2 基础设施 | `Infra` 只持有 `Database`、`Arc<VaultSession>`、Journal emit 句柄和 `ServerLifecycle`，不持有业务组件或协调器，不是 Components wrapper |
| D3 Feature 间依赖 | 通过 `load` 的具名参数显式传递已构造的共享句柄（例如 Photo 接收 Contest 的 `Arc`），由编译器检查。不提供依赖查找 trait、注册表、动态集合或模块生命周期 trait |
| D4 认证挂载 | 顶层按 public、operator、source、client 四个 access class 合并 feature 路由，组装时对受保护分组中的每条 `MethodRouter` 调用 `route_layer` 挂载认证，使方法不匹配时先返回 405，不经过认证；认证中间件只持有窄状态（Operator 或 Print 的 `Arc`）。审计、completion 与 HEAD 拒绝仍包在 `/api/v2` 外层，顺序不变 |
| D5 运行期所有者 | `ServerRuntime` 持有 `ServerLifecycle`、`Arc<DeviceControl>`、Journal writer、HTTP serving task 与 `BackgroundTasks`，负责既有的固定关闭顺序；HTTP 只取得 Journal emit/reader 句柄。`serve` 只做命令适配 |
| D6 启动错误 | 可失败的 feature `load` 返回其组件自身的 Snafu 错误，由组合层 `StartupError` 中归属该 feature 的变体包装（如 `PrintStorage`、`PhotoStorage`），不另设 feature 级错误类型；`CommandFailure` 只以一个变体包装 `StartupError`，不再逐项镜像 |
| D7 共享实例 | 每个共享组件在进程内只构造一次，跨 feature 只传递同一个 `Arc`，保持 Print、Photo、Contest 的预算、信号量、缓存以及唯一的 `DeviceRegistry` |
| D8 公开入口 | `natsume_server::router` 与 `commands::run_until` 没有 workspace 外部调用者；收窄为 `pub(crate)` 后在 crate 内也无调用者，因此直接删除 |

已删除 `ServerState`、`AppState` 与旧构造器链，各测试使用 feature 或组件的 fixture；HTTP 集成测试通过共同的 `assemble` 取得共享实例。新增 feature 的预期改动范围是：自身模块、组装入口中的一行加载和一行路由合并、`StartupError` 的一个变体，以及架构文档中该 feature 的章节。

## 备选方案

- 仅整理构造参数与错误映射：消除元组和镜像错误，但 HTTP 仍取得全量状态，新增 feature 仍需改中心文件，不足以解决问题。
- 保留 `ServerState`，用 axum `FromRef` 提供窄状态：依赖在签名中可见，但顶层加载与容器仍随 feature 增长。它的窄状态正是本方案各 feature 的 HTTP 状态，因此作为中间形态没有单独保留的价值。
- `trait ServerModule` 加注册表，或 `HasPrint`/`ComponentProvider` 一类取件 trait：只是把容器换成依赖查找接口，仍属 service locator，并违反架构文档对 DI framework trait 和动态注册的禁止，否决。

## 后果与验收

- 组装入口仍需要了解完整依赖图，`DeviceControl` 的多组件依赖是真实业务耦合，会保留为入口中最长的部分；本决策约束的是每个 feature 的修改范围，不追求消除组装代码。
- 已实施完成：生产入口为 `feature::load`，运行期资源由 `ServerRuntime` 持有，所有业务路由经 feature 绑定窄状态后交给 `http::mount`，health 由 HTTP 骨架固定注册。
- 对外 HTTP 契约（路径、方法、状态码、错误码、缓存头、请求体限制、HEAD 拒绝）、认证→审计→completion 语义、journal 事件序列及关闭顺序保持不变，由现有路由与关闭测试守护。
- 纪律约束：feature 不得重新聚合跨 feature 的句柄容器。代码审查须检查 `feature/` 中不出现持有多个无关组件的汇总结构。
