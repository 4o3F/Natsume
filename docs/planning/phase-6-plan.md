# Phase 6 执行计划：Session & Home

> 状态：`DRAFT-PLAN`（2026-08-16 起草）
> 适用：Phase 6 启动时提升为 `docs/gates/phase-6-status.md` 的启动分解基线，届时按最新事实修订
> 权威来源：[路线图](../roadmap.md) §Phase 6 与 G6 覆盖、[契约](../contracts.md) §10/§12、[状态与执行模型](../state-and-execution.md) §7、[ADR-0035](../adr/0035-session-home-and-desktop-cycle.md)、[支持平台](../supported-platform.md) capability 清单
> 前置：Phase 5 关闭（Caddy 数据面稳定后才谈会话）。Session Oneshot 仅 live socket；`HOME_RESET` 是 Converge（`home_epoch`），不经 Device journal。

本文件是计划，不是完成声明。Phase 6 是**工作量最大的未开工阶段**——`org.natsume.Device1` 服务端 7 method + 2 signal 全缺，`Privileged1` 10 方法只实现 1，Session Agent 只有骨架。

## 1. 阶段目标与边界

**结果**：包自带的 XDG Autostart 直接拉起常驻隐藏 Session Agent；Agent 经本地 typed D-Bus 与 Daemon 通信，受 logind session 校验、owner-only singleton 与 lease 约束；WSS lock / unlock / terminate / open_binding_prompt 为 Oneshot（当前 session，无 SessionTarget）；固定 contest user；选定唯一 Home backend；`HOME_RESET` 按 `home_epoch` Converge，不拆 daemon WSS。

**非目标**：overlay UI（ADR-0035 明令当前不实现，未来需新 ADR）；任何 session 动作触碰 Caddy（G6 以「lock/unlock 的 Caddy 调用数为 0」为通过条件）；systemd user unit（四处 CI 断言禁止）。

## 2. 入场检查

| # | 检查项 | 依据 | 阻塞范围 |
|---|---|---|---|
| E1 | **Home backend 定案**（OverlayFS vs staged copy），并把判据与证据写入 platform evidence | ADR-0035：deployment 只选一个、**runtime 不静默 fallback**；roadmap 风险表列「限时定案」 | 阻塞 WP6/WP7 |
| E2 | `supported-platform.md` 的 lock API 表述与 G0 裁定同步（已定案 logind `LockSession`，该文件两处仍写「Phase 6 限时定案」） | 文档漂移，见 §6 D1 | 阻塞 WP4 |
| E3 | 固定 contest user 的创建者定案（镜像提供 vs 包创建；UID 策略、home 路径与 backend 的关系） | D-Bus policy 已假定 `user="contest"` 存在，而 `sysusers.d` 不创建它 | 阻塞 WP1/WP6 |
| E4 | `HomeEpoch` 的 Server 侧分配机制冻结（存储位置、单调保证、与 `commands` 的关系、溢出规则） | ADR-0035 要求 Server 分配且严格单调，但 `domain-model` 无对应列 | 阻塞 WP7 |

## 3. 已冻结事实（不得重新设计）

### 3.1 Current-session Oneshot（ADR-0035）

