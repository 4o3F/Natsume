# GNOME 双会话：镜像配置附录

更新：2026-09-10。现行镜像输入已进入正式 [packaging/image](../packaging/image/README.md)，并由 Client Deb 原样安装到 `/usr/share/natsume/image-integration/`。镜像构建读取该包内目录；本页负责解释归属和接入位置，配置正文以源文件为准，不再从被忽略的 `target-vm` 或文档代码块复制。

比赛角色 `contest` 对应系统用户 **teams**、Home **/home/teams**、dconf profile **natsume_teams**；waiting 用户不变。`gdm-contest`、`natsume-contest-admission` 和 Helper 子命令参数 `contest` 保留为固定角色入口。旧 VM 证据中的账号和 profile 保留历史名称，不算 teams 新镜像已经验收。

完整部署顺序见[主文](gnome-session-image-requirements.zh-CN.md)和随包 [README](../packaging/image/README.md)。[manifest.tsv](../packaging/image/manifest.tsv)列出每个输入的目标路径、模式和应用方法；系统目标 root 所有、目录 0755，waiting Home 文件按实际 waiting UID/GID 初始化。Client 持续安装，卸载场景不再作为镜像交付门槛。

| 方法 | 镜像构建操作 |
| --- | --- |
| copy | 安装完整的命名配置，先检查冲突的旧文件和覆盖层 |
| merge | 保留官方文件并合并片段，不能把片段覆盖成完整上游栈 |
| render | 替换目标路径或内容中的 UID/模板占位符，再安装 |
| initialize-home | 初始化 waiting 的独立环境文件，保留 Home 所有权和其他数据 |

<a id="client-files"></a>
## A. Client 直接安装的运行文件

| 安装位置 | 包内来源 |
| --- | --- |
| `/etc/pam.d/gdm-contest` | [固定比赛入口](../packaging/client/rootfs/etc/pam.d/gdm-contest)，仅允许 teams |
| `/etc/pam.d/gdm-waiting` | [固定等待入口](../packaging/client/rootfs/etc/pam.d/gdm-waiting)，仅允许 waiting |
| `/etc/pam.d/natsume-contest-admission` | [Home 门禁](../packaging/client/rootfs/etc/pam.d/natsume-contest-admission) |
| `/usr/lib/systemd/system/natsume-session-prepare@.service` | [固定 prepare unit](../packaging/client/rootfs/usr/lib/systemd/system/natsume-session-prepare@.service)，实例名使用角色 |
| `/usr/lib/systemd/user/org.gnome.Kiosk.Script.service.d/50-natsume.conf` | [Agent 启动 drop-in](../packaging/client/rootfs/usr/lib/systemd/user/org.gnome.Kiosk.Script.service.d/50-natsume.conf) |
| `/usr/share/natsume/image-integration/` | [镜像集成输入](../packaging/image/README.md)，由镜像构建进一步应用 |
| Helper/Daemon unit、IPC policy、sysusers/tmpfiles、程序 | [Deb 清单](../packaging/client/nfpm.yaml) |

