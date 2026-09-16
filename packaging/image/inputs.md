# 构建输入与 Client 契约

本页列完接收方需要落实的外部输入。配置和设计说明已经包含在交接目录中；不要求取得 Natsume 源码或旧试验附件。回到 [交接入口](README.md)。

## 1. 部署方必须提供

| 输入 | 约束和用途 | 缺失或不匹配时 |
| --- | --- | --- |
| 完整 `natsume-client` Deb 及可信 SHA-256 | 目标架构匹配；支持 waiting/contest 原生双会话、teams 账号及本交接目录；从部署方指定的发行产物来源取得，不猜测下载 URL | 停止镜像构建，不只复制几个二进制 |
| 匹配的 Natsume Server/协议版本 | 用于首次 Enrollment、Binding 和验收，由同一发行交付方确认兼容组合 | 可以做离线构建检查，不能签收业务闭环 |
| `[server].ip` | 由 autoinstall 在目标系统中提供；工位可访问的 IPv4/IPv6 字面量；Server TLS 证书包含该 IP SAN，不能填写主机名 | 运行时拒绝缺失或非法端点 |
| `[server].port` | 由 autoinstall 在目标系统中提供；合法 TCP 端口；HTTPS 与设备 WSS 使用同一端口 | 不能静默选用测试端点 |
| 完整 Client 配置和两个 CA 证书 | 部署方独立生成，由 autoinstall 安装到下文固定路径；不包含在通用 Client Deb 中 | 安装系统首次启动前必须完整匹配，不生成测试 CA 回退 |
| 镜像发行版、目标架构、安装源、构建时间基准 | 用于官方依赖解析、每档 skel、模板和可复现输入 | 不把一档模板当作所有安装源的最终模板 |
| 独立管理员及账号保留规则 | 管理员不得命名 waiting/teams，UID 不与受管账号共用；凭据遵循镜像项目现有私密输入机制 | 名称/UID 冲突必须停止，不能接管不相干的旧账号 |

外部 Deb 是本目录的运行依赖，不能嵌入本目录后又随同一个 Deb 打包。交接目录可独立压缩分发；镜像项目按自己的依赖管理方式保存/获取 Deb，记录其真实内容摘要。所需官方系统包同样从该项目的官方包仓库或离线包池解析。

本地候选 `2.0.0~r5.14` 已包含 teams 账号改动，但不是最终发行承诺，且它早于本次完整交接文档。仅按版本号或文件名放行不够：安装前运行 `python3 image/check.py --deb CLIENT_DEB`，要求实际包内副本与交接目录匹配；再验证发行方确认的程序/协议版本。

## 2. 公共站点配置与身份

通用镜像只预装 Client；autoinstall 在目标系统中安装部署方生成的以下完整公共文件。镜像项目不生成 CA，也不从 CI 包提取测试信任关系；证书使用 PEM，三个文件均为 root:root/0644，父目录为 root:root/0755：

| 安装目标 | 内容 |
| --- | --- |
| `/etc/natsume/config.toml` | `[server]` 下 `ip`、`port`；`[site]` 下 `gateway_hostname` |
| `/etc/natsume/trust/control-ca.crt` | 控制平面公共 CA 证书 |
| `/etc/natsume/trust/local-origin-ca.crt` | 本地 Origin 公共 CA 证书 |

这些文件归部署方所有，Client 安装、重装、重新配置、移除和 purge 均不写入、改权限或删除它们。完整配置示例随包放在 `/usr/share/doc/natsume-client/config.example.toml`，不自动安装到 `/etc`：

```toml
[server]
ip = "192.0.2.10"
port = 8443

[site]
gateway_hostname = "gateway.contest.example"
```

部署生成器须替换示例值，核对与配套 Server 的 Gateway 域名和 CA 一致，并检查文件权限。Client 不读取 Server 的证书期限、比赛结束时间或第二份站点配置；运行时直接从两份 PEM 建立各自的信任关系。这里的部署文件不进入通用镜像或构建缓存键，内容变化时重新生成安装部署输入。

包安装脚本只初始化服务账号/目录并检查已提供文件是否为非空可读文件。尚未提供时报告并允许预装；Daemon unit 对三条路径均有 `ConditionPathExists`。这些存在性检查不替代启动时配置解析和 TLS 验证，也不替代部署验收。没有 debconf 端点输入或延后配置开关；autoinstall 直接落地完整配置，不调用配置生成命令。