WSS `lock_session` / `unlock_session` / `terminate_session` / `open_binding_prompt` 是 Oneshot：仅 live socket；离线丢弃；重连不重放。目标 = 该 Device 唯一当前 graphical session。空 body，**不**携带 `SessionTarget` / `session_instance_id` / `session_epoch`。Client 在动作开始时捕获本地 logind session identity，并在 privileged effect 前重验；期间 session 被替换则返回 `SESSION_CONTEXT_STALE`，不得 retarget。Unlock 不从 Observed 读取 `expected_lock_command_id`。`open_binding_prompt` 无 TTL、无 `prompt_message_id`；Device 打开 binding-prompt screen 即 `CommandStatus` `SUCCEEDED`。现场确认绑定不是该 Command 的成功，而是 Device 发起 `BindingRequest{seat_code}` → Server `BindingResult{state,error_code}`；每个 active session 同时最多一个 prompt/request，以连接内顺序关联，无 `binding_request_id` 或 capability token。`BindingResult` 不携带 occupancy `binding_id`。`CommandState` 只有 `SUCCEEDED` | `FAILED`；若副作用后断线丢失结果，Server 终态为 `outcome_unknown`，不重放。本地 Agent UI 仍校验 logind session、UID 与 Agent lease；stale Agent / stale UI action / expired lease 返回稳定错误。Agent crash 或 lease expiry **不增加 authority**，也不能 unlock 后续 session。session action **不调用 Caddy、不修改 Caddy state/config、不进入数据面状态页**。使用冻结镜像的 **native session lock**。一台 Device 一名选手、autologin；选手不知道 unlock 密码。

### 3.2 Home 事务（ADR-0035 的 2026-08-14 修订）

`HOME_PREPARE` + `HOME_CLEAN` 已合并为单一 **`HOME_RESET`**（Converge，键 = `home_epoch`）。本地 Prepare/Activate/Recover/GC 对同一 epoch 必须可重入；已完成则为 success/no-op；`HOME_EPOCH_STALE` 仅当 epoch < 已完成 epoch；重试不得 bump epoch。`HOME_RESET` **不拆 daemon WSS**。中断经本地状态文件 + `RecoverHomeInstance` 恢复，不靠 Command durability。证明 mount/copy 与 ownership safety 后才能启动受管图形会话。reset 是 **operator-present 受控事件**，不因 session replacement 隐式发生。

### 3.3 Agent 形态（ADR-0035）

包在 `/etc/xdg/autostart/org.natsume.SessionAgent.desktop` 安装 **system XDG Autostart**，直接启动同一 resident binary（`Exec=… --autostart`、`NoDisplay=true`、无 `OnlyShowIn`）。Agent **先验证当前 logind session 与 owner-only singleton，再服务本地 UI**；常驻 hidden，typed snapshot 到达时 lazy create/present。GUI 为 **build-time Slint + winit + Skia**（1.15.1，feature 与 runtime closure 已冻结并实测：直接 ELF NEEDED 7 项、二进制 11,734,952 字节、冷启动至 resident marker 59 ms）。Agent **不拥有** password、Device Token、Gateway key、Server client、Caddy 或通用 privileged capability。

### 3.4 本地 D-Bus 冻结面（[契约](../contracts.md) §10）

**Daemon ↔ Agent**：UI snapshot 只含展示所需数据，**不含 password、token、certificate private material、Server 凭据或任意 HTML**；view kind 与 action 为封闭 enum；调用校验 UID/PID/logind session 与当前 session 身份；陈旧 Agent 重放被拒；Agent 退出致 lease 过期，不授予额外权限；**lock/unlock 不调用 Caddy adapter**。

**Daemon ↔ Helper**：方法按 capability 命名，参数必须是封闭 enum、规范化 ID、Helper 内重新派生或 allowlist 校验的路径/UID、明确 epoch，**无 secret**；Helper 不接受 Server/WSS request 的原始对象；**禁止 `execute(request)` / `run_action(name, args)` 一类通用入口**（[仓库布局](../repository-layout.md) §5）。

### 3.5 稳定码（[契约](../contracts.md) §12）

session：`SESSION_CONTEXT_STALE`、`SESSION_UNAVAILABLE`、`SESSION_ACTION_UNSUPPORTED`、`SESSION_STATE_CONFLICT`；home：`HOME_EPOCH_STALE`、`HOME_OPERATION_FAILED`。**注意**：ADR-0036 的 pre-release baseline window 已随 G0 关闭，新增稳定码须走 §13 兼容性论证。

## 4. 现有资产（实现落点）

