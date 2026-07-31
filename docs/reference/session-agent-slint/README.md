# Phase 6 Session Agent Slint Reference

> 状态：`NON-NORMATIVE SCAFFOLD`  
> 此目录不进入当前 Phase 0 Cargo graph，也不构成 GUI 已实现或目标环境已验证的证据。

生产 contract 已冻结：

- 系统级 XDG Autostart 直接启动同一 resident Agent；
- 只接受 `--autostart`；
- 无 systemd user unit；
- 无 bootstrap/run handoff 或环境 descriptor；
- 初始 hidden；
- 收到 typed Daemon snapshot 后 lazy 创建窗口；
- build-time compiled Slint；
- winit backend + Skia renderer；
- Agent 无 vault、PKI、Server 或 Caddy 所有权；
- GNOME/GDM/Wayland 与 LightDM/目标 X11 desktop 必须实测。

本目录的文件只展示模块形状：

- [`build.rs.example`](build.rs.example)
- [`platform.rs.example`](platform.rs.example)
- [`session_agent.slint`](session_agent.slint)
- [`ui.rs.example`](ui.rs.example)

Phase 6 开始时：

1. 通过正常 dependency/lockfile 流程准入 pinned Slint；
2. 按当时精确 Slint API 校正示例；
3. 把 reviewed pieces 移入 `client/session-agent`；
4. 添加 unit/integration/package tests；
5. 执行 Probe E/F；
6. 不从 docs example 直接宣称 production-ready。

权威文档：

- [ADR-0018](../../adr/0018-xdg-direct-slint-session-agent.md)
- [Dependency policy](../../dependency-policy.md)
- [Probe E](../../probes/e-session-home.md)
