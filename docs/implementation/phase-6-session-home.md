# Natsume V2 Phase 6 详细实施计划：XDG Autostart Slint Session Agent、D-Bus、Session 与 Home

> 架构基线：`Natsume_V2_Design_v2.7.md`  
> Roadmap：`Natsume_V2_Implementation_Roadmap_v1.4.md`  
> 详细计划版本：v1.2  
> 日期：2026-07-23  
> 阶段窗口：W25–W34  
> Gate：G5

---

## 1. 阶段使命与边界

交付由认证后的desktop session通过system-wide XDG Autostart直接启动的常驻Session Agent。Agent启动时无窗口，只有收到Daemon的typed UI snapshot才通过Slint懒显示Binding、Recovery或Lock presentation。支持矩阵至少覆盖GNOME/GDM/Wayland与LightDM启动的目标X11 desktop；Display Manager只负责认证和启动desktop，Agent不调用LightDM/GDM，也不由systemd user unit监督。

Session lock/unlock只控制desktop，与Caddy完全解耦。Agent只消费typed、无秘密的local UI snapshot，不读取Client vault、password、Device/Gateway key或Caddy runtime config。

本阶段不改变Enrollment、QUIC、Gateway certificate或secret协议；它消费Phase 4/5已经冻结的Daemon和local-control contract。

## 2. 前置依赖

- G0已证明XDG Autostart direct launch、Slint build/runtime与目标桌面矩阵可行；
- G3提供可靠Command、Observed与connection/session状态；
- G4提供Gateway readiness、Browser gate、secret/LKG和Caddy恢复；
- contest UID/seat、目标Display Manager/desktop/Browser与Home backend已冻结；
- D-Bus interface、logind权限和受管Home shadow guard设计已review。

## 3. 详细工作包

### P6.1 Local-control D-Bus契约

- 冻结`org.natsume.Privileged1`与`org.natsume.Device1`；
- `SessionAgentRegistration`删除supervisor字段；
- registration/lease/snapshot/action/presentation/binding/unregister methods；
- D-Bus peer UID、session target、nonce、revision与payload上限；
- zbus introspection/golden/compatibility tests。

### P6.2 XDG Autostart直接启动

- package-owned `/etc/xdg/autostart/org.natsume.SessionAgent.desktop`；
- 唯一命令`/usr/bin/natsume-session-agent --autostart`；
- 无OnlyShowIn/NotShowIn，无desktop-specific副本；
- 无systemd user unit、无bootstrap/run双模式、无environment descriptor；
- Agent通过自身PID调用logind并验证UID/Class/Type/Active/Remote/Seat；
- 多eligible session返回`SESSION_AMBIGUOUS`；
- greeter/TTY/SSH/remote/inactive/错误UID或seat拒绝；
- `$XDG_RUNTIME_DIR/natsume/session-agent.lock` owner-only singleton；
- user-level同名Autostart shadow fixed-path guard；
- 不执行shell、systemctl/systemd-run --user、dbus environment import。

### P6.3 Agent生命周期与lease

- Agent注册后为`ready + hidden`，没有window/tray/splash；
- background lease renewal与session/display liveness probe；
- logout/display loss/session replacement主动停止Browser、unregister并退出；
- duplicate Agent返回`SESSION_AGENT_DUPLICATE`；
- crash/kill导致Daemon lease timeout、`SESSION_AGENT_MISSING`和Browser gated；
- 恢复只通过受管logout/terminate/session replacement重新触发Autostart；
- Daemon禁止猜测DISPLAY/WAYLAND_DISPLAY或直接spawn Agent；
- Daemon restart后的Agent re-register和snapshot恢复。

### P6.4 Slint构建与feature边界

- `client/session-agent/build.rs`使用`slint-build`编译`ui/session_agent.slint`；
- `slint::include_modules!()`只加载build output；
- disable-default-features；显式`std`、`compat-1-2`、`backend-winit`、`renderer-skia`、accessibility；
- BackendSelector固定`winit` + `skia`；
- 禁止Qt backend、system-tray、interpreter、live-preview、MCP、system-testing；
- 不直接依赖winit、softbuffer、tiny-skia、cosmic-text；
- UI source/message/icon为package-owned build inputs，不在运行时下载/解释；
- 构建体积、链接闭包和Skia toolchain可重复性报告。

### P6.5 无窗口常驻与UI线程模型

- main thread调用`run_event_loop_until_quit()`；
- 初始不实例化/显示top-level component；
- Tokio/zbus worker运行Daemon IPC与lease；
- worker通过`invoke_from_event_loop`提交UI更新；
- 第一次非hidden snapshot懒创建`SessionWindow`；
- hidden snapshot调用hide并保留event loop/lease；
- callback只形成typed action，不阻塞UI thread；
- close request映射为cancel/ack/hide，不能绕过active policy；
- display/backend fatal error撤销registration并fail closed。