| 资产 | 状态 |
|---|---|
| Slint `ui::apply` + `session_agent.slint` | **已实现但极简**：单一 600×360 通用窗口，8 个 `SessionScreenKind` 仅以文本标签区分，无独立视图 |
| `--autostart` 常驻循环 | **骨架**：接受单参、建 tokio runtime、重试打 resident marker、跑事件循环 |
| logind 验证 / lease 续期 | **缺失**，代码内明写「remain owed to Phase 6」，须在事件循环初始化前执行 |
| Agent 侧 D-Bus | **缺失**：`zbus` 已声明但零使用 |
| singleton 锁 | **缺失（仅常量）**：`SESSION_AGENT_SINGLETON_RELATIVE_PATH`、`SESSION_AGENT_AUTOSTART_MODE` 已定义 |
| seat 提交回路 | **桩**：只 `tracing::info!`，注释写「Phase 6 wires the typed D-Bus submission」 |
| `local-control-api` 值类型 + proxy | **已实现**：19 结构体 + 11 枚举；`Device1` proxy（7 method + 2 signal）、`Privileged1` proxy（10 method）；注释明写「本 crate 有意不含 Caddy 方法」 |
| D-Bus XML | **手工维护**，仅被 roxmltree 测试消费；无 codegen、无服务端 |
| `Device1` 服务端 | **完全缺失**（全 workspace 无 `#[zbus::interface]`） |
| `Privileged1` 服务端 | **10 缺 9**：只有 `collect_hardware_candidates` |
| proto session/home 面 | Oneshot 空 body，无 `SessionTarget`；无 Heartbeat protobuf（WS ping/pong）；`OpenBindingPrompt` 空 body，Seat 在无 request ID 的 `BindingRequest.seat_code`。Observed slim：`applied_hash`、完整 `CredentialObservation{binding_id,account_id,credential_revision,state}`、`gateway_state`、`gateway_certificate_fingerprint`、`session_state`。 |
| proto 命令体语义校验 | `LockSession` / `UnlockSession` / `TerminateSession` / `OpenBindingPrompt` 为空 body。校验 `ResetHome.home_epoch`；同 epoch 可重入。 |
| session 集成测试 | **骨架**：`session_lock_epoch.rs` 仅构造并回读三字段；`session_agent_platform.rs` 五测试全为静态断言 |
| 打包 | XDG desktop entry、两份 D-Bus policy（`Device1` 由 `natsume` own / `contest` send；`Privileged1` 由 `root` own / `natsume` send，均 default deny）、GUI 启动 runbook（67 行，含 9 个诊断条件与安全禁用清单）、无 user unit 的四处 CI 断言 |
| Home / browser policy 内容 | **仅占位 README**，实际模板不存在 |

## 5. 工作包分解（候选基线）

依赖序：WP1 → WP2 → WP3 → WP4 → WP5 → WP6 → WP7 → WP8。

### WP1：Agent 启动纪律（logind + singleton + lease）

- 目标：`--autostart` 进程在服务任何 UI 前，验证自己属于当前 logind graphical session、确认 owner-only singleton、向 Daemon 注册并开始续 lease；失败以 typed 稳定码退出并留可诊断日志。
- 冻结项：singleton 锁语义（`$XDG_RUNTIME_DIR/natsume/session-agent.lock`，flock 独占 + 持有期）；logind 校验用的字段（session id / seat / UID / PID → session 反查）；**lease TTL 与续期间隔**（当前只有 `expires_at_unix_ms` 字段无常量，按契约纪律应为硬编码 Rust 常量并记入契约，**D2**）；Agent 日志路由（Phase 0 遗留观察项：XDG autostart 实例 stderr 未落 `~/.xsession-errors`，**D3**）。
- 测试：非当前 session 启动被拒；重复实例被 singleton 拒绝且首实例存活；lease 过期后动作被拒；崩溃后 lease 过期不遗留 authority。

### WP2：`org.natsume.Device1` 服务端

