# 镜像实施要求：IMG-01～08

按本文完成 image builder 的全部修改。配置正文在同目录的 [manifest.tsv](manifest.tsv) 及其列出的文件中；依赖、站点值和 Client 文件归属见 [inputs.md](inputs.md)，通过标准见 [acceptance.md](acceptance.md)。

## 1. 行为与所有权

| 固定业务角色 / Helper CLI 参数 | Unix 用户 | Home | X11 session | dconf profile |
| --- | --- | --- | --- | --- |
| waiting | waiting | /home/waiting | gnome-kiosk-script-xorg | gnomekiosk |
| contest | teams | /home/teams | ubuntu-xorg | natsume_teams |

两个会话各有 Xorg、GNOME、用户总线、Xauthority 和 Home，都由官方 GDM 创建及管理。正常情况下两边同时存在，普通“显示等待界面/显示比赛桌面”只切换前台，保留双方 session/PID、比赛窗口、文件和 Home generation；不执行 GNOME Lock/Unlock。后台比赛会话合法。`gdm-contest`、`natsume-contest-admission`、`prepare-session contest`、协议 `foreground_target=contest` 是角色标识，不能改名。

Home reset 的职责归 Natsume：先撤销上游业务访问、取得可用 waiting 前台，再关闭 teams 登录许可，结束捕获的比赛会话并排空 UID/manager/PAM worker，正常卸载并恢复 Home，验证后重新登录比赛会话，最后依最新目标决定前台。waiting/GDM 不随普通 reset 重启。重建期间允许 greeter/闪屏；Home 失败必须保留 waiting 与诊断，不能让整个 GDM 依赖 Home 成功。

waiting 的同一 Agent 窗口承载 Binding 与已绑定队伍／学校／Logo 展示，断网时保留非秘密缓存并标离线。学校 Logo 从 Natsume Server 拉取，镜像不预置学校图；缺图使用随包默认图。Client Deb 提供 Noto CJK 字体依赖和 `/var/lib/natsume-display`（natsume 可写、waiting 只读），私有展示记录留在 `/var/lib/natsume/state/waiting.json`。Agent 的启动与重启只归官方 Kiosk Script 用户服务；模板和图形配置归 image builder，运行时不再修改它们。Client 持续安装，不新增卸载流程。

## 2. 在 image builder 中安排修改位置

按构建阶段组织实现；下面以 `icpc-contest-image` 为例。Natsume 固定预装在 extra 层，安装器只暴露 minimal + standard + extra 的完整安装源；其他 builder 按同样的最终安装源边界安排。

| 位置 | 修改责任 |
| --- | --- |
| 构建参数、配置目录 | 构建提供完整通用 Client Deb；完整 config.toml 和两个公共 CA 文件仅由部署方交给 autoinstall，构建侧没有启用开关或手填摘要 |
| `lib/stamp.sh` 或实际缓存键实现 | 将 Deb 实际内容/模式、交接目录和三个站点公共文件计入 extra 和后继层；同名文件内容变化须失效，部署端点不参与层键 |
| `lib/chroot.sh` / 构建入口 | 配置读取、缓存键和 chroot 只读输入挂载使用同一个实际 CONFIG_DIR；不能仍绑定硬编码仓库 config；构建用独立临时 /run |
| IDE 层之后的 extra 模块，如 `310-natsume` | 安装官方依赖和完整 Client，注入并检查站点公共文件，创建受管账号，应用除最终模板以外的 IMG 配置，离线 enable |
| 旧 OOBE、streaming、monitoring 模块 | 移除旧 OOBE、probe、会话工具、VLC 推流及独立 exporter；工位监控由 Natsume 负责 |
| 每个安装源最后的模块，如 `930-natsume-home` | 在 extra 的全部 skel/Browser/IDE 配置完成后生成完整安装源模板；其他构建层跳过 |
| `810-installer` / Live 层 | 保留 Casper 临时管理员和可见安装器；关闭 Live 中继承的 Client/模板启动与 waiting 自动登录 |
| 安装器/autoinstall | 在 /target 配置真实 Server IP/端口；保留 waiting/teams 名称和 UID，另建管理员；确保安装复制的是正确安装源，Live 覆盖不泄漏到安装后的系统 |
| 构建/运维文档及验证 | 记录 Client 输入、最终配置、镜像/模板版本和本包验收结果，不照搬旧 VM 的完成状态 |

