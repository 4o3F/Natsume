# Phase 6 — Session & Home

> 计划：W31–W37  
> 入口：G5 PASS；双桌面与 Home backend 已冻结  
> 退出：G6

## 1. 目标

交付受管图形会话、resident Slint Session Agent 和可恢复 Home transaction，同时证明所有 Session 操作与 Caddy 数据面解耦。

## 2. 工作包

### P6.1 Package and launch

- `/etc/xdg/autostart/org.natsume.SessionAgent.desktop`；
- direct `--autostart`；
- one resident process；
- initial hidden；
- no user unit；
- no bootstrap/run handoff；
- no environment descriptor；
- package/runtime closure scan。

### P6.2 Platform adapter

- current process → logind session；
- graphical eligibility；
- contest UID；
- boot/session ID；
- owner-only singleton；
- display available/lost；
- desktop capability；
- no display-manager private API。

### P6.3 Slint UI

- build-time compile；
- winit backend；
- Skia renderer；
- event-loop thread；
- typed snapshot → view model；
- lazy window；
- hide/show；
- Chinese/IME；
- HiDPI；
- focus result；
- accessibility；
- no external helper/interpreter。

### P6.4 Local API and lease

- Daemon typed snapshot/actions；
- UID/PID/session/epoch validation；
- Agent heartbeat/lease；
- stale Agent rejection；
- crash/display lost recovery；
- no secret/PKI/Caddy data。

### P6.5 Session state

- session epoch；
- start/active/lock/unlock/terminate；
- current logind session validation；
- race/retry；
- managed session replacement；
- audit/Observed；
- lock/unlock no Caddy call。

### P6.6 Home

- fixed contest user；
- versioned template；
- selected OverlayFS or staged-copy backend；
- home epoch/journal；
- prepare/activate/clean；
- ownership/mode；
- crash/reboot；
- disk-full/corruption；
- no silent backend fallback；
- cannot prove safe → no session.

### P6.7 Dual desktop evidence

- GNOME/GDM/Wayland；
- LightDM + selected X11 desktop；
- XDG direct launch；
- hidden/lazy；
- focus denied；
- lock/unlock；
- display lost/Agent crash；
- Home；
- Caddy call count/hash/generation unchanged。

## 3. 交付物

- production Session Agent；
- local D-Bus；
- platform adapters；
- Session state machine；
- Home backend；
- package files；
- desktop probe；
- session/home runbooks；
- G6 decision。

## 4. Definition of Done

- both desktop matrices pass；
- no systemd user unit/descriptor/helper；
- no early visible window；
- Agent only current session；
- stale epoch rejected；
- crash/lease recovery；
- lock/unlock Caddy call count zero；
- Caddy hash/generation/status unchanged；
- Home crash/reboot/disk-full recovery；
- unable-to-prove Home blocks session；
- GUI runtime closure approved；
- G6 decision signed。