### P6.6 Typed UI model与Binding Prompt

- screen闭集：hidden/idle/binding_prompt/binding_pending/binding_result/recovery/lock/fatal；
- message ID + bounded typed parameters，不接受HTML/Markdown/format string/remote asset；
- Machine ID短码、Seat输入、deadline、pending/result；
- `prompt_command_id + prompt_nonce + exact SessionTarget`提交；
- duplicate/out-of-order ui_revision拒绝；
- presentation ack区分focused/unfocused/unsupported/failed；
- Agent crash/relogin后从Daemon snapshot重建当前状态，不在本地保存业务truth。

### P6.7 Wayland/X11、文本与可访问性

- Slint winit backend自动选择当前Wayland/X11环境；
- GNOME Wayland、GNOME X11（可用时）、LightDM + Xfce/MATE X11实测；
- Chinese/ASCII、system font fallback、IME composition、paste、keyboard navigation；
- HiDPI、fractional scale、multi-monitor、window close与focus denied；
- `presented_unfocused` + optional standard notification；
- no focus stealing loop、no compositor lock-screen override；
- Slint built-in software renderer western-script限制的negative decision test；正式renderer为Skia；
- 无可用GPU加速的目标机上启动/输入/绘制验收。

### P6.8 Managed Browser

- package-owned absolute executable与fixed argv；
- no PATH discovery、xdg-open或shell；
- exact Agent lease/session target/Home/Gateway/Browser gate；
- Agent exit/display loss/lease revoke停止Browser；
- Browser restart/backoff有界；
- argv/env/log无credential；
- desktop lock期间Caddy/Gateway保持不变。

### P6.9 Desktop lock/unlock/terminate

- exact session instance/epoch/lock epoch/originating lock command；
- Helper经logind/desktop capability请求；
- Agent presentation不充当安全锁；
- verified lock state + Agent ack才terminal success；
- stale/late unlock拒绝；
- unsupported desktop返回稳定错误；
- lock/unlock zero Caddy Admin/config/secret mutations；
- terminate使旧lease/UI/unlock全部失效。

### P6.10 Privileged Helper hardening

- root、PrivateNetwork、固定method与本地path/UID/seat；
- 无DISPLAY/WAYLAND_DISPLAY、Agent spawn、任意unit或任意argv接口；
- logind/session/Desktop Manager控制只通过typed/fixed action；
- D-Bus unauthorized UID tests；
- no external network和capability最小化。

### P6.11 Home Template与Reset

- immutable versioned template；
- OverlayFS instance/staging/active record；
- staged-copy deployment-time fallback；
- session quiesce、new instance、mount/copy、verify、activate、commit、cleanup；
- display-manager前boot recovery与Autostart shadow guard；
- 每个durable step kill/reboot fault injection；
- uncertain Home不允许desktop session/Agent启动。

### P6.12 Packaging、Panel、Runbook

- system-wide XDG desktop entry与fixed shadow guard；
- 明确断言不存在Session Agent user unit；
- Slint build inputs/features与ELF/package dependency report；
- Panel显示Agent state、hidden/presentation/backend/lease/error；不显示supervisor；
- runbooks：direct Autostart、greeter rejection、LightDM desktop、focus denied、Agent crash/relogin、Home recovery；
- forbidden feature/executable/runtime scans。

## 4. 建议执行顺序

### W25–W26

- D-Bus contract与peer authorization；
- XDG desktop entry、logind validation、singleton；
- direct Agent registration/lease/hidden state；
- remove user-unit/bootstrap/descriptor assumptions。

### W27–W28

- Slint build.rs、feature boundary、UI component；
- no-window event loop + zbus worker bridge；
- GNOME Wayland与LightDM/X11 first smoke；
- Chinese/IME/focus/notification/HiDPI。

### W29–W30

- Binding Prompt/Result；
- managed Browser；
- Agent crash/relogin/snapshot recovery；
- desktop lock/unlock/terminate。

### W31–W32

- Home Template/OverlayFS/staged-copy；
- transaction/reboot recovery；
- Browser/Gateway/Home/session integration。

### W33–W34

- full target matrix；
- packaging/dependency/feature scan；
- G5 regression、fault injection、runbook rehearsal。

## 5. 交付物

