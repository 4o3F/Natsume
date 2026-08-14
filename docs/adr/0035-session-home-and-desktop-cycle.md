# ADR-0035: Session, Home, and desktop cycle

> Status: `ACCEPTED`
> Scope: Session Agent, session actions, Home lifecycle, and desktop support policy
> Consolidates: ADR-0007, ADR-0015, ADR-0017, ADR-0018, ADR-0027
> Supersedes: consolidated historical records; see [`history-map.md`](history-map.md)
> Superseded by: —

## Context

Desktop session 会因 reboot、logout/login、display loss 或 delayed message 被替换；Seat 或 UID 无法区分旧 Agent 与当前 graphical session。Home reset 同样必须在中断后可恢复，不能在 runtime 静默切换 backend。

Session Agent 必须运行于真实 graphical session，但不能成为 credential、Server、Caddy 或 privileged-control owner。项目每个赛事周期只部署一个最终镜像，未来 image upgrade 可能改变 desktop capability，因此支持边界应按 image capability 冻结，而不是永久承诺多个桌面名称。

## Decision

### Epoch-bound Session

- lock、unlock、terminate、Agent UI action 与 recovery transition 都携带并验证当前 `SessionEpoch`。
- Device 同时验证 logind session、UID、boot/session identity 与 Agent lease；stale Agent、stale UI action、stale epoch 或 expired lease 返回稳定错误。
- Agent crash 或 lease expiry 不增加 authority，也不能 unlock 后续 session。
- session action 不调用 Caddy、不修改 Caddy state/config，也不成为数据面状态页内容。
- 使用 frozen image 的 native session lock；无法取得 focus 时报告可观察结果（如 `VISIBLE_UNFOCUSED`），不采用 desktop-specific focus bypass。

### Home recovery transaction

**2026-08-14 修订：** 面向 Server/operator 的 `HOME_PREPARE` 与 `HOME_CLEAN` Command family 合并为单一 `HOME_RESET`；每次 reset 的 `HomeEpoch` 改由 Server 分配并保持严格单调；中断 reset 的恢复保证实质不变；本地 `PrepareHomeInstance` / `ActivateHomeInstance` / `RecoverHomeInstance` / `GarbageCollectHomeInstance` 分解保持不变，仍只属于实现面，D-Bus surface 保持不变。

- 使用固定 contest user 与 versioned Home template。
- deployment 基于 target safety、recovery 与 performance evidence 只选择一个 Home backend（OverlayFS 或 staged copy），并记录到 platform evidence；runtime 不静默 fallback。
- 每次 `HOME_RESET` 获得由 Server 分配且严格单调的新 `HomeEpoch`；reset 完成并证明 mount/copy 与 ownership safety 后才能启动 managed graphical session。
- 中断的 reset 通过显式状态和可重入步骤恢复，不确定时 fail closed。
- reset 是 operator-present controlled event，不因 session replacement 隐式发生。

### Direct capability-oriented Agent

- Client package 在 `/etc/xdg/autostart/org.natsume.SessionAgent.desktop` 安装 system XDG Autostart，直接启动同一 resident Agent binary。
- Agent 先验证当前 logind session 与 owner-only singleton，再服务本地 UI；常驻 hidden，typed snapshot 到达时 lazy create/present UI。
- GUI 使用 build-time Slint、winit backend 与 Skia renderer；本地 D-Bus contract 使用 closed typed view/action enums。
- Agent 不拥有 password、Device Token、Gateway key、Server client、Caddy 或通用 privileged capability；desktop 差异留在 adapter 后。

### One image per contest cycle

- 每个赛事周期冻结一个项目构建镜像和一个 desktop environment；当前 X11 计划仍需 target evidence 才能成为 `ENV-FROZEN`。
- 每次 image bump 重跑 capability checklist：direct launch、resident/hidden/lazy、logind identity、singleton、中文 IME、HiDPI、focus outcome、lock/unlock、terminate/replacement、display loss、crash recovery 和 lock/unlock 零 Caddy 调用。
- 当前不实现 overlay UI；未来 overlay 需要新 ADR 和 image capability evidence，并且只是 UX，不成为 integrity boundary。

## Alternatives

- 只用 Seat/UID 或 wall-clock timeout：不能阻止 stale process 控制新 session。
- 把 lock 与 Caddy BLOCKED/READY 耦合：混淆 desktop visibility 与 network safety。
- 每 session 创建 Linux user、持久复用 Home 或 backend fallback：削弱 reset、ownership 和 recovery guarantees。
- systemd user unit、bootstrap/run handoff、environment descriptor 或自建 renderer/text stack：增加 graphical lifecycle race 与维护成本。
- GTK/Qt/Electron/runtime interpreter：扩大 desktop runtime 与依赖面。
- 永久双桌面矩阵或永久锁死一个桌面：分别产生无限支持义务或阻断未来 ICPC image upgrade。
- 现在实现 full-screen overlay：需求和 target capability 尚未证实。

## Consequences

### Positive

- stale Agent 和 delayed action 不能控制 replacement session。
- Home reset、crash 与 recovery 有明确 epoch boundary。
- Agent authority、secret exposure 与 GUI runtime 被严格收窄。
- image upgrade 对应有限、可重复的 capability validation，而非永久兼容承诺。

### Negative / trade-offs

- epoch、lease、logind 与 boot/session identity 增加实现和测试成本。
- 单 Home backend 降低 runtime flexibility，并依赖真实 target evidence。
- Slint、IME、HiDPI、display 与 focus behavior 必须在实际 image 验证。
- 每次 image upgrade 都产生完整 revalidation 成本。

## Acceptance basis and revisit trigger

证据必须覆盖 stale epoch/Agent/UI、logind replacement、reboot、lease expiry、Caddy-call counter；direct XDG launch、singleton、hidden/lazy UI、typed IPC、IME、HiDPI、focus、display loss 和 crash recovery；Home reset/disk-full/ownership/reboot/repeated reset 与 backend performance；以及 package/Slint runtime closure。

出现 simultaneous multi-image/multi-desktop requirement、direct Agent capability 不再可行、新 Home backend 或确认的 overlay requirement 时，以新 ADR 重开。

## Normative sources

- [Architecture](../architecture.md)
- [Contracts](../contracts.md)
- [State and execution](../state-and-execution.md)
- [Security and recovery](../security-recovery.md)
- [Dependency policy](../dependency-policy.md)
- [Repository layout](../repository-layout.md)
- [Supported platform](../supported-platform.md)
