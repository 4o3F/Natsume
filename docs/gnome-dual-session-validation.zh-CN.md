# GNOME 双会话：架构定型与 VM 验证记录

日期：2026-09-07。以下保留 PRD v1.1 的历史实测。本轮目的是用现有虚拟机关闭机制选型和恢复顺序的疑问，不是交付正式 Client 实现。

后续目标已在[主架构](architecture.md)和 [PRD v1.3](prd-gnome-dual-session.zh-CN.md)修订：业务操作改名为“显示等待界面 / 显示比赛桌面”，以 `foreground_target=waiting|contest` 选择前台，取消正常流程中的桌面 Lock/Unlock；waiting 本期使用纯黑或单张 ICPC logo 静态占位。下文“contest 已锁定”“unlock + activate”和带文案的 Skia 截图仍按原测试事实保留，不是新语义的完成条件。前台切换、GDM 登录、PAM 互锁和全屏能力证据可复用；正式实现需另验收仅激活、不锁屏的调用路径、统一命名及最终静态占位。

## 1. 结论与边界

当前镜像的官方 GDM 46 / GNOME 46 / X11 能支持两个独立会话常驻、正常切换，以及仅重建 contest 的 Home reset。接受重建期间切屏后，不需要 Wayland、嵌套桌面或 GDM/GNOME patch。

本轮进一步确定了以下方案：

| 原待定项 | 确定的方案 | VM 证据 |
| --- | --- | --- |
| 正式 PAM 互锁方式 | 发行版 pam_exec + 固定本地门禁入口 + 短锁内原子登记精确 GDM worker；Helper 排空后修改 Home | 三个 PAM 阶段暂停，logind 尚无 contest 时仍阻止维护；取消后可恢复登录 |
| Skia 全屏 | Slint/Skia 1.15.1 原生 X11，fullscreen API、跟随窗口的根布局、关闭回调和首帧确认 | 1280×800、1920×1080；EWMH、实际几何与截图一致 |
| waiting 程序恢复 | GNOME RegisterClient + X-GNOME-AutoRestart；触发 GNOME 限流后有界地仅重建 waiting | 首次崩溃约 0.54 秒恢复，连续崩溃触发限流；waiting 单独重新登录不影响 contest/GDM |
| waiting 配置隔离 | 独立 dconf profile，经 waiting 的 environment.d 传给用户运行环境 | 避开全局 ArcMenu 锁定；四种快捷键测试后的截图与基线完全相同 |
| Agent 会话身份 | 总线 PID/UID + logind/systemd 用户运行环境 + 唯一精确 waiting 图形会话 | 证实旧 GetSessionByPID 路径失败；新的严格映射在前后台均成立 |
| 开机与恢复 | GDM/waiting 不依赖 Home 成功；独立恢复 Home，再经固定 API 预备并锁定 contest | 正常重启、四阶段硬复位恢复、Home 恢复失败时 waiting 单独可用 |
| 旧窗口升级 | 选定拒绝带进行中旧窗口升级，由旧 Helper 恢复完再升级；新格式显式版本化 | 这是迁移策略决定；新窗口阶段恢复有 VM 证据，正式旧格式安装检查尚未实现 |

上述“精确映射”限定固定 waiting 用户和唯一图形会话，不把同一 UID 下的任意多会话归属问题宣称为已解决。正式 Binding 的 caller/lease、Server epoch、断网 fencing、模板内容及发行压力验收仍需在产品集成后测试。

## 2. 被测环境

| 项目 | 实际值 |
| --- | --- |
| VM | 已有 `natsume-gdm-test`；QEMU 8.2.2、KVM、4 vCPU、6 GiB、OVMF、virtio-vga |
| 系统 | 用户提供 ICPC 镜像安装后的 Ubuntu 24.04.4 测试系统 |
| gdm3 | `46.2-1ubuntu1~24.04.9` |
| gnome-shell | `46.0-0ubuntu6~24.04.14` |
| gnome-session-bin | `46.0-1ubuntu4` |
| libmutter-14-0 | `46.2-1ubuntu0.24.04.16` |
| systemd | `255.4-1ubuntu8.16` |
| libpam-modules | `1.5.3-5ubuntu5.6` |
| xserver-xorg-core | `2:21.1.12-1ubuntu1.6` |
| 图形用户 | waiting UID 1002；contest UID 1001；seat0；各自原生 X11 |
| 验证 UI | 独立 Rust 程序，Slint 1.15.1，backend-winit-x11 / renderer-skia；不是已接入 Device1 的正式 Agent |
| Home | 实际 OverlayFS，固定只读 lower 与按 generation 分离的 upper/work |

