# ADR-0018: XDG direct Slint Session Agent

> Status: `ACCEPTED`  
> Scope: Natsume V2

## Context

Session Agent 必须在真实图形会话中工作，支持 GNOME/GDM/Wayland 与 LightDM/X11，并保持 Daemon/Agent 权限分离。bootstrap/environment handoff 和 systemd user unit 增加 session identity、环境和生命周期耦合。

## Decision

Client package 安装系统级 XDG Autostart entry，直接启动同一个 resident Agent binary。Agent 验证当前 logind session和 singleton，初始 hidden，收到 typed snapshot 后 lazy 创建 build-time Slint UI；使用 winit backend + Skia renderer。无 user unit、bootstrap/run handoff、环境 descriptor、runtime interpreter 或外部 GUI helper。

## Alternatives

- systemd user unit：不作为唯一可靠图形会话启动边界。
- bootstrap → run：环境文件和双进程竞态。
- 直接手拼 winit/renderer/text：维护和 IME/HiDPI 成本高。
- GTK/Qt/Electron/helper：引入额外 runtime 或桌面依赖。

## Consequences

### Positive

- 单进程生命周期；
- UI 与业务 contract 分离；
- package 边界可扫描；
- 双桌面共用核心模型。

### Negative / trade-offs

- Slint runtime closure 必须实测；
- Wayland focus 不能保证，只能报告结果；
- Agent event loop/thread 约束严格。

## Evidence and revisit trigger

Probe E/F 必须验证双桌面、hidden/lazy、IME/HiDPI、focus denied、display lost、no user unit 和 Caddy 不变。

## References

- [supported-platform.md](../supported-platform.md)
- [phase-6-session-home.md](../implementation/phase-6-session-home.md)