- 目标：Daemon 侧实现 7 method（`RegisterSessionAgent`、`RenewSessionAgentLease`、`GetSessionUiSnapshot`、`SubmitSessionUiAction`、`SubmitBinding`、`AcknowledgePresentation`、`UnregisterSessionAgent`）与 2 signal（`SessionUiSnapshotChanged`、`SessionLeaseRevoked`）。
- 冻结项：每方法的 UID/PID/session/epoch 校验点；snapshot 推送节奏（信号驱动 vs 轮询）与 `ui_revision` 单调规则（**D4**）；XML ↔ Rust proxy 的同步机制升级（当前只有子串检查，无法捕获参数挂错方法；建议逐方法签名相等断言或 codegen，**D5**）。
- 测试：全部方法的陈旧 epoch / 非法 UID / 过期 lease 拒绝矩阵；signal 投递；XML 与实现签名逐方法相等。

### WP3：Agent UI 接线与生产 GUI

- 目标：typed snapshot → lazy create/present；8 个 `SessionScreenKind` 的真实视图；seat 提交回路走真实 D-Bus。`OPEN_BINDING_PROMPT` 在 screen 打开时即以 `CommandStatus` `SUCCEEDED` 结束；志愿者确认后的 seat 提交是 `BindingRequest`，本地取消只关闭 UI，不是该 Command 的成功。
- 冻结项：每屏布局与文案来源（message-catalog ID，禁任意 HTML）；本地化与主题；`UiPresentationState` 的上报口径（含 `PresentedUnfocused`）。
- 测试：`ui_probe` 扩展到全部屏形态；CJK 与 HiDPI 回归；focus denied 的可观察结果；display lost 与 crash recovery。

### WP4：Session lock / unlock / terminate

- 目标：Oneshot `lock_session` / `unlock_session` / `terminate_session` 的 live-socket Device 执行，经唯一有界 single-consumer executor 严格按到达顺序调度，并经 `Privileged1.RequestDesktopLock/Unlock` 落到动作开始时捕获且 effect 前重验的 logind session。离线丢弃，重连不重放。
- 冻结项：`SESSION_ACTION_UNSUPPORTED` 的触发条件（镜像不支持时）；Unlock 不以 Observed `active_lock_command_id` 为 fence；**Caddy 调用计数器的实现形态**（G6 要求「lock/unlock 的 Caddy 调用数为 0」的可测证据；计数器放哪一层、真实镜像上如何采集，**D6**）。
- 测试：stale Agent 动作被拒；Oneshot 离线丢弃；replacement session 不被旧 Agent 控制且不 retarget；副作用后断线记录 `outcome_unknown`；并发抵达严格串行；lock/unlock 全流程 Caddy 调用计数为 0；terminate 后状态收敛。

### WP5：proto 命令体语义校验补齐

- 目标：wire 已无 `SessionTarget` / lock epoch / `expected_lock_command_id`；`ResetHome.home_epoch` 存在且同 epoch 可重入。
- 冻结项：校验位置——补进 `device-protocol::validation`（跨进程共享）还是 Device 侧 application 层（**D7**）。

### WP6：Home backend 与模板

- 目标：按 E1 定案实现唯一 backend；产出 versioned Home template 实际内容（当前仅占位 README）；固定 contest user 的 home 布局。
- 冻结项：template 版本号与 `home_template_revision` 的对应；mount/copy 的 ownership 证明方式；磁盘空间不足的 fail-closed 路径。
- 测试：backend 性能与恢复基线；disk-full；ownership 错误；reboot 中断。

### WP7：`HOME_RESET` 事务与多次重置