先实现构建前置检查 → 账号/Client/桌面 → 配置合并 → 旧链退出 → 最终模板 → 离线 enable/Live 分离 → 新镜像验收。构建不得启动 GDM、logind、用户 manager 或 Client，也不得将运行主机的 /run 绑定进目标 root。

## 3. IMG-01：账号与入口隔离

新镜像创建 waiting/teams，使用独立 UID/GID 和固定 Home。UID/GID 从目标 root 的 passwd/group 读取，不能读取宿主 `id teams`，也不能硬编码旧 VM 的数值。账号需有可运行图形会话的正常 shell；通过密码/PAM/SSH 策略约束入口，不用 nologin 破坏 GDM 会话。

两个受管用户不得处于 sudo/admin 等管理组，锁定密码，禁止 linger、SSH 密码/密钥/证书、TTY、普通图形登录及不受控定时任务。单纯锁密码或隐藏 GDM 用户列表不够。新镜像中不留相应 `/var/lib/systemd/linger/` 文件；运行维护时按正常 loginctl 接口关闭并核对 `Linger=no`。清点 cron/at、system timer、`User=teams/waiting` 服务和其他可绕开 PAM 拉起 UID 的任务；存在活动任务时先按维护流程处置。

waiting Home 使用最小内容，只初始化清单规定的环境文件，不复制比赛 skel/扩展/自启动。teams Home 先创建真实目录并赋予实际 teams UID/GID；正式内容由 IMG-06 提供。Home reset 不删除/重建账号，不清理 waiting Home。

独立管理员必须有可用 SSH/TTY 维护路径；不能接管不相干的同名账号，不保留两个名字共用受管 UID。

## 4. IMG-02：GDM、桌面、停止顺序与 VT

先确认 inputs.md 中官方依赖与两个 xsessions entry 存在，再合并 `fragments/gdm/custom.conf` 的 daemon 配置：禁用 Wayland，AutomaticLogin=waiting，关闭 timed login。AccountsService 的两个 `[User] XSession=...` 片段合并进各自文件，保留其他合法字段，目标 root:root/0600。

每次 GDM 启动前，`rootfs/etc/systemd/system/gdm.service.d/20-waiting-autologin.conf` 使用官方 gdm-runtime-config 开启运行态自动登录；waiting 进入 PostLogin 时，`fragments/gdm/PostLogin.sh` 关闭它并 HUP GDM。此后重建 waiting/contest 由 Client 固定入口完成。只有静态 AutomaticLoginEnable=true 会让后续 display 重复自动登录 waiting，阻挡受控登录，不能省略这一组合。

把 PostLogin 逻辑合并到 GDM 实际选择的 Default/display/hostname 钩子中，保留已有逻辑；不能假定 Default 一定运行。目标 hook root:root/0755。ExecStartPre drop-in 追加到官方命令列表，不清空上游条目；保持所选发行版的真实工具和服务路径。不直接覆盖运行态 `/run/gdm3/custom.conf`。PostLogin 完成不等于桌面首帧就绪。

应用 session scope 的 `Before=display-manager.service` 停止顺序配置，使停止时 GDM 先于 scope 退出，避免 worker 的 VT 等待拖到旧 90 秒超时。该前缀也覆盖管理员 scope，须验证管理员退出和无依赖环；不缩短超时掩盖故障。