- frozen local-control UI/session contract，无SessionSupervisor；
- XDG Autostart direct-launch entry；
- direct resident Agent with singleton/lease；
- build-time Slint UI与typed screen model；
- Agent no-window/lazy-window state machine；
- Binding Prompt与managed Browser；
- exact desktop lock/unlock/terminate；
- Home template/backends/reset/recovery；
- GNOME/LightDM目标矩阵与Slint feature/dependency report；
- runbooks和G5 evidence bundle。

## 6. 测试与验证矩阵

| 场景 | 预期 |
|---|---|
| GNOME/GDM/Wayland login | XDG direct launch，Agent ready+hidden，Slint Wayland window only on trigger |
| GNOME/GDM/X11 | X11 backend，行为等价 |
| LightDM + target X11 desktop | Desktop Autostart直接启动同一Agent，无LightDM私有耦合 |
| LightDM/GDM greeter | logind class/UID/session校验拒绝 |
| SSH/TTY/remote/inactive | 拒绝，不注册Agent |
| two eligible sessions | `SESSION_AMBIGUOUS`，不猜测 |
| duplicate Autostart | singleton拒绝第二实例 |
| initial steady state | no visible window/tray/splash；lease持续 |
| binding snapshot | lazy component creation、show、typed ack |
| hidden snapshot | hide window，Agent/event loop/lease继续 |
| Agent kill | lease timeout、Browser gated、no daemon spawn |
| managed relogin | new process/session/lease from XDG Autostart |
| Daemon restart | Agent re-register并恢复latest snapshot |
| Wayland focus denied | presented_unfocused，不循环抢focus |
| Chinese/IME/paste | Skia text/input behavior正确 |
| no usable GPU acceleration | UI仍可启动、输入与呈现，或在Gate前明确冻结受支持fallback |
| forbidden Slint feature | CI拒绝Qt/tray/interpreter/live-preview/MCP/testing |
| user autostart shadow | fixed-path clean/block，Browser gated |
| stale unlock | rejected |
| lock/unlock | zero Caddy mutation |
| each Home reset crash point | resume或fail closed |

## 7. 风险与缓解

| 风险 | 缓解 |
|---|---|
| XDG Autostart不监督崩溃进程 | Agent无秘密；lease fail closed；受管session replacement/relogin恢复；不引入第二启动平面 |
| LightDM被误当Agent API | 架构/测试只验证其启动的desktop；runtime session action归Daemon/Helper/logind |
| Wayland不允许抢焦点 | presentation state + notification；不循环抢焦点 |
| Slint/Skia构建体积和C++ toolchain | 版本/源码/digest锁定、CI cache、SBOM、reproducible build和target package test |
| Slint built-in software renderer中文受限 | 正式选择Skia；中文/IME fixture；无GPU加速机器实测 |
| Skia隐式动态依赖 | ELF/DT_NEEDED、dpkg dependency closure、clean VM install gate |
| user-level Autostart覆盖 | managed Home fixed-path guard + missing lease Browser gate |
| Agent crash现场恢复成本 | Panel stable error、operator one-click terminate/recreate session runbook |
| desktop lock signal不等于完成 | 验证真实session lock state；unsupported/timeout不冒充成功 |
| Home/Display Manager竞态 | home-prepare before display-manager，transaction journal，login gate |

## 8. G5 Gate 清单

- [ ] GNOME/GDM/Wayland与LightDM/目标X11 desktop真实login通过；
- [ ] GNOME X11在目标发行版提供时通过；
- [ ] greeter/TTY/SSH/remote/inactive/错误UID/seat/ambiguous session全部拒绝；
- [ ] package无Session Agent user unit、bootstrap/run模式或environment descriptor；
- [ ] XDG direct launch、singleton、lease、logout/relogin/display-loss语义通过；
- [ ] Agent启动为ready+hidden，无window/tray/splash；typed trigger lazy show/hide通过；
- [ ] Slint build-time UI、backend-winit/renderer-skia、thread bridge通过；
- [ ] no Qt/tray/interpreter/live-preview/MCP/system-testing features；
- [ ] Chinese/IME/HiDPI/multi-monitor/focus denied与无GPU加速目标机通过；
- [ ] no systemctl/systemd-run --user、dbus-update、zenity/kdialog/yad/xdg-open shellout；
- [ ] Agent kill导致lease fail closed且managed relogin恢复；Daemon不spawn GUI；
- [ ] BindingRequest/BindingResult端到端且不auto secret；
- [ ] exact epoch-bound lock/unlock/terminate；
- [ ] lock/unlock zero Caddy mutations；
- [ ] Home reset每个step fault injection通过；
- [ ] Helper D-Bus authorization/no-network/hardening通过；
- [ ] G5 evidence由Server、Client、OS/QA、安全和运维owner签收。
