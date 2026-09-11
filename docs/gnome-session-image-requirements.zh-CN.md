# GNOME 双会话：镜像交付要求

更新：2026-09-10。本文是仓库内的镜像要求导航与摘要。向独立 image builder 交付时，完整打包 [packaging/image](../packaging/image/README.md) 即可：其目录内包含全部输入说明、IMG-01～08 实施要求、验收标准和独立检查器，接收方无需阅读本文或其他仓库文档。**IMG-01～08 均为必需交付项，由镜像项目集成并按最终产物验收。**

| 要查什么 | 文档 |
| --- | --- |
| 镜像要交付什么、按什么顺序做 | 本文 |
| 配置文件与机器可读部署清单 | [正式镜像输入](../packaging/image/README.md)、[manifest.tsv](../packaging/image/manifest.tsv)，随 Client Deb 交付 |
| 配置的路径、内容、权限和接入位置 | [配置附录](gnome-session-image-configuration.zh-CN.md)，属于本文的实施要求 |
| 验收用例、测量口径与证据要求 | [随包验收标准](../packaging/image/acceptance.md)、[PRD §11～12](prd-gnome-dual-session.zh-CN.md#11-非功能要求与测量口径) |
| 产品行为与组件职责 | [PRD](prd-gnome-dual-session.zh-CN.md)、[架构](architecture.md) |

更换官方发行版本时，可适配路径、包名和上游配置，但须保持行为并复验。镜像实施所需文件和上下文完整保存在 [packaging/image](../packaging/image/README.md)。

## 1. 已确定的运行方式与交付边界

系统账号固定为 `waiting` 与 `teams`，Home 为 `/home/waiting` 与 `/home/teams`。下文 `contest` 表示比赛角色；协议目标、固定 PAM 名和 Helper CLI 的角色参数保持 contest。Client 持续安装，卸载不作为交付门槛。

waiting 使用独立账号的官方 **GNOME Kiosk Script X11**，contest 使用另一账号的完整 **Ubuntu/GNOME X11**。两套会话及 Xorg 都由 GDM 管理。正常情况下双方同时存在，“显示等待界面／显示比赛桌面”只切换前台。Home reset 才先进入 waiting、结束并排空 contest、重置 Home，再通过固定入口重新登录 contest；重建期间允许 greeter 和闪屏。waiting 本期为纯黑全屏，同一 Agent 窗口承载 Binding；logo 和复杂 Skia 页面不构成本次镜像交付前置条件。

| 所有者 | 交付内容 | 实施边界 |
| --- | --- | --- |
| Natsume Client 包 | Helper、Daemon、Agent、固定登录 prepare unit、三个固定 PAM 文件、IPC policy、sysusers/tmpfiles、Kiosk Script 的 Agent drop-in、`/usr/share/natsume/image-integration/` 镜像输入 | 通过既有 API 编排，不在运行时重写 GDM/PAM/dconf，不直接托管 Xorg/GNOME |
| 镜像项目 | IMG-01～08：账号、官方桌面、系统配置、上游 PAM 接入、模板、旧流程退出、构建和维护流程 | 只使用官方发布的 GDM/GNOME/Xorg/systemd，不修改程序或资源，不用嵌套桌面 |
| 部署方 | 兼容的 Server/Client/镜像组合、公共端点与信任锚、独立管理员、注册和绑定 | 首次启动后按现有 Provisioning Gate 和人工审批完成 Enrollment，再进行 Binding |

Client Deb 内的镜像输入由 [packaging/image](../packaging/image/README.md)唯一维护。镜像构建读取包内 `manifest.tsv`，按 copy/merge/render/initialize-home 应用到目标 root；安装 Client 本身不自动接管上游 PAM/GDM、创建账号或生成最终模板。输入版本与 Client Deb 一致，具体应用依赖实际 UID、上游栈和最终 skel。

Client 文件来源见[附录 A](gnome-session-image-configuration.zh-CN.md#client-files)。镜像不得另复制一套固定 PAM 或 Agent 启动器，以免包升级后仍运行旧实现。

## 2. 实施顺序与交付总表

按以下顺序处理每个实际安装源；运行中系统的 Home 与配置变更另见[维护要求](#maintenance)。

1. 锁定兼容的 Client Deb、公共站点配置及官方依赖，预留管理员与受管账号，检查名称/UID/GID 冲突。
2. 安装通用 Client，确认包提供的 PAM、unit 和程序存在；将完整 config.toml 和两份公共 CA 留给 autoinstall 落地，并从包内 `/usr/share/natsume/image-integration/` 接入 GDM/PAM/登录入口配置。
3. 安装 dconf、英文键盘、中文字体、Xorg、VT 和退出顺序配置；退出旧 OOBE、登录、锁屏和 Agent 启动链。
4. 所有桌面、语言、Browser/IDE 层写完 `/etc/skel` 后，生成该安装源的正式模板，校验摘要并安装 mount 与 Helper drop-in。
5. 在目标 root 中离线 enable；分离 Live 安装环境与实际安装源；检查没有带入机器身份和运行态数据。
6. 构建新镜像，从零安装后按[交付验收](#handoff)执行，提交版本、配置和证据清单。

| 编号 | 必须交付的结果 | 配置入口 | 当前证据边界／验收清单 |
| --- | --- | --- | --- |
| [IMG-01](#img-01) | 两个受管账号和独立管理员，禁止未受控入口 | 正文及附录 C | VM 账号可用；最终账号与安装器冲突检查待镜像，2.1 |
| [IMG-02](#img-02) | 双 X11 桌面、GDM 启动自动 waiting、停止顺序和 VT 配置 | 附录 B | VM 启动、恢复和维护已验证；最终交付待镜像，2.2、AT-01、7.2 |
| [IMG-03](#img-03) | PAM 全阶段门禁、普通入口限制、缺失服务 fallback、管理员维护 | 附录 C | VM 真实入口和包生命周期已验证；新镜像须组合复验，2.3～2.5、7.3/7.4/7.7/7.8 |
| [IMG-04](#img-04) | 生效的 dconf、英文键盘、中文字体、缩放和禁用睡眠策略 | 附录 D | VM 首帧、缩放、退出和输入就绪已有验证；英文输入与中文显示按当前交接包复验，驱动范围见正文，6.1～6.8 |
| [IMG-05](#img-05) | 双 Xorg 禁用普通 VT/终止快捷键及自动黑屏 | 附录 E | VM 真实快捷键和受控切换通过；最终交付须复验，6.2～6.4 |
| [IMG-06](#img-06) | 最终安装源的只读版本化 Home 模板及挂载依赖 | 附录 F | VM 机制与隔离原型通过；最终 Browser/IDE 内容未交付，2.6、2.7 |
| [IMG-07](#img-07) | 旧控制链退出，Agent 只由官方 Kiosk 用户服务管理 | 正文清理清单 | Client 旧入口已移除、VM 交接通过；最终镜像待清点，1.6、2.8 |
| [IMG-08](#img-08) | 可复现构建、离线启用、Live/安装源分离和维护交付 | 正文构建与维护要求 | VM 包流程已有证据；完整新装和发行验收未完成，2.9、7.1、8.1～8.7 |

<a id="img-01"></a>
## 3. IMG-01：账号

- 固定用户名 `teams`、`waiting`，Home 为 `/home/teams`、`/home/waiting`；独立 UID、用户 manager、总线和 Xauthority。reset 不删除重建账号，也不清理 waiting Home。
- UID/GID 由最终镜像确定，不能复用不相干的同名账号。旧 VM 用户名和数值仅为历史测试取值；按实际 teams/waiting 账号生成 UID 相关 systemd 配置和模板所有权，校验 Home、shell、附加组。
- 安装器/autoinstall 为管理员保留第三个用户名及 UID，拒绝以两个受管账号作为初始管理员；两者不得进入 sudo 等管理组。管理员 SSH/TTY 维护路径必须可用。
- 锁定受管账号密码，并落实 IMG-03 的 SSH 密钥/证书、TTY、指纹/智能卡、cron/at 和 polkit 限制。仅锁密码或隐藏用户列表不满足要求。
- 双方 `Linger=no`，禁止普通用户重新开启；排查绕过 PAM 的系统定时任务或 `User=teams/waiting` 服务，防止排空后重新产生进程。
- waiting Home 最小化，不复制完整比赛 skel，只初始化必要显示环境；比赛默认配置进入 IMG-06 模板。

验收：账号隔离和冲突拒绝、固定登录正例、普通入口负例及管理员正例均须实际执行。contest 许可关闭时 waiting 仍能建立，维护排空后 UID 不被非管理入口重新拉起。

<a id="img-02"></a>
## 4. IMG-02：GDM、桌面、停止顺序与 VT

安装官方 `gdm3`、`libgdm1`、`gnome-kiosk`、`gnome-kiosk-script-session`、完整 Ubuntu/GNOME X11 session、Xorg 及运行依赖。当前验证参考为 GDM 46.2、Kiosk 46.0，不要求永久锁死这两个版本；交付记录实际版本并复验兼容性。

`/etc/gdm3/custom.conf` 关闭 Wayland、固定自动登录 waiting、关闭 timed login。AccountsService 为 waiting 选择 `gnome-kiosk-script-xorg`，teams 选择 `ubuntu-xorg`，确认对应 `/usr/share/xsessions/*.desktop` 存在。内容见[附录 B](gnome-session-image-configuration.zh-CN.md#gdm-config)。

<a id="img-02-autologin"></a>
### 每次 GDM 启动只初始化一次自动登录

必须同时交付 `gdm.service.d/20-waiting-autologin.conf` 和 GDM `PostLogin` 钩子：GDM 启动前用官方 `gdm-runtime-config` 开启自动登录；waiting 登录时关闭运行态自动登录并向 GDM 发 HUP。此后 contest 准备和 waiting 恢复由 Client 固定入口完成。仅有静态 `AutomaticLoginEnable=true` 不足以覆盖所测 waiting 故障恢复：GDM 曾对后续 display 重复自动登录 waiting，阻挡受控登录。

合并已有 `ExecStartPre`、PostLogin 和 display/hostname 专用钩子，保证实际选中的钩子执行该逻辑；不得覆盖官方启动列表或假定 `Default` 一定执行。运行态文件由官方工具维护，不直接覆盖 `/run/gdm3/custom.conf`。最终镜像检验钩子失败、GDM 重启、包升级和 waiting 恢复后再次 reset/登录。

<a id="img-02-stop-order"></a>
### 停止顺序

交付 `/etc/systemd/system/session-.scope.d/50-gdm-stop-order.conf`，设置 `Before=display-manager.service`。停止依赖按反向执行，使 GDM 先于 session scope 结束，避免 worker 在 VT 等待中拖到原 90 秒超时。该前缀配置也作用于管理员 scope，须检查实际依赖和管理员退出，无 ordering cycle。它只指定顺序，不要求登录时启动 GDM；不能改小超时掩盖问题。

<a id="img-02-vt"></a>
### 图形 VT 与 getty

交付 `/etc/systemd/logind.conf.d/60-natsume-graphical-vts.conf`，设置 `NAutoVTs=0`、`ReserveVT=6`，避免自动 getty 与新图形 session 抢占 VT；保留 tty6 管理员维护。通过维护期正常重启生效，不在运行的图形会话中重启 logind。

保留官方 GDM 的 `/run/user/<uid>/gdm/Xauthority`，以及角色读取自身合成器进程环境、访问自身 Xorg/总线的能力。greeter 可能使用 `dbus-run-session`；Client 在 gdm UID 下查询其 `org.gnome.SessionManager.IsSessionRunning`，不能强制假定总线为 `/run/user/<gdm uid>/bus`。不新增跨用户环境读取权限或任意显示/总线地址。

验收：无 Server/Target 冷启动 waiting、健康 Helper 重启复用、waiting 有界恢复后的登录/reset、GDM 维护重启及管理员 SSH/tty6。模板失败时 Helper 诊断和 waiting 继续可用；不增加 keeper、后台 `gnome-session`、全局 Home gate 或让 GDM 依赖模板成功。

<a id="img-03"></a>
## 5. IMG-03：PAM 与全部登录入口

三个固定 PAM 文件由 Client 包维护；镜像先安装 Client，再将 `@include natsume-contest-admission` 前置到官方 `gdm-autologin`、`gdm-password`。门禁覆盖 auth、account、open_session，不能被前面的 sufficient 分支绕过；非 contest 和 close_session 沿用固定门禁语义。

<a id="img-03-entries"></a>
以下矩阵是最终行为，内容及接入位置见[附录 C](gnome-session-image-configuration.zh-CN.md#pam-config)。

| 入口 | waiting | contest | 实施要求 |
| --- | --- | --- | --- |
| 固定 `gdm-waiting` / `gdm-contest` | 仅 waiting 对应入口 | 仅 contest 对应入口，且 Home/登录许可有效 | 用 Client 原文件，不能套普通入口拒绝策略 |
| `gdm-autologin` | GDM 启动初始化允许 | 不配置 contest 自动登录；固定入口引用时仍须过门禁 | 前置 admission，保留官方栈 |
| 普通 `gdm-password`、指纹、全部已安装智能卡实现 | 拒绝 | 拒绝 | 前置镜像自有 `natsume-managed-entry-deny`，覆盖 alternatives 实际目标 |
| SSH 密码/密钥/证书、TTY、cron、at | 拒绝 | 拒绝 | SSH `DenyUsers` 加 account 策略；清理可执行的既有受管定时任务 |
| `systemd-user` | 允许官方后续检查 | 允许官方后续检查 | 不能因全局 account 限制阻断用户 manager |
| 真正 root 调用 `runuser`、`runuser-l`、`su`、`su-l` | 允许维护 | 允许维护 | 校验调用 UID，具名服务须有正确 account 栈；普通用户不放行 |
| 缺失具名 PAM 服务或缺失阶段，落入 `other` | 拒绝 | 拒绝 | 镜像完整配置 auth/account/session fallback |

<a id="img-03-account"></a>
account 策略通过镜像 `pam-auth-update` profile 进入 `common-account`，检查生成后的跳转计数和 allowlist；它不能替代三阶段 Home 门禁或普通图形入口前置拒绝。非受管管理员继续发行版认证流程，root 正例与普通用户负例分别验证。

镜像 polkit 规则对受管用户拒绝 login1、AccountsService、systemd1、udisks2 管理操作，覆盖 linger 重启用。`runuser`/`runuser-l` 若上游没有 account 段，补 `account include common-account`；否则新增 `other` 会误伤 root 维护。新版本已有 account 段时合并验证，不能机械追加。

上游 PAM 文件按 conffile 维护：升级比较旧/新官方栈、保留并合并站点配置、重新验证。**不使用 `dpkg-divert`、无条件 `--force-confnew` 或永久冻结旧官方栈。** `--force-confold` 不能替代合并。镜像自有拒绝文件、profile 和 `other` 接入须完整交付。

验收：真实 auth/account/open_session、密码/密钥、全部已安装图形认证分支、root 维护、普通用户 polkit、cron/at payload，以及具名服务或阶段缺失时的 fallback。移除 at 时确认无活动任务，保留则测拒绝。若有旧 login 配置引用不存在的 `pam_lastlog.so`，按所选官方版本修正。不能只以文本或 account 单阶段测试代替完整登录。

<a id="img-04"></a>
## 6. IMG-04：dconf、键盘、字体、缩放与睡眠

交付[附录 D](gnome-session-image-configuration.zh-CN.md#desktop-config)的两个 profile、公共/专用数据库与 locks、按 UID 配置的用户 manager 环境，以及缩放 drop-in。执行 `dconf update`，维护期重建会话，检查真实进程环境和有效设置。

- waiting 使用官方 Kiosk 实际选择的 `gnomekiosk` profile，保留官方 compiled 数据库；contest 使用 `natsume_teams`，保留镜像已有系统数据库层。只新建 Kiosk 不读取的 profile 无效。
- 双方关闭自动锁屏、用户切换、空闲调暗/睡眠和锁屏快捷键；waiting 另设纯黑背景，禁用普通关闭、应用切换和运行对话框。**保留 `disable-log-out=false`**，true 会阻碍 GNOME 正常结束；普通退出快捷键单独禁用。
- waiting/teams 默认只使用 US 英文键盘，不额外安装中文输入法。中文字体须正常显示，例如保留发行版 `fonts-noto-cjk` 提供的 Noto Sans CJK；首次启动后核对两种会话的有效输入源。
- 不部署专用 IBus 配置；`GDK_SCALE=1` 只给 Kiosk compositor 及其子进程，不把 frame-helper 的固定倍率传给 Agent/contest。
- 保留 `/etc/systemd/sleep.conf.d/do-not-suspend.conf` 四项睡眠禁止值。临时放行 S3 是故障测试，不是生产配置。

<a id="img-04-readiness"></a>
前台就绪要求实际 XInput2 slave 键盘和指针已启用，并带 `/dev/input/eventN` 的 `Device Node`；后台物理输入禁用是正常状态。保留官方输入驱动、属性和自身 Xorg 查询能力，不用虚拟 XTEST 替代、不硬编码设备编号。最终仍须实际画面和键鼠操作，不能只看 ready=true。

Binding 英文输入、焦点、中文显示、缩放及恢复后的真实键鼠必须在交付镜像上验证。测试环境的显卡、内核或软件光标配置不能直接作为生产镜像要求；验证报告应明确硬件、驱动、倍率和字体的覆盖范围。

验收：有效 dconf 值及 locked 状态、660 秒无输入持续输出、正常退出无旧超时、英文输入/中文字体/焦点/点击区域/首帧、显示故障后输入与恢复；睡眠策略和恢复证据分开记录。

<a id="img-05"></a>
## 7. IMG-05：Xorg

交付[附录 E](gnome-session-image-configuration.zh-CN.md#xorg-config)的 `ServerFlags`：`DontVTSwitch=true`、`DontZap=true`，Blank/Standby/Suspend/Off 时间为 0。合并并移除冲突旧片段，特别是 `90-natsume-test-kiosk.conf`；不能只增加一个更早的文件。

验收：读取两个 Xorg 的 `xset q` 和日志，屏保 timeout 与 DPMS 三项为 0；`timeout: 0` 时 `cycle: 600` 不代表自动黑屏。实际 Ctrl+Alt+Fn、Ctrl+Alt+Backspace 不切出/终止会话，Helper/logind 受控激活仍有效。这不是完整恶意程序隔离承诺。

<a id="img-06"></a>
## 8. IMG-06：正式 Home 模板

模板输入是**该安装源所有层完成后的 `/etc/skel` 和受管 Browser/IDE 默认配置**，不能只捕获 minimal、复用 Live 层或从已使用的 contest Home 制作。每个安装源分别生成并记录来源，禁止带入 cookies、密钥、凭据、比赛文件。

布局、mount 和校验见[附录 F](gnome-session-image-configuration.zh-CN.md#home-template)。`sha256-<64 位小写十六进制>` 版本对应 `home.squashfs` 实际摘要，附 `SHA256SUMS`、`packages.tsv` 和 `current/version`。元数据由 root 控制，无符号链接，组和其他用户不可写。`current/lower` 是真实目录上的唯一只读 SquashFS loop 挂载，无叠加/子挂载。

lower 的 UID/GID 与 contest 一致，保留正常 owner 写位以支持 OverlayFS copy-up；只读依靠挂载实现。固定 mount 通过 Helper `Wants/After` 排在 Home 处理前，**不通过 Requires/global gate 使模板失败停止 Helper、GDM 或 waiting**。Helper 检查源、元数据、挂载和 Home 实际可写性；完整内容散列由可信构建/安装边界校验，不要求逐 tick 重读镜像。

模板更新须在维护窗口排空 contest、正常卸载旧 Home 后执行；不能替换健康 OverlayFS 下的 lower、强制/lazy 卸载、删除窗口或伪造 epoch。外部 UID 占用和空间不足时保留现场及同一维护责任，修复后继续。

验收：Browser/IDE 默认配置可用；修改默认文件并写 canary，reset 后恢复默认且 canary 消失。缺模板、错误摘要/所有权/源版本、非只读、Home 不可写、busy、中断分别检查拒绝和恢复。正式 20 个 reset 使用最终模板；旧空 lower 或 VM 旧 skel 次数不计入。

<a id="img-07"></a>
## 9. IMG-07：退出旧控制链

清点并停用下列已知入口及同类项，记录最终处置。可移入 root 私有维护备份，不得保留开机依赖或受管用户可触发入口。

| 类别 | 需要清点的入口 |
| --- | --- |
| 自动改写 GDM／控制受管会话 | `machine-setup-oobe.service`、`hydro-machine-tools.service`、OOBE user autostart、machine-tools probe |
| 旧登录/锁屏工具 | `/usr/local/sbin/` 和 `/usr/local/share/icpc/scripts/` 中的 `login-user`、`logout-user`、`lock-user`、`unlock-user` |
| 旧 Client/实验启动 | 全局 XDG `org.natsume.SessionAgent.desktop`、bootstrap/Home 服务、全局 `display-manager.service.d/50-natsume-home.conf`，包括 `/etc` 残留覆盖 |
| 重复应用管理 | Kiosk 示例脚本编辑器、另一个 Agent 启动/重启所有者、waiting 继承的比赛自启动 |

Client 已删除自身旧全局入口，镜像仍须清点历史副本。Agent 只由官方 Kiosk Script 用户服务和 Client drop-in 启动。waiting 用静态页面，去掉不必要动画和比赛自启动，保持两个用户/Xorg 独立。

验收：重启与升级后只有新控制链操作受管会话，无旧配置写入竞争、重复 Agent 或进程累积。当前用 QEMU 测功能和资源变化，不增加物理机前置门槛；Intel 核显型号未知，不将软件渲染数据换算为核显损耗百分比。

<a id="img-08"></a>
## 10. IMG-08：构建、安装与首次启动

- 输入使用完整通用 `natsume-client` Deb，核对版本、架构和 SHA-256，按包依赖安装运行库；完整 Client config.toml 和两份正式 CA 由部署方独立生成，在 autoinstall 中落地并检查匹配关系，不使用包内测试 CA。Client 重装或 purge 不管理这些镜像文件；只复制二进制不算包部署。
- Client 包只提供配置示例，不在 `/etc` 安装占位配置。autoinstall 直接写完整 `/etc/natsume/config.toml`：`[server]` 保存 IP 字面量和端口，`[site]` 保存站点命名空间及 Gateway 域名。包安装、重新配置和卸载都不改写配置；缺配置可预装，已提供的空文件或不可读文件使包配置失败，内容由 Client 启动时校验。
- 缓存键覆盖 Deb 的**实际内容**及依赖层/镜像配置；同名文件内容变化须失效。自定义配置目录的宿主输入、缓存键、chroot 只读挂载必须一致；完整部署配置和两份 CA 只进入 autoinstall，不进入镜像层键，修改后重新生成部署输入。禁用 Natsume 的安装源不依赖此 Deb。
- Client postinstall 做 sysusers/tmpfiles 和适用的 daemon-reload，**不负责开机 enable**。镜像在目标 root 离线启用 `natsume-privileged-helper.service`、`natsume-device-daemon.service`、模板 mount；GDM 按发行版设为显示管理器。Caddy 由 Daemon 管理，prepare unit 不单独 enable，Agent 随官方 Kiosk session 启动。
- 构建不启动 GDM、用户 manager、Client，不把运行主机 `/run` 绑定进离线 root。按发行版首次启动机制准备 machine-id；不克隆设备身份、私钥、Enrollment、Binding 或 Home 维护状态。
- **实际安装源**保留固定 waiting 及 Client/模板启动；**Live 层**保留 Casper 临时管理员和可见安装器，关闭该层 Client/模板自动启动和继承的 waiting 自动登录。Live 专用覆盖不能复制进已安装系统。

验收：构建失败保护、同输入复现和内容变化失效；从 Live 完成真实安装，首次启动生成本机身份，人工审批和绑定后形成最小业务闭环。包非交互安装与系统正确启动分别验收。

<a id="maintenance"></a>
## 11. 升级与回退

本期按当前数据库、Home 窗口格式和 waiting/teams 账号全新部署，不提供重构前版本迁移。在用工位维护时先用独立管理员入口，让当前 Helper 完成已捕获的 Home 窗口，再停会话并处理模板和软件包。不得删除进度绕过恢复。模板、上游 PAM 或 GDM 配置变化后，先合并检查再显式启动服务/GDM；包脚本不自动重启桌面。

回退使用经过验证的完整 Server/Client、数据库与密钥、站点和模板配置备份，保留设备身份、Binding、Home 和完成 epoch，不单独降级 Helper 或混用状态。具体维护步骤见[交接包](../packaging/image/integration.md#11-维护与回退)；包安装成功不代表整套交接成功。

<a id="maintenance-remove"></a>
Client 持续安装，remove/purge 不作为镜像实现或交付门槛。`other` 拒绝仍是必需 PAM 配置，因为具名服务或某阶段缺失也会触发 fallback。

<a id="handoff"></a>
## 12. 新镜像交接与最终验收

镜像项目交付以下材料，才能启动正式发行验收：

1. 镜像下载地址、SHA-256、源码 revision、安装源/构建方式；兼容 Server/Client 版本和 Deb 摘要。
2. 官方内核、GDM、GNOME/Kiosk、Xorg、PAM、中文字体及依赖的包版本；IMG-01～08 配置清单、内容摘要、权限与归属。
3. 受管 UID/GID、管理员保留规则；每个安装源的模板来源、版本/SHA-256、包清单、mount 与 Helper 依赖。
4. Live/安装源分离、离线 enable、首次身份初始化、旧控制链清理的结果，以及升级/回退流程。
5. 逐项验收和失败记录；证据关联实际镜像/包/模板版本，不含设备私钥或比赛数据。

正式验收必须基于同一组最终镜像、Server/Client、模板和配置。历史 VM 结果仅证明当时的被测组合；teams 新镜像需要完成以下验证。

| 顺序 | 新镜像通过标准 |
| --- | --- |
| 1. 镜像交付 | 账号/桌面、最终模板、旧链清理、构建与安装层次落地 |
| 2. 从零安装和首次启动 | 正式 Deb/权限/依赖、离线 enable；无 Target 首个稳定业务界面为 waiting；Browser/IDE 和业务闭环可用 |
| 3. 正式重复与耗时 | 100 轮切换 P95 ≤3 秒；20 个 reset 各 ≤120 秒；10 次启动；允许 GDM 登录至 contest 真实 ready ≤30 秒，按 PRD 测量口径 |
| 4. 同一最终基线签收 | PRD AT-01～26 全部签收，检查实际帧/输入、会话保持和进程累积，完成发行证据、runbook 与 WP10 |

PAM、英文输入、中文显示、恢复和包生命周期按[随包验收标准](../packaging/image/acceptance.md)复验。最终产物和证据完成后才能更新发行签收。