应用 logind 的 `NAutoVTs=0`、`ReserveVT=6`，防止自动 getty 抢占新图形 VT，保留 tty6 管理员维护。检查较晚 drop-in 和显式 getty 配置是否抵消它。通过维护期正常重启生效，不在活跃桌面中重启 logind。

保留每个用户自身的总线与 GDM Xauthority。greeter 可以使用 dbus-run-session；不要强制把 greeter 总线改为 `/run/user/<gdm uid>/bus`。不另起后台 gnome-session、keeper 或第二个显示管理器。

## 5. IMG-03：PAM、SSH、polkit

以下片段只合并到厂商文件，保留完整官方栈及其 conffile 管理，不使用 dpkg-divert、无条件 force-confnew 或永久冻结旧官方栈。前缀须在所有 sufficient 分支之前。先安装完整 Client，确保它的三个固定 PAM 文件存在，再接入镜像配置。

| 目标 | 合并动作 |
| --- | --- |
| `/etc/pam.d/gdm-autologin` | 前置 `fragments/pam/gdm-autologin`，即 Home admission |
| `/etc/pam.d/gdm-password` | 前置 `fragments/pam/gdm-password`，按给定顺序先普通入口拒绝，再 Home admission |
| 指纹及所有智能卡实现 | 前置 `fragments/pam/gdm-managed-entry`；`@MANAGED_GRAPHICAL_SERVICE@` 展开为各实际服务名 |
| `/etc/pam.d/other` | 前置 `fragments/pam/other`，覆盖缺失具名服务或缺失 phase 的 auth/account/session |
| 缺 account 段的 runuser/runuser-l | 加 `fragments/pam/runuser-account`，保留 auth/session；`@ROOT_TOOL_SERVICE@` 只展开到确实缺失 account 的两个服务 |

所测发行版有 `gdm-fingerprint`、`gdm-smartcard-pkcs11-exclusive`、`gdm-smartcard-sssd-exclusive`、`gdm-smartcard-sssd-or-password`；保留 `gdm-smartcard` alternatives 链，覆盖所有已安装和可选实现，不只改链接当前指向的一个文件。

安装 rootfs 中的 `natsume-managed-entry-deny` 与 `natsume-managed-sessions` profile，再通过发行版 pam-auth-update 启用 profile，例如 `pam-auth-update --enable natsume-managed-sessions`。检查生成的 common-account 跳转数和 allowlist；手工修改过的 common-account 需要按工具提示合并，不能强行覆盖其他策略。

| 入口 | waiting | teams |
| --- | --- | --- |
| 固定 gdm-waiting / gdm-contest | 仅自己的固定入口 | 仅自己的固定入口，且通过 Home 三阶段门禁 |
| gdm-autologin | GDM 启动初始化允许 | 不配置自动登录；固定入口包含该官方栈时仍受门禁 |
| 普通 gdm-password / 指纹 / 智能卡 | 拒绝 | 拒绝 |
| SSH 密码/密钥/证书、TTY、cron/at | 拒绝 | 拒绝 |
| systemd-user | 继续官方检查，允许正常 manager | 同左 |
| 真 root 调用 runuser/runuser-l/su/su-l | 允许维护，校验真实调用 UID 与 account 栈 | 同左 |
| 缺失服务/phase 落入 other | 拒绝 | 拒绝 |

不要给 gdm-contest、gdm-waiting、gdm-autologin、systemd-user 添加“普通入口拒绝”片段。common-account 不能代替 teams 的 auth/account/open_session Home 门禁；non-teams 和 close_session 保留固定门禁的直通语义。

SSH 合并 `DenyUsers teams waiting`，保留站点原规则，用实际 sshd 配置确认全部认证方法受限。polkit 规则限制受管用户的 login1/AccountsService/systemd1/udisks2 管理能力，包括重新启用 linger。管理员保持发行版认证流程，root 正例与普通用户负例分别验证。新版本 PAM 已提供 account 栈时重新合并；若旧 login 引用不存在的 pam_lastlog.so，按官方版本修正。