- 目标：Server 分配 `home_epoch` → Device 执行可重入 Prepare/Activate；中断走 `RecoverHomeInstance`；同 epoch 重试不得 bump；`HOME_RESET` 不拆 daemon WSS。
- 冻结项：`home_epoch` 存储（E4）；reset 与当前 session 的交互（reset 前是否必须先 terminate，由谁保证，**D8**）；reset 完成前不得启动受管会话的强制点。`ResetHome` 本来就不携带 `SessionTarget`。
- 测试：连续多次重置；同 epoch 重试为 success/no-op；epoch < 已完成 → `HOME_EPOCH_STALE` 且零副作用；中断经 RecoverHomeInstance；无法证明 ownership → `HOME_OPERATION_FAILED` 且不启动会话；daemon WSS 在 reset 期间保持。

### WP8：capability 清单复跑与 G6 证据

- 目标：在当期镜像上复跑 ADR-0035 全清单，尤其 Phase 0 明确「Phase 6 接线后必须复跑」的五项：lock/unlock、terminate/replacement、display lost 与 crash recovery、logind 识别、**lock/unlock 的 Caddy 调用数**。
- 说明：Phase 0 条目 11 已通过的部分（XDG 直启、resident/hidden、lazy window、CJK、HiDPI、focus 可观察、无 user unit）在本阶段复跑以覆盖接线后的行为；中文 IME 的全清单复跑并入下一次实质镜像 bump。

## 6. G6 覆盖项 → WP 映射

| G6 主题 | WP |
|---|---|
| 当期镜像 capability 清单全项 | WP8 |
| 中文 IME / HiDPI / focus denied | WP3 + WP8 |
| display lost 与 Agent crash | WP1（lease）+ WP3 + WP8 |
| 无 user unit | 既有 CI 断言 + WP8 |
| stale Agent / Oneshot 离线丢弃 | WP2 + WP4 |
| lock/unlock 的 Caddy 调用数为 0 | WP4（计数器）+ WP8 |
| Home reset/fault/reboot 与连续多次重置 | WP6 + WP7 |

## 7. owner 决策点

| # | 决策 | 影响 |
|---|---|---|
| D1 | 同步 `supported-platform.md` 的 lock API 表述（已定案 logind `LockSession`）；`RequestDesktopLock/Unlock` 是否即 logind 调用、是否需 Xfce 回退 | 文档一致性 + WP4 实现 |
| D2 | lease TTL、续期间隔、snapshot 推送节奏的常量值 | 契约需登记（硬编码常量纪律） |
| D3 | Agent 日志路由 | 现场诊断能力 |
| D4 | `ui_revision` 单调规则与 snapshot 推送模型 | D-Bus 面语义 |
| D5 | XML↔Rust 同步机制升级（codegen vs 逐方法签名断言） | 契约 §14 一致性要求 |
| D6 | Caddy 调用计数器形态 | G6 通过判据的可测性 |
| D7 | proto 命令体语义校验的位置（共享 crate vs 各端） | 双份规则风险 |
| D8 | `ResetHome` 与当前 session 的交互（是否先 terminate） | 不引入 SessionTarget |
| D9 | session 稳定码：把 runbook 的 9 个条件折叠进现有 4 码，还是扩 registry（后者须走 §13 兼容性论证） | 诊断粒度 vs 兼容承诺 |
| D10 | `slint-build` 版本改走 workspace 继承；`serde`/`zbus` 未使用依赖的去留；`deny`/`forbid(unsafe_code)` 不一致 | 构建卫生 |

## 8. 跨切风险

| 风险 | 控制 |
|---|---|
| Home backend 迟迟不定案 | E1 设硬期限；WP1–WP5 不依赖 backend，可先行 |
| `Device1` 服务端工作量被低估（7 method + 2 signal 从零） | WP2 单独排期并先做 XML↔实现同步机制（D5），避免后期漂移 |
| 镜像 bump 使 capability 结论失效 | WP8 结论登记 snapshot 标识；bump 触发全清单复跑 |
| session 稳定码不足导致诊断黑洞 | D9 在 WP2 前定案，避免后期扩码走兼容论证 |
| GUI 设计缺失拖住 WP3 | 先做 typed 数据面与 lazy present，视图迭代不阻塞 D-Bus 契约 |