`dpkg --verify` 对上述图形/PAM 程序包只报告 `/etc/gdm3/custom.conf`、`/etc/pam.d/gdm-autologin`、`/etc/pam.d/gdm-password` 的配置变化；未报告所查程序或 GNOME 资源的变更。实验没有修改 GDM、GNOME Shell、Mutter 的程序或资源。新增 API 调用程序、门禁和 systemd/dconf 配置位于测试目录或配置目录。

这是一台经过配置的现有安装 VM，不能将结果表述为“最终 ISO 已重新构建并完成从零安装验收”。

## 3. PAM 互锁：为什么采用 worker 登记

仅在 PAM 检查一次 ready 文件有竞态：PAM 返回成功后，登录还可能没有出现在 logind；此时重置不能把“没有 session”解释成“没有登录事务”。仅让 pam_exec 子进程持有短时 flock 也有同一问题。[Linux-PAM 1.5.3 的官方实现](https://raw.githubusercontent.com/linux-pam/linux-pam/v1.5.3/modules/pam_exec/pam_exec.c)会等待执行的子进程结束，子进程退出后不能替整个登录事务继续持锁。

选定两部分共同构成互锁：

1. PAM 门禁在共享锁内确认本次启动的 Home 许可，原子登记实际父 GDM worker 的 boot ID、PID、进程启动时间，再返回。登记保持到精确 worker 退出；auth/account/open_session 都执行，close_session 不提前清除记录。
2. Helper 关闭许可，取得同一锁的排他权，检查登记中没有仍存活的精确 worker，并确认 contest 会话、UID 进程、用户 manager 和 Home 占用已排空，然后才卸载或替换 Home。记录损坏、查询失败、取消超时均不能解释为已排空。

最终原型不包含自定义 PAM 模块，也没有持锁 keeper 常驻进程。生产实现由固定 Helper 子命令提供同样契约；Python 是本次验证载体。

VM 的 `gdm-autologin`、`gdm-password` 在保留官方栈的基础上添加三类 requisite pam_exec 检查；`gdm-contest` 限制 contest 并包含该自动登录栈。waiting/greeter 不受 contest 门禁阻塞。其他未授权 SSH/TTY/指纹等入口应关闭，不能因测试了两个入口就假定所有可能路径都已封闭。

| 注入点 | logind contest | 登记的 worker | 维护排他判定 | 取消后 |
| --- | --- | --- | --- | --- |
| auth 检查后暂停 | 无 | 94517 | 拒绝 | worker 排空，可继续 |
| account 检查后暂停 | 无 | 94555 | 拒绝 | worker 排空，可继续 |
| open_session 检查后暂停 | 无 | 94586 | 拒绝 | worker 排空，可继续 |

三次均通过杀死单次登录 API 客户端来模拟调用者崩溃；GDM 排空相应 worker，waiting 与 GDM 保持实例，随后重新允许登录成功。另行将 Home 保持不就绪，实测 `gdm-contest` 与 `gdm-password` 均在认证阶段拒绝，没有创建 contest。

证据：`pam-registry-races.log`、`closed-login.log`；实现载体：`scripts/pam-home-gate.py`、`scripts/admission.py`、`scripts/pam-races.py`。较早的 `pam-races.log` 和 `stock-pam-cycle.log` 属于中间方案，不作为最终互锁的主要证据。

## 4. Skia、GNOME 恢复与显示配置

### 4.1 全屏及首帧

验证程序调用 `Window.set_fullscreen(true)`，使用 Slint render notifier 记录首帧，在 UI 事件循环中报告存活和窗口尺寸，并通过 close callback 拒绝普通关闭。X11 实际窗口具有 `_NET_WM_STATE_FULLSCREEN`，位置为 0,0，边框为 0，尺寸随 1280×800、1920×1080 切换。

发现并修正了两种误判：固定根内容尺寸会造成窗口虽全屏、文字布局却仍按初始尺寸显示；应用刚启动时也可能先报告初始尺寸。因此选定 preferred size + 随窗口变化的根布局，并在首帧与实际尺寸确认后才允许报告 UI 就绪。渲染成功不单独证明它在前台，前台与遮挡仍需独立验证。

证据：`skia-final-1080p.log`、`skia-final-1080p.png`、`skia-final-1280.log`、`keys-baseline.png`。未测试正式 Binding 输入框、IME 或分数缩放。

### 4.2 崩溃恢复

XDG entry 的 `X-GNOME-AutoRestart=true` 与 Agent 自己的 `RegisterClient` 连接配合使用。第一次 SIGKILL 后 UI 从 PID 91805 恢复为 92429，耗时约 0.54 秒；两边 GNOME/Shell 和 GDM 没有重建。随后再次 SIGKILL，GNOME 触发 60 秒窗口内的重启限流，UI 保持缺失，两个桌面和 GDM 仍存活。

GNOME 46 的[应用限流实现](https://raw.githubusercontent.com/GNOME/gnome-session/46.0/gnome-session/gsm-app.c)与[会话管理器的重启处理](https://raw.githubusercontent.com/GNOME/gnome-session/46.0/gnome-session/gsm-manager.c)支持上述观测。Agent 不标为 RequiredComponent，避免把 UI 连续故障提升为必需桌面组件故障。

另用固定 `gdm-waiting` API 入口重建 waiting，验证 contest 的 session/Shell/Xorg 与 GDM InvocationID 保持不变。产品选定“先由 GNOME 恢复；持续故障冷却至少 60 秒后，仅自动重建 waiting 一次；再次失败等待维护”的有界策略。冷却计数器与正式故障 UI 尚未实现，不能把原型的手动故障注入当作该产品状态机已验收。

证据：`skia-crash.log`、`gnome-restart-journal.txt`、`waiting-profile-final-relogin.log`。

### 4.3 当前镜像的 dconf 冲突

只清空 GNOME overlay-key 不够：镜像 `/etc/dconf/db/local.d/locks/locks` 锁定了扩展策略，waiting 会继承 ArcMenu；Super 能在全屏窗口上打开菜单，且普通 gsettings 设置返回不可写。

最终选择 waiting 专用配置，不修改比赛桌面的全局策略：

```ini
# waiting 的 ~/.config/environment.d/90-natsume-waiting.conf
DCONF_PROFILE=natsume_waiting
```

```text
# /etc/dconf/profile/natsume_waiting
user-db:user
system-db:natsume_waiting
```

单独的 system 数据库锁定禁用用户扩展、清空启用扩展列表、关闭热角和被测菜单/切换快捷键；dconf 空字符串数组写成 `@as []`。数据库使用下划线名称，以避免写入通知的 D-Bus 路径兼容问题。waiting 用户运行环境重新建立后，该 profile 对 GNOME 生效；contest 继续使用原 profile。

Xorg 使用官方 `DontVTSwitch=true`、`DontZap=true` ServerFlags；这是系统 Xorg 配置，影响双方键盘 VT/Zap 行为。受控 logind 激活仍然有效。

最终 Alt+F4、Super、Alt+Tab、Ctrl+Alt+F2 后 waiting 均在前台，四张截图与基线像素完全一致。它们是四个具体用例，不是所有快捷键、扩展和恶意输入均不可逃逸的证明。

证据：`dconf-locks.log`、`scripts/setup-waiting-profile.py`、`keyboard-checks.json`、`keys-*.png`。

## 5. Agent 身份：不能原样沿用 GetSessionByPID

实际 Agent 位于 `user@1002.service/app.slice/app-gnome-….scope`。`GetSessionByPID(977)` 返回“PID does not belong to any known session”；其 GNOME SessionManager 也运行在 systemd 用户服务中，向上找父进程不能解决问题。GNOME 的[官方应用启动实现](https://raw.githubusercontent.com/GNOME/gnome-session/46.0/gnome-session/gsm-autostart-app.c)会为应用建立 systemd scope，与 VM 观测一致。

实测可用的映射为：

| 真实查询 | 结果 |
| --- | --- |
| logind GetUserByPID(AgentPID) | waiting 的 user 对象 |
| 系统 systemd GetUnitByPID(AgentPID) | waiting 的 `user@1002.service` |
| logind User.Display | 精确 waiting session 1 |
| 该用户的会话集合 | 唯一 waiting/seat0/X11 图形会话 |
| 切到 contest 后再次查询 | waiting 身份及 Display 不变 |
| 后台 UI 存活 | 同一 PID 的事件循环心跳继续推进 |

该映射使用[已安装版本的官方 logind API](https://raw.githubusercontent.com/systemd/systemd/v255/man/org.freedesktop.login1.xml)。正式鉴权必须从系统总线获取调用者 PID/UID，严格限制唯一 waiting 用户运行环境，再绑定 boot/session/lease；不能只信 `XDG_SESSION_ID`，也不能对任意用户用 User.Display 做宽松回退。直接 PID 查询若返回别的会话，应拒绝而非回退。

waiting 禁用 linger 和其他登录入口；replacement 前必须确认旧 waiting 用户 manager 和 UID 进程全部退出。这样旧 Agent 无法依靠残留进程身份重新归属新 session。该前提属于部署和 Helper 恢复契约，不能省略。

证据：`identity-failure-detail.log`、`identity-parent.log`、`identity-user-display.log`、`agent-identity-fixed.log`、`agent-identity-final.log`。测试验证真实系统映射和后台事件循环，没有启动正式 Device1 来验证完整 Binding 提交或伪造请求拒绝。

## 6. Home 恢复与启动顺序

原型采用独立的宿主 Home 恢复 unit 和固定双会话预备 unit；GDM 仍自动登录 waiting。预备流程等实际 Home 许可和 waiting UI 就绪后调用 GDM 登录 contest，确认 GNOME 运行，再锁定 contest 并返回 waiting。Home 任务失败不会停止 GDM 或 waiting。

持久测试窗口记录 generation、阶段、旧 contest 的 boot/session。分别在写入 closed、prepared、applied、verified 后令测试 Helper 退出 99，确认 ready 关闭，再用 QMP `system_reset` 硬复位：

| 中断点 | 恢复 generation | 到双会话等待态的观测耗时 | 结果 |
| --- | --- | --- | --- |
| closed | 28 | 22.92 秒 | 成功 |
| prepared | 29 | 51.23 秒 | 成功 |
| applied | 30 | 23.02 秒 | 成功 |
| verified | 31 | 23.12 秒 | 成功 |

每次都观察到新 boot、waiting 前台、contest 已锁定、正确 OverlayFS、脏标记消失及持久窗口结束。prepared 那次在 UEFI 停留较久后自行启动，没有人工修复。此处测试的是虚拟硬复位，不能替代真实断电时磁盘缓存、宿主存储及实体硬件持久化验证。

另注入模板不可用错误，使 Home 恢复入口失败：重启后 waiting 全屏可用，contest 不存在，Home 许可关闭；专用及密码登录均拒绝。撤销故障后，恢复原 generation 31 并由 GDM 登录 contest，waiting 与 GDM 不需要重建。这里通过显式错误注入验证失败传播，不宣称覆盖真实磁盘满、模板包缺文件等全部 I/O 故障。

同一 boot 内还执行了正常 reset 和 applied 后 Helper 崩溃恢复：generation `31 → 32 → 33`，contest `10 → 15 → 17`；waiting session 1、Shell 793、Xorg 637、Skia 977 以及 GDM PID 584/InvocationID 都保持不变。每次恢复后延迟执行 unlock + activate，都进入已有新 contest，没有再次登录或重置。

证据：`boot-crashes.log`、`crash-*.log`、`boot-closed.json`、`boot-prepared.json`、`boot-applied.json`、`boot-verified.json`、`missing-template-boot-fixed.log`、`template-repaired.log`、`final-cycles.log`。正常启动还包括 `normal-boot.log` 及最终 profile 配置的 `final-profile-boot.log`。

原型用 generation 标识测试事务，不含真实 Server reset epoch；正式 Helper 的窗口编码、完整模板验证、旧 generation 回收与 durable completion 仍须在产品中实现。旧窗口升级选择“先由旧版恢复完成，再升级”，不进行含 restart-display 语义的在线格式转换。

## 7. 已关闭的疑问与发行前剩余项

| 分类 | 状态 |
| --- | --- |
| GDM 受控重新登录、两原生会话、普通切换、正常 reset 保留 waiting | 已通过真实 API 和 VM 验证 |
| PAM 前检查竞态及应用 scope 身份兼容 | 已发现问题、确定具体处理方式并用真实系统接口验证 |
| Skia 全屏、GNOME 重启限流、waiting 独立配置 | 已实测并写入 PRD |
| 新窗口四阶段恢复、Home 失败不阻塞 waiting | 已通过原型恢复和硬复位测试 |
| 正式 Daemon/Helper/Agent/Panel、caller/lease/epoch fencing | TODO：实现与端到端集成回归 |
| 正式模板和镜像/包安装、升级拒绝旧窗口 | TODO：交付并验证，不能用测试目录代替 |
| 100 轮切换、20 个正式 reset epoch、10 次发行启动 | TODO：按 PRD 的正式产品矩阵执行；本轮不以原型次数冲抵 |
| 实体 GPU、驱动、IME、缩放、休眠、真实断电 | TODO：最终环境验收，当前 QEMU 不能全部回答 |

这些剩余项不再是是否需要 patch、嵌套桌面或 Wayland 的选型问题。

## 8. 复核材料

主证据目录：`/tmp/natsume-gdm-qemu/resolution/`。脚本目录：其中的 `scripts/`；Skia 验证源码：`skia/`。正式仓库仅增加/更新设计文档，没有把测试脚本安装为 Natsume 产品代码。

关键脚本可复核原型的具体断言；其中 `boot-tests.py`、`home-lifecycle.py`、`final-checks.py` 包含破坏比赛 Home、终止会话或重启测试 VM 的操作，限此可丢弃 VM 使用。较早失败日志保留，用于区分初始采样、dconf 配置错误与最终通过结果。

原始日志、截图、配置与源码需随验收构建归档；临时目录和当前 VM 本身不是长期发布材料。正式实现应遵循 PRD 的受限 API、身份验证、超时和持久化契约，不能直接将 Python 原型当成发行依赖。