## 6. IMG-04：dconf、键盘、字体、缩放与睡眠

安装清单中的公共/专用数据库、locks、profiles 和按用户/服务划分的环境文件。waiting 使用 Kiosk 实际读取的 gnomekiosk profile，保留官方 compiled file-db；teams 使用 natsume_teams，并保留现有比赛系统数据库层。profile 与旧配置冲突时合并检查，不能机械覆盖掉比赛默认设置。

将 `@WAITING_UID@`、`@TEAMS_UID@` 替换为目标 root 的实际 UID，替换同时适用于目标路径。例如 `user@@TEAMS_UID@.service.d` 在 UID 1001 时为 `user@1001.service.d`。waiting 的 environment.d 文件由 waiting UID/GID 持有，模式 0644，目录由其本人持有；所有 systemd drop-in 为 root 所有。teams 的 profile 为 natsume_teams，并保留镜像所需数据库层。

在目标 root 执行 dconf update；维护后重建会话并读取实际进程环境和有效值。两个角色关闭自动锁屏、用户切换、空闲调暗/睡眠、锁屏快捷键。waiting 另设纯黑并禁用普通关闭、应用切换、运行对话框及交互退出快捷键。保留 `disable-log-out=false`，设为 true 会阻碍 GNOME 自身正常退出。

waiting/teams 共用 US 英文键盘默认值，不新增中文输入法或输入源切换快捷键。保留现有中文字体；若缺少中文字形，安装发行版的 CJK 字体包并验证实际显示。首次启动后在对应用户的正确总线/profile 下核对有效输入源。

不部署专用 IBus 配置。`GDK_SCALE=1` 只给 Kiosk compositor 及其子进程，不能把 frame-helper 的固定倍率传给 Agent/teams。

保留 sleep.conf 四项禁止：AllowSuspend、AllowHibernation、AllowSuspendThenHibernate、AllowHybridSleep 均为 no。临时 S3 放行只用于单独故障测试，交付前恢复。前台就绪要求真实 XInput2 slave keyboard/pointer 启用，并带 `/dev/input/eventN` Device Node；后台物理输入禁用正常，不用 XTEST 假设备或固定设备编号代替。

## 7. IMG-05：Xorg

安装清单中的 ServerFlags，DontVTSwitch/DontZap=true，Blank/Standby/Suspend/Off 时间为 0。清点并处理冲突的旧 ServerFlags，特别是后加载的 `90-natsume-test-kiosk.conf`，不能只放更早的文件。

在两个实际 Xorg 上确认屏保 timeout 与 DPMS 三项时间为 0；timeout=0 时 cycle=600 不表示会自动黑屏。实际 Ctrl+Alt+Fn、Ctrl+Alt+Backspace 不切出/终止会话，Helper/logind 的受控激活仍有效。这些约束不宣称构成完整恶意程序隔离。

## 8. IMG-06：每个最终安装源的 Home 模板

模板必须来自该安装源所有模块完成后的 `/etc/skel` 和受管 Browser/IDE 默认配置。icpc-contest-image 仅暴露 extra 累计安装源，因此在 extra 生成；若其他 builder 暴露多种安装源，每个源都须生成最终模板。不能从已用 Home 或 Live 层捕获。交付前确认没有 cookies、凭据、比赛文件、设备密钥及旧 `/home/contest` 专用引用。

下面是生成参数的完整参考，不是运行系统上的维护命令。变量全部由 builder 定义：`template_source` 是离线目标 root 的最终 skel（如需额外默认文件，先在独立暂存目录合成）；`template_output` 是新的空构建目录；UID/GID 来自目标 passwd/group；`template_epoch` 是已记录、非负的固定构建时间戳。