Gateway 域名由 Natsume 运行时管理：Daemon 启动时从 `config.toml` 解析 Gateway 域名并传给 root Helper，由 Helper 校验域名后同步 `/etc/hosts` 的 loopback 映射和 `/etc/firefox/policies/policies.json` 的主页、`Contest Site` 工具栏书签及已有的相关通知许可。修改配置后重启 Daemon，并完全退出再打开 Firefox；不需要镜像或 autoinstall 再复制一份域名到 hosts/浏览器配置，不增加配置命令。改名会清理旧受管映射；无关 hosts 别名、Firefox 策略、书签和 CA 设置保留。同步不依赖 Server 在线、Enrollment 或 teams Home。

镜像提供 Firefox 基础策略及 CA 信任，使用以上系统策略路径；配置和策略路径应为 root 所有的普通文件/真实目录，仅允许 root 用户/组写入，不能使用符号链接，建议文件 0644、目录 0755。缺失的 Firefox 策略文件/目录由 Helper 创建。不要在登录脚本或 Home 模板中回写旧主页。若换用 Snap 等 Firefox 打包形式，须验证该路径被实际加载，不能只验证 JSON 文件存在。

同一次 Gateway 同步还会生成 root:root/0644 的 `/etc/natsume/submit.env`，内容为 `SUBMITBASEURL='https://<gateway_hostname>/'`。它仅包含公开地址，由 Helper 独占维护，不作为 autoinstall 输入或镜像预置文件。镜像提供固定版本的 DOMjudge `submit` 脚本及薄启动器：每次调用读取该文件，以系统 Python 和 `python3-requests` 执行上游脚本，仅为该进程设置 `REQUESTS_CA_BUNDLE=/etc/natsume/trust/local-origin-ca.crt`，并将 Gateway 域名加入代理绕过列表。不能依赖登录 shell 继承的旧地址，不能将队伍密码写入 `.netrc` 或 Home 模板；Caddy 在 READY 时为 `/api/*` 注入当前 Binding 的 Basic 认证，BLOCKED 时统一拒绝访问。

Server root key、CA 私钥、每设备控制/网关私钥不进入交接包或可克隆镜像。首次启动前，`/var/lib/natsume/{identity,control,keys,state}` 与 `/var/lib/natsume-privileged/home-reset` 不得携带运行状态；允许包初始化空目录。不要把已运行工位清空后当作可信新镜像来源。已部署工位的升级必须保留这些状态，不能套用新镜像初始化清理。

按发行版机制准备首次生成的 machine-id。Natsume 自身首次身份还依赖真实机器硬件证据，使用程序内固定的 UUIDv5 命名空间，部署方无需生成站点 UUID。同样的硬件证据在不同部署中得到相同 Hardware ID。QEMU 克隆须配置独立、有效的 SMBIOS/系统及主板标识，不能让多台工位共享同一硬件身份。身份就绪并连上 Server 后，Provisioning window 为 Open 时自动批准 Enrollment，Closed 时等待管理员审批，再由 waiting 的 Agent 进行 Binding；包非交互安装不等于注册或业务授权。

## 3. 官方依赖与实际文件

参考基线为 Ubuntu Noble 系列、GDM 46.2、GNOME Kiosk 46.0、原生 Wayland。版本是已测试的参考，不要求永久冻结官方安全更新；更换版本应检查下列路径/API 和完整验收。当前目标架构为 amd64；其他架构需要匹配 Deb 和对应验证。

