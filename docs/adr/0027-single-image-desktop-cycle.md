# ADR-0027: Single-image desktop cycle

> Status: `ACCEPTED`  
> Scope: Natsume V2 桌面支持策略与 Session Agent 呈现范围  
> Supersedes: —（替代双桌面持续支持矩阵；[ADR-0018](0018-xdg-direct-slint-session-agent.md) 的启动模型与 GUI 栈维持不变）  
> Superseded by: —

## Context

原平台要求持续支持双桌面矩阵（GNOME/GDM/Wayland 与 LightDM/X11 各自全项验证）。按 [ADR-0022](0022-deployment-facts-and-trust-assumptions.md) F4：基础镜像派生自 ICPC 官方镜像、最终镜像由本项目构建——任一赛事周期内**只存在一个镜像、一个桌面环境**；但 ICPC 镜像大版本更新可能改变桌面栈，"永久锁定单一桌面"同样不成立。正确形态是按周期冻结加升级重验，而不是持续双环境支持。

另有一项已知的未来需求：全屏遮罩层展示校徽/队伍名。技术事实：GNOME/Mutter Wayland 下普通客户端无法可靠实现全屏置顶 + 输入独占（compositor 特权，Mutter 不支持第三方 session-lock 协议）；X11 下 XGrab + override-redirect 是成熟做法。且任何与会话同 UID 的遮罩进程都可被预埋脚本 kill——遮罩层在任何桌面上都不构成完整性边界。

## Decision

1. **每赛事周期冻结恰好一个镜像版本**，因此恰好一个桌面环境；当前周期为 **X11**（F4）。
2. **双桌面持续支持矩阵取消**，替换为**镜像升级重验清单**：每次镜像 bump 重跑一遍桌面 capability 清单（XDG Autostart 直接启动、resident + hidden、logind session 识别、owner-only singleton、中文/IME、HiDPI、focus 结果可观察、lock/unlock、terminate/replacement、display lost 与 crash recovery、无 systemd user unit、lock/unlock 的 Caddy 调用数为 0）。清单本体在 [supported-platform.md](../supported-platform.md) 维护。
3. **核心依赖 capability、不依赖桌面名称**的原则维持（[ADR-0018](0018-xdg-direct-slint-session-agent.md)）；桌面差异仍封装在 adapter。
4. **本周期不实现遮罩层**；锁定语义走桌面原生 session lock。扩展空间的具体形态：`local-control-api` 的 UI view kind 与 action 保持封闭枚举 + 版本升级路径（新增枚举值即可），不预建遮罩实现。
5. 未来引入遮罩层的前置条件（届时新 ADR）：当期镜像桌面上的输入独占可行性证据（X11 grab 或等价机制），并明确立场——**遮罩是 UX 呈现，不是完整性边界；完整性依靠 `SESSION_TERMINATE` 与数据面 BLOCKED**。

## Alternatives

- **双桌面持续矩阵**：为不存在的并行环境付出最难约的验证资源（G0 最大阻塞项之一）；单镜像事实下无对应场景。
- **永久锁定单一桌面**：ICPC 镜像大版本可能改变桌面栈，锁定会在 bump 时被迫违约。
- **现在实现遮罩层**：需求未定型（仅"后续可能"），且 Wayland 可行性依赖桌面选型——按 YAGNI 延后，仅留枚举扩展位。

## Consequences

### Positive

- Phase 6 与平台输入门禁的验证面减半；
- 镜像升级有明确、可重复的重验程序，而不是开放式兼容承诺；
- 遮罩需求到来时有干净的扩展点与清晰的安全定位。

### Negative / trade-offs

- ICPC 镜像 bump 可能强制重验甚至适配（如未来切换 Wayland，遮罩前置条件需重评）；
- 不同赛事若使用不同镜像版本，需各自跑一遍重验清单。

## Evidence and revisit trigger

- 接受前需要：当前周期镜像上 capability 清单全项通过的可复现记录。
- 重开条件：需要同时支持多镜像/多桌面的部署出现；或遮罩层从"可能"变为确认需求（触发第 5 条的新 ADR）。

## References

- [ADR-0018](0018-xdg-direct-slint-session-agent.md)
- [ADR-0022](0022-deployment-facts-and-trust-assumptions.md)
- [supported-platform.md](../supported-platform.md)
- [state-and-execution.md](../state-and-execution.md)