```sh
env -u SOURCE_DATE_EPOCH mksquashfs "$template_source" "$template_output/home.squashfs" \
  -noappend -comp zstd -processors 1 -no-progress \
  -force-uid "$teams_uid" -force-gid "$teams_gid" \
  -all-time "$template_epoch" -mkfs-time "$template_epoch"
```

使用所选官方 squashfs-tools 的受支持参数，固定版本/压缩参数并记录。`template_epoch` 可以取 builder 的 SOURCE_DATE_EPOCH 值，但执行命令时先移除同名环境变量，避免它与显式时间参数冲突。保留模板文件正常 owner 写位、目录和链接，使 OverlayFS copy-up 后能编辑；只读由挂载实现，不是 chmod 去掉 owner 写位。用 unsquashfs 检查实际镜像根及内容的 teams UID/GID。

最终布局固定如下：

```text
/usr/lib/natsume/home-templates/
  sha256-<home.squashfs 实际 SHA-256，64 位小写十六进制>/
    home.squashfs
    SHA256SUMS
    packages.tsv
  current/
    version
    lower/
```

SHA256SUMS 的一行是 `<digest>  home.squashfs`。packages.tsv 保存目标 root 的实际包名/版本，可在目标 chroot 用 `dpkg-query -W -f='${binary:Package}\t${Version}\n'` 生成并按固定 locale 排序。另在构建记录保存源安装层、skel 内容摘要、squashfs-tools 版本、UID/GID、构建时间和参数。current/version 写所选 `sha256-...` 目录名，最多一个结尾换行。

base/current/version 目录、image/metadata 均 root 所有，不是符号链接，不允许组/其他用户写；目录 0755、文件 0644。current/lower 是真实 mount point，运行时只允许该镜像在此形成唯一 read-only SquashFS loop mount，无 stacked/child mount。挂载后的 lower 根及 Home 归实际 teams UID/GID。

`@TEMPLATE_VERSION@` 替换为已校验目录名。通过以下命令取得 `@TEMPLATE_MOUNT_UNIT@`，用它同时渲染 mount 目标文件名和 Helper drop-in：

```sh
systemd-escape --path --suffix=mount /usr/lib/natsume/home-templates/current/lower
```

安装 templates 中的 mount 和 Helper Wants/After，离线 enable 该 mount。不加 Requires、不恢复全局 display-manager Home gate：模板挂载失败时 teams admission 关闭，Helper 诊断和 waiting 必须可用。构建/安装边界校验完整摘要；Helper 运行时核对真实版本/源/挂载/Home，不在每个 tick 重读完整镜像。

## 9. IMG-07：退出旧控制链

在启用 Natsume 的安装源中清点并停用这些已知入口及同类项，尤其检查后续模块是否重新写回。需要保留的旧配置仅置于管理员私有维护备份，不保持开机依赖或受管用户可触发入口。

| 类别 | 已知入口 |
| --- | --- |
| 自动写 GDM / 控制会话 | machine-setup-oobe.service、hydro-machine-tools.service、OOBE user autostart、machine-tools probe/NetworkManager dispatcher |
| 旧登录/锁屏工具 | `/usr/local/sbin/`、`/usr/local/share/icpc/scripts/` 下 login-user、logout-user、lock-user、unlock-user 及调用它们的菜单/快捷键 |
| 旧 Client/实验入口 | 全局 XDG org.natsume.SessionAgent.desktop、bootstrap/Home 服务、display-manager.service.d/50-natsume-home.conf（包括 /etc 的残留覆盖） |
| 重复 Agent/比赛自启动 | Kiosk 示例脚本编辑器、第二个 Agent user service/keeper、waiting 继承的比赛程序自启动 |

Client 包已删除自身旧全局入口，镜像仍须清理历史副本与站点覆盖。Natsume 未启用的安装源保留原镜像功能，不能通过全局删除破坏另一路交付。

## 10. IMG-08：安装、缓存、Live 与首次启动

镜像预装通过目标 root 的正常包管理安装完整 Client 并解析 Depends，不提供部署配置或 CA：

