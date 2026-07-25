# Session Agent GUI Startup

> 适用：Agent 未启动、重复、窗口不显示、focus/display 问题  
> 关键不变量：`INV-PRIVILEGE-01`、`INV-SESSION-01`

## 1. 正常模型

- 系统级 `/etc/xdg/autostart/org.natsume.SessionAgent.desktop`；
- 直接启动同一 Agent binary，模式 `--autostart`；
- one resident process；
- 验证当前 logind graphical session；
- owner-only singleton；
- 初始 hidden；
- 收到 Daemon typed snapshot 后 lazy create Slint window；
- 无 systemd user unit、bootstrap/run handoff 或环境 descriptor；
- 无 Server、vault、PKI 或 Caddy ownership。

## 2. 前提检查

1. 确认当前登录会话属于 fixed contest user；
2. 获取 logind session ID、UID、seat、type、boot ID；
3. 检查 XDG Autostart entry package owner/content；
4. 检查不存在 `natsume-session-agent.service` user unit；
5. 检查 binary/package runtime closure；
6. 检查 Daemon local D-Bus；
7. 检查 Agent singleton/lease；
8. 记录 session epoch。

## 3. Agent 未运行

- 检查 desktop environment 是否启用 system XDG Autostart；
- 检查 `.desktop` Exec、TryExec、permissions；
- 检查 display/session environment由桌面直接提供；
- 检查 binary crash/stable ErrorCode；
- 不得手工创建 environment descriptor；
- 不得新增 user unit 作为现场补丁；
- 可在同一图形会话中按 package-defined方式启动 `--autostart` 做诊断，并记录与自动启动差异。

## 4. Agent 重复

- 检查 owner-only singleton；
- 检查是否有手工启动、旧 autostart entry 或 user unit；
- 只保留 package-owned entry；
- 终止陈旧 Agent；
- 确认 current session lease；
- 不得让多个 Agent竞争 UI/D-Bus。

## 5. 窗口不显示

1. 确认 Agent resident；
2. 从 Daemon 读取 typed UI snapshot 和 revision；
3. 检查 action 是否允许显示；
4. 检查 Slint event loop；
5. 检查 display available/lost；
6. 检查 window `show` result；
7. 区分：
   - hidden by design；
   - visible focused；
   - visible unfocused；
   - display unavailable；
   - display lost。
8. Wayland focus denied 记录为 unfocused，不使用强制聚焦 hack。

## 6. IME/HiDPI/渲染

记录：

- desktop/session protocol；
- locale；
- input method；
- scale；
- Slint/backend/renderer；
- 动态库；
- 截图（无 secret）和日志；
- 是否仅一个平台失败。

平台特例只修复 adapter/runtime closure，不修改核心 D-Bus 或 Session 状态机。

## 7. Agent crash/display lost

- lease 到期；
- Daemon 拒绝陈旧 action；
- 根据冻结策略重新启动 Agent或替换受管 session；
- 不得自动 unlock；
- 不得修改 Caddy；
- 记录 crash、epoch、Observed 和 recovery。

## 8. 成功判定

- 当前 graphical session 只有一个 Agent；
- 初始 hidden；
- typed trigger 后窗口按预期出现；
- focus result被正确报告；
- hide/reopen；
- IME/HiDPI；
- stale Agent 被拒绝；
- Caddy call/hash/generation/status不变；
- 无 user unit/descriptor/helper。

## 9. Evidence

```text
DESKTOP_ID=
SESSION_ID=
SESSION_TYPE=
SESSION_EPOCH=
AUTOSTART_HASH=
AGENT_PID=
SINGLETON=
UI_SNAPSHOT_REVISION=
PRESENTATION_RESULT=
SLINT_VERSION=
BACKEND_RENDERER=
CADDY_BEFORE_AFTER=
OWNER=
REVIEWER=
```
