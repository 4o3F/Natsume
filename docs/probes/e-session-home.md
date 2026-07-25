# Probe E — Session Agent、Desktop 与 Home

> Status: `NOT-RUN`  
> Requirements: `REQ-P0-034`、`REQ-P0-038`、`REQ-P0-039`、`REQ-P0-063`、`REQ-P0-064`、`REQ-P0-066`  
> Gates: `G0-010`、`G0-014`

## 目标

在两套目标桌面验证 XDG direct resident Slint Agent、current-session/epoch、lock/unlock、focus/display 故障、Home backend 和 Caddy 解耦。

## 环境矩阵

| ID | DM/DE/协议 | Exact versions | 状态 |
|---|---|---|---|
| E-WAYLAND | GNOME/GDM/Wayland | | NOT-RUN |
| E-X11 | LightDM/selected X11 desktop | | NOT-RUN |

## Agent

每套环境验证：

- `/etc/xdg/autostart/org.natsume.SessionAgent.desktop` 直接启动同一 binary；
- 只接受 `--autostart`；
- resident + hidden；
- typed snapshot 后 lazy create/show；
- hide/show；
- current logind session；
- singleton；
- Chinese/IME；
- HiDPI；
- focus focused/unfocused result；
- display lost；
- Agent crash/lease；
- no systemd user unit；
- no environment descriptor；
- Slint winit + Skia closure；
- no external GUI helper/interpreter。

## Session

- current epoch；
- stale Agent/action 拒绝；
- lock；
- unlock；
- terminate/replacement；
- race/retry；
- audit/Observed。

在 lock/unlock 前后记录：

```text
CADDY_ADMIN_CALL_COUNT
CADDY_CONFIG_HASH
CONFIGURATION_GENERATION
CADDY_STATUS
```

四项必须不变，状态页不得出现 `session_locked`。

## Home

对选定 backend 验证：

- prepare/activate/clean；
- ownership/mode；
- crash；
- reboot；
- disk full；
- partial copy/mount；
- stale epoch；
- unable-to-prove → session blocked；
- no runtime backend fallback。

## 结果

```text
STATUS=NOT-RUN
WAYLAND_RESULT=
X11_RESULT=
HOME_BACKEND=
ARTIFACTS=
LIMITATIONS=
```