| 依赖 | 官方包/必须存在的能力 |
| --- | --- |
| GDM | `gdm3`、`libgdm1`；`gdm.service`/`display-manager.service`、`/usr/libexec/gdm-runtime-config`、官方 GDM D-Bus 登录 API 和 worker |
| waiting | `gnome-kiosk`、`gnome-kiosk-script-session`；`/usr/share/wayland-sessions/gnome-kiosk-script-wayland.desktop`、`org.gnome.Kiosk.Script.service`、`org.gnome.Kiosk@wayland.service` |
| teams 桌面 | 官方 Ubuntu/GNOME session、GNOME Shell；`/usr/share/wayland-sessions/ubuntu-wayland.desktop`，由所选发行版的 Ubuntu session 与完整桌面依赖提供 |
| 图形与输入 | 官方 Mutter、对应 kernel/DRM/libinput 驱动；原生 Wayland 输出与输入，按需保留 Xwayland 应用兼容层 |
| 键盘与字体 | 仅要求 US 英文键盘，不额外安装输入法引擎；保留可正常显示中文的字体，例如发行版 `fonts-noto-cjk` 提供的 Noto Sans CJK |
| 配置/权限 | dconf 工具与编译数据库、polkit、发行版 `pam-auth-update`、`libpam-modules` 的 pam_exec/pam_succeed_if、OpenSSH（若提供远程维护） |
| Home | 内核 OverlayFS、SquashFS、loop，官方 `squashfs-tools`、`util-linux`；systemd 253+，支持包内 OpenFile/namespace 配置 |
| Client 运行库 | 以 Deb `Depends` 为准，当前包括 ca-certificates、dbus、libfontconfig1、libfreetype6、libstdc++6、libsystemd0、libudev1、systemd、util-linux、libpam-modules |
| 构建检查 | Python 3.10+、sh、dpkg-deb、sha256sum；模板生成需要 mksquashfs、dpkg-query、systemd-escape |

保留发行版正常内核和显示驱动选择；不修改 GNOME/GDM 程序、资源或 greeter，不引入 nested Xorg/Wayland。角色需访问自身 compositor 的 Wayland socket、用户总线和进程信息，不新增跨用户读取权限。Xwayland 由 GNOME 按需管理，仅用于兼容 X11 应用，不能作为物理输出或会话身份的依据。CPU 验证使用软件渲染；交付按目标驱动验证，不把 QEMU 试验参数直接写成全局生产策略。

## 4. Client 包直接提供的运行文件

下列文件由完整 Client 包独占。image builder 检查它们存在并保留其包版本，不复制第二份、不自制替代启动器：

| 安装目标 | 契约 |
| --- | --- |
| `/usr/bin/natsume-device-daemon` | `run` 常驻；独占与 Server 的通信和 Caddy 配置 |
| `/usr/lib/natsume/natsume-privileged-helper` | root 服务；受限 Gateway 本机配置、GDM/logind/Home 能力，必须处于宿主 mount namespace |
| `/usr/bin/natsume-session-agent` | `run`，waiting 队伍／学校／Logo／离线状态与 Binding 窗口，由官方 Kiosk Script service 管理 |
| `/usr/lib/natsume/caddy` | 包内已校验版本，由 Daemon 管理；镜像不额外启动第二个网关 |
| `/usr/lib/systemd/system/natsume-{device-daemon,privileged-helper,caddy}.service` | 主机服务；只离线 enable 前两者，Caddy 的启停归 Daemon |
| `/usr/lib/systemd/system/natsume-session-prepare@.service` | 固定 root 登录入口，实例名只用 waiting/contest；不单独 enable |
| `/usr/lib/systemd/user/org.gnome.Kiosk.Script.service.d/50-natsume.conf` | `ConditionUser=waiting`，ExecStart 为 Agent run；它是 Agent 唯一启动/重启所有者 |
| `/etc/pam.d/gdm-contest` | 只允许 Unix teams，包含 Home admission 与官方 gdm-autologin |
| `/etc/pam.d/gdm-waiting` | 只允许 Unix waiting，包含官方 gdm-autologin |
| `/etc/pam.d/natsume-contest-admission` | auth/account/session 使用 pam_exec 调用 Helper pam-gate；teams 在 auth/account/open_session 受互锁，close_session 不阻塞 |
| `/usr/share/dbus-1/system.d/org.natsume.{Device1,Privileged1}.conf` | 固定系统 IPC 权限；不放宽为所有本地用户均可调用 |
| `/usr/lib/sysusers.d/natsume.conf`、`/usr/lib/tmpfiles.d/natsume.conf` | 服务账号和状态/运行目录；不创建 waiting/teams 桌面账号 |
| `/usr/share/natsume/image-integration/` | 本交接目录原样副本；由镜像构建继续应用 |

Client 安装脚本执行 sysusers/tmpfiles；检查部署文件是否已提供、适用时 daemon-reload；配置由部署方生成，包从不改写；不创建桌面用户、合并官方 PAM/GDM、生成 Home 模板或 enable/start 桌面。image builder 必须实现 [实施要求](integration.md) 中的剩余步骤。