`/etc/natsume/config.toml` 和 `/etc/natsume/trust/{control-ca,local-origin-ca}.crt` 由部署方独立提供，autoinstall 在目标系统中直接安装完整配置和证书；通用 Client Deb 不包含这些文件。固定路径、权限和检查要求见[站点输入约定](../packaging/image/inputs.md#2-公共站点配置与身份)。

以下 B～F 的系统目标由镜像复制/合并/生成；Deb 只安装它们的输入副本，不自动写入这些目标。账号和最终 Home 内容依赖具体安装源，不能由包安装脚本猜测。输入随 Client 版本升级后，镜像须在维护期完成对应系统配置交接。

<a id="gdm-config"></a>
## B. IMG-02：GDM、停止顺序和 VT

GDM 每次启动前启用 waiting 自动登录，waiting 进入 PostLogin 后关闭运行态自动登录并 HUP 重读；后续受控重建使用固定 API。只靠静态 AutomaticLoginEnable=true 未能覆盖旧 VM 的等待会话恢复场景。PostLogin 是登录流程钩子，不是桌面首帧就绪报告。

合并实际被选择的 Default/display/hostname 钩子，保留既有逻辑及上游 ExecStartPre。AccountsService 为 waiting 选择 gnome-kiosk-script-xorg、teams 选择 ubuntu-xorg，确认对应 xsessions 文件存在。session scope 前缀作用于管理员会话，检查停止顺序无环；NAutoVTs=0、ReserveVT=6 保留管理员 tty6。配置通过维护期正常重启生效。

| 输入源 | 目标 | 模式 | 方法 |
| --- | --- | --- | --- |
| [fragments/accountsservice/teams](../packaging/image/fragments/accountsservice/teams) | `/var/lib/AccountsService/users/teams` | 0600 | merge |
| [fragments/accountsservice/waiting](../packaging/image/fragments/accountsservice/waiting) | `/var/lib/AccountsService/users/waiting` | 0600 | merge |
| [fragments/gdm/PostLogin.sh](../packaging/image/fragments/gdm/PostLogin.sh) | `/etc/gdm3/PostLogin/Default` | 0755 | merge |
| [fragments/gdm/custom.conf](../packaging/image/fragments/gdm/custom.conf) | `/etc/gdm3/custom.conf` | 0644 | merge |
| [rootfs/etc/systemd/logind.conf.d/60-natsume-graphical-vts.conf](../packaging/image/rootfs/etc/systemd/logind.conf.d/60-natsume-graphical-vts.conf) | `/etc/systemd/logind.conf.d/60-natsume-graphical-vts.conf` | 0644 | copy |
| [rootfs/etc/systemd/system/gdm.service.d/20-waiting-autologin.conf](../packaging/image/rootfs/etc/systemd/system/gdm.service.d/20-waiting-autologin.conf) | `/etc/systemd/system/gdm.service.d/20-waiting-autologin.conf` | 0644 | copy |
| [rootfs/etc/systemd/system/session-.scope.d/50-gdm-stop-order.conf](../packaging/image/rootfs/etc/systemd/system/session-.scope.d/50-gdm-stop-order.conf) | `/etc/systemd/system/session-.scope.d/50-gdm-stop-order.conf` | 0644 | copy |

<a id="pam-config"></a>
## C. IMG-03：PAM、SSH 和 polkit

PAM 前缀在官方栈之前，保留完整原栈。gdm-password 的片段依次包含普通入口拒绝和 admission；gdm-autologin 只加 admission。不能给 gdm-contest、gdm-waiting、gdm-autologin、systemd-user 添加普通入口拒绝。

`@MANAGED_GRAPHICAL_SERVICE@` 为 gdm-fingerprint 及所有已安装 smartcard 实现；所测版本有 gdm-smartcard-pkcs11-exclusive、gdm-smartcard-sssd-exclusive、gdm-smartcard-sssd-or-password，保留 gdm-smartcard alternatives 链接。`@ROOT_TOOL_SERVICE@` 只展开为实际缺少 account 段的 runuser/runuser-l。other 的三阶段拒绝覆盖缺失服务或缺失阶段，root 维护工具需显式接回 common-account。

通过 pam-auth-update 启用 natsume-managed-sessions，检查生成栈的跳转数和实际 allowlist。SSH 禁止 teams/waiting 的密码、密钥、证书入口；polkit 禁止受管用户管理系统、账号及重新开启 linger。上游 PAM 作为 conffile 合并，不使用 dpkg-divert。管理员维护、cron/at 实际行为和固定登录分别验收。

| 输入源 | 目标 | 模式 | 方法 |
| --- | --- | --- | --- |
| [fragments/pam/gdm-autologin](../packaging/image/fragments/pam/gdm-autologin) | `/etc/pam.d/gdm-autologin` | 0644 | merge |
| [fragments/pam/gdm-managed-entry](../packaging/image/fragments/pam/gdm-managed-entry) | `/etc/pam.d/@MANAGED_GRAPHICAL_SERVICE@` | 0644 | merge |
| [fragments/pam/gdm-password](../packaging/image/fragments/pam/gdm-password) | `/etc/pam.d/gdm-password` | 0644 | merge |
| [fragments/pam/other](../packaging/image/fragments/pam/other) | `/etc/pam.d/other` | 0644 | merge |
| [fragments/pam/runuser-account](../packaging/image/fragments/pam/runuser-account) | `/etc/pam.d/@ROOT_TOOL_SERVICE@` | 0644 | merge |
| [fragments/ssh/managed-users.conf](../packaging/image/fragments/ssh/managed-users.conf) | `/etc/ssh/sshd_config.d/60-natsume-managed-users.conf` | 0644 | merge |
| [rootfs/etc/pam.d/natsume-managed-entry-deny](../packaging/image/rootfs/etc/pam.d/natsume-managed-entry-deny) | `/etc/pam.d/natsume-managed-entry-deny` | 0644 | copy |
| [rootfs/etc/polkit-1/rules.d/00-natsume-managed.rules](../packaging/image/rootfs/etc/polkit-1/rules.d/00-natsume-managed.rules) | `/etc/polkit-1/rules.d/00-natsume-managed.rules` | 0644 | copy |
| [rootfs/usr/share/pam-configs/natsume-managed-sessions](../packaging/image/rootfs/usr/share/pam-configs/natsume-managed-sessions) | `/usr/share/pam-configs/natsume-managed-sessions` | 0644 | copy |

<a id="desktop-config"></a>
## D. IMG-04：dconf、输入、缩放与睡眠

按目标 root 的实际 passwd 解析 `@WAITING_UID@`、`@TEAMS_UID@`。例如 teams UID=1001 时，`user@@TEAMS_UID@.service.d` 渲染为 `user@1001.service.d`。不使用宿主 UID。

保留 Kiosk 的官方 compiled file-db，比赛 profile 保留镜像所需的已有数据库层。数据库与 locks 成套安装，在目标 root 执行 dconf update；核对真实用户 manager/桌面环境及 gsettings get/writable。waiting 必须保留 disable-log-out=false，普通退出快捷键单独禁用。

waiting/teams 共用 US 英文键盘默认值，不新增中文输入法；保留能够正常显示中文的字体，并在实际用户总线/profile 下核对有效输入源。GDK_SCALE=1 只随 Kiosk compositor/子进程。

保持四项睡眠禁止策略。验证英文输入、中文字体显示、DPI、点击区域、首帧、660 秒无输入输出、正常退出及前台物理键鼠；XInput2 条件见[IMG-04](gnome-session-image-requirements.zh-CN.md#img-04-readiness)。QEMU 试验驱动/软件光标不是镜像生产输入。

| 输入源 | 目标 | 模式 | 方法 |
| --- | --- | --- | --- |
| [rootfs/etc/dconf/db/natsume-session.d/00-session](../packaging/image/rootfs/etc/dconf/db/natsume-session.d/00-session) | `/etc/dconf/db/natsume-session.d/00-session` | 0644 | copy |
| [rootfs/etc/dconf/db/natsume-session.d/locks/session](../packaging/image/rootfs/etc/dconf/db/natsume-session.d/locks/session) | `/etc/dconf/db/natsume-session.d/locks/session` | 0644 | copy |
| [rootfs/etc/dconf/db/natsume-waiting.d/00-waiting](../packaging/image/rootfs/etc/dconf/db/natsume-waiting.d/00-waiting) | `/etc/dconf/db/natsume-waiting.d/00-waiting` | 0644 | copy |
| [rootfs/etc/dconf/db/natsume-waiting.d/locks/waiting](../packaging/image/rootfs/etc/dconf/db/natsume-waiting.d/locks/waiting) | `/etc/dconf/db/natsume-waiting.d/locks/waiting` | 0644 | copy |
| [rootfs/etc/dconf/profile/gnomekiosk](../packaging/image/rootfs/etc/dconf/profile/gnomekiosk) | `/etc/dconf/profile/gnomekiosk` | 0644 | copy |
| [rootfs/etc/dconf/profile/natsume_teams](../packaging/image/rootfs/etc/dconf/profile/natsume_teams) | `/etc/dconf/profile/natsume_teams` | 0644 | copy |
| [rootfs/etc/systemd/sleep.conf.d/do-not-suspend.conf](../packaging/image/rootfs/etc/systemd/sleep.conf.d/do-not-suspend.conf) | `/etc/systemd/sleep.conf.d/do-not-suspend.conf` | 0644 | copy |
| [rootfs/etc/systemd/user/org.gnome.Kiosk@x11.service.d/70-natsume-frame-scale.conf](../packaging/image/rootfs/etc/systemd/user/org.gnome.Kiosk@x11.service.d/70-natsume-frame-scale.conf) | `/etc/systemd/user/org.gnome.Kiosk@x11.service.d/70-natsume-frame-scale.conf` | 0644 | copy |
| [templates/user-manager-teams.conf](../packaging/image/templates/user-manager-teams.conf) | `/etc/systemd/system/user@@TEAMS_UID@.service.d/20-natsume-session.conf` | 0644 | render |
| [templates/user-manager-waiting.conf](../packaging/image/templates/user-manager-waiting.conf) | `/etc/systemd/system/user@@WAITING_UID@.service.d/20-natsume-session.conf` | 0644 | render |
| [templates/waiting-environment.conf](../packaging/image/templates/waiting-environment.conf) | `/home/waiting/.config/environment.d/99-natsume-session.conf` | 0644 | initialize-home |

<a id="xorg-config"></a>
## E. IMG-05：Xorg

合并 ServerFlags 并移除冲突的旧 90-natsume-test-kiosk.conf。两个 Xorg 的实际屏保 timeout 和 DPMS 三项均须为 0，普通 VT/终止快捷键被拒绝，Helper/logind 的受控激活保持可用。

| 输入源 | 目标 | 模式 | 方法 |
| --- | --- | --- | --- |
| [rootfs/etc/X11/xorg.conf.d/20-natsume-session.conf](../packaging/image/rootfs/etc/X11/xorg.conf.d/20-natsume-session.conf) | `/etc/X11/xorg.conf.d/20-natsume-session.conf` | 0644 | copy |

<a id="home-template"></a>
## F. IMG-06/08：模板与启动依赖

最终安装源的全部 skel、Browser/IDE 默认值完成后，由镜像生成版本化 SquashFS。其内部使用实际 teams UID/GID，保留 owner 写位以支持 copy-up。构建、版本/SHA-256、packages.tsv 和真实只读 loop 挂载契约见[随包实施要求](../packaging/image/integration.md#8-img-06每个最终安装源的-home-模板)及[Client 模板契约](../packaging/client/rootfs/usr/lib/natsume/home-templates/README.md)。

`@TEMPLATE_VERSION@` 是经过摘要校验的 sha256-目录名；`@TEMPLATE_MOUNT_UNIT@` 由 `systemd-escape --path --suffix=mount /usr/lib/natsume/home-templates/current/lower` 生成。保留 Helper Wants/After，模板失败不停止 GDM/waiting。

离线启用模板 mount、Helper、Daemon，不带 --now，不启动构建宿主服务。Live 专用禁用覆盖不得进入实际安装源。镜像直接创建 waiting/teams 和对应 Home，不提供重构前账号迁移。

| 输入源 | 目标 | 模式 | 方法 |
| --- | --- | --- | --- |
| [templates/helper-home-template.conf.in](../packaging/image/templates/helper-home-template.conf.in) | `/etc/systemd/system/natsume-privileged-helper.service.d/20-home-template.conf` | 0644 | render |
| [templates/home-template.mount.in](../packaging/image/templates/home-template.mount.in) | `/etc/systemd/system/@TEMPLATE_MOUNT_UNIT@` | 0644 | render |