```sh
DEBIAN_FRONTEND=noninteractive apt-get install -y /path/to/natsume-client.deb
```

包初始化 sysusers/tmpfiles；部署文件缺失时允许安装，服务保持跳过启动。autoinstall 将部署方生成的完整 `/etc/natsume/config.toml` 和两份 CA 写入目标系统，路径、内容与权限遵循 [inputs.md](inputs.md#2-公共站点配置与身份)。配置只含 `[server]` 和 `[site]` 中的 Client 参数，不调用 debconf、端点配置命令或包脚本生成配置。文件缺失、内容非法或与配套 Server 不匹配时不能交付系统；已存在的非空可读检查只是包安装阶段的基本检查。

缓存键覆盖 Deb 实际字节/模式、交接目录和相关依赖层；部署配置和 CA 不进入构建参数、层键或发布 rootfs。自定义 CONFIG_DIR 的宿主读取、键计算、chroot 只读挂载须一致。

在离线目标 root 启用 natsume-privileged-helper.service、natsume-device-daemon.service 和模板 mount（使用 builder 的 `systemctl --root=... enable` 等价封装，不带 --now）。按发行版正常机制选择 GDM 为 display manager。Caddy 由 Daemon 管理；prepare instance 不独立 enable；Agent 随官方 Kiosk session 启动。

实际安装源保留上述服务与 waiting 自动登录。Live 层保留 Casper 临时管理员和可见安装器，关闭该层继承的 Client/模板启动及 waiting 自动登录：同时处理静态 GDM 配置与 ExecStartPre 运行态自动登录 drop-in，不能只改 AutomaticLoginEnable=false 后又被 ExecStartPre 重新打开。Live 专用覆盖不能写回安装源，也不能随安装过程复制到已安装系统。

构建禁止启动服务，也不复用主机 /run。镜像只含空的首次身份/状态目录；machine-id 和硬件身份边界见 inputs.md。第一台工位启动后产生的身份、Enrollment、Binding、Home epoch 或 waiting 恢复预算，以及队伍展示记录、Logo 缓存均不得回写模板或公共镜像。

## 11. 维护与回退

本期只支持当前数据库、Home 窗口格式和 waiting/teams 账号，不提供重构前数据库、窗口或账号的迁移路径。

Client 已改用程序内固定的硬件 ID 派生命名空间，identity.json 只保存 machine_hardware_id，Daemon 与 Helper 的 DeriveMachineIdentity 调用也不再带参数。使用旧站点 UUID 的 Client 不支持原地身份迁移：先完成 Home 恢复并退出受管会话，备份所需数据，在 Server 解除旧 Binding 并 Revoke 旧设备，再从干净镜像重新部署、审批注册和绑定。不要单独删除 identity.json、改写 ID 或沿用旧 Control/Gateway 凭据；旧身份记录会被拒绝，不能把它当作首次启动。Daemon 和 Helper 必须使用配套的新版本。

在用工位维护前保留独立管理员入口，让当前 Helper 完成已捕获的 Home 窗口，确认维护关闭后再停止服务和排空受管 UID。不得删除进度记录绕过恢复，也不在活跃会话或挂载下替换模板。

停止新的业务操作和 Daemon/control 连接，确认 Server offline/原 Actual 已撤销。用一致私有备份保存 Server 数据库与密钥、Client 身份/凭据、Binding、完成 epoch、Home 和镜像配置，不在 image builder 中改写业务数据库。

安装 Client 只更新随包输入副本，不会自动应用已安装系统配置。维护中按清单合并官方 PAM/GDM 栈，保留必要站点逻辑；再正常启动服务/GDM并复验。回退采用经过验证的完整 Server/Client/协议/模板/配置备份，不能单独降级 Helper 或混用状态。无完整冷备份和恢复验证时不能声称整套回退已通过。

完成后执行 [验收与交付记录](acceptance.md)。
