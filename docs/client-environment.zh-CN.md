# Client 运行环境要求

本页只规定 Natsume Client 正常运行所需的系统条件。镜像构建、发行版适配、安装器、配置合并和目标机器验收由镜像项目负责，本仓库不提供镜像模板、适配脚本或检查器。安装 Client Deb 不等于完成桌面环境配置。

## 系统与桌面

- 当前部署基线是 Ubuntu 24.04 amd64、systemd 253+、官方 GDM、GNOME Shell 和 GNOME Kiosk，使用原生 Wayland；运行库以 Deb 的 `Depends` 为准。
- GDM 必须能管理两个独立原生会话：waiting 使用 `gnome-kiosk-script-wayland`，比赛桌面使用 `ubuntu-wayland`。每个会话具有自身的 compositor、用户总线和 Wayland socket。
- 官方 Kiosk Script 用户服务是 Session Agent 的唯一启动和重启所有者。Client 已提供对应 drop-in；不能另起第二个 Agent、keeper 或全局自启动入口。
- 必须具有可用的显示输出、键盘和指针，保留中文字体与 US 英文键盘。普通前台切换保留比赛会话和应用；不能用锁屏、注销或重启桌面代替切换。
- 禁止受管用户自动锁屏、挂起及绕过受控前台切换。GDM 必须先于 session scope 完成关闭，避免 worker 与 VT 退出互相等待；不能让自动 getty 抢占图形会话使用的 VT。

## 账号与登录边界

| 用途 | Unix 用户 | Home | 固定 PAM 入口 |
| --- | --- | --- | --- |
| 等待界面 | `waiting` | `/home/waiting` | `gdm-waiting` |
| 比赛桌面 | `teams` | `/home/teams` | `gdm-contest` |

`contest` 是协议和 Helper CLI 的业务角色名，不是 Unix 用户名。两个用户必须具有独立 UID/GID，不属于管理组；保留独立管理员维护入口。

受管用户只能通过固定入口进入会话。普通图形登录、SSH、TTY、定时任务及 PAM fallback 不能绕过限制；仍须保留正常的 systemd 用户管理和真正 root 的维护能力。Client 提供固定 PAM 服务和 Home admission，发行版 PAM/GDM/SSH/polkit 的接入归镜像项目，不能用删除官方检查或放宽系统总线权限代替适配。

GDM 首次启动进入 waiting，后续 waiting/contest 的准备与重建由 Client 控制；自动登录不能反复抢占后续登录。两个受管会话应能同时存在，后台比赛会话是正常状态。

## Home 与状态

- 系统须支持 OverlayFS、SquashFS 和 loop。正式比赛 Home 模板来自所用镜像的最终默认配置，不得来自已使用的 Home。
- 模板路径为 `/usr/lib/natsume/home-templates/sha256-<64 位小写 SHA-256>/`，包含 `home.squashfs`、`SHA256SUMS` 和 `packages.tsv`；`current/version` 保存所选目录名。
- `current/lower` 必须是所选镜像唯一的只读 SquashFS 挂载点。模板、元数据及父目录由 root 所有且不可由非特权用户修改；挂载后的 Home 内容归实际 teams UID/GID。
- Helper 必须运行在宿主 mount namespace。模板不可用时禁止比赛登录，但必须保留 waiting 和管理员诊断能力，不能让整个 GDM 依赖 Home 恢复成功。
- Home reset 归 Natsume 管理；镜像或登录脚本不得并行重建 Home、删除恢复进度或绕过 admission。普通会话切换不重置 Home。
- 通用镜像不得携带设备身份、私钥、Enrollment、Binding、Home 恢复记录或队伍展示缓存；首次身份在实际机器生成。升级已部署工位时则必须保留这些状态，不能按新镜像清空。

## 部署输入与服务所有权

部署方在首次业务启动前提供 `/etc/natsume/config.toml`、`/etc/natsume/trust/control-ca.crt` 和 `/etc/natsume/trust/local-origin-ca.crt`。文件为 root:root/0644，父目录为 root:root/0755；配置中的 Server IP、端口、Gateway 域名与证书必须匹配配套 Server。完整格式见随包 `config.example.toml` 和[部署运维手册](operations-deployment.zh-CN.md)。

Client 包不生成或改写这些部署输入，不创建 waiting/teams，不合并发行版桌面配置。未提供部署输入时可预装包，但不能据此认定工位可用。只启用 Helper 和 Daemon 的主机服务；Caddy 归 Daemon 管理，会话准备实例不单独启用，Agent 随 Kiosk 会话启动。

Gateway 的 hosts 映射、Firefox 主页和 `/etc/natsume/submit.env` 归 Helper 维护；镜像不得重复写入赛事域名。Firefox 必须实际读取 `/etc/firefox/policies/policies.json` 并信任部署 CA。若提供 DOMjudge `submit`，启动器每次调用读取 `submit.env`，仅为该进程设置 Local Origin CA，不在 Home 模板中保存队伍密码。

## 维护要求

Client、Server、协议、桌面配置与 Home 模板必须使用经过目标环境验证的兼容组合。维护前保留管理员入口、完成进行中的 Home 恢复、退出受管会话并停止控制连接；替换模板前排空相应用户与挂载。备份和恢复必须覆盖匹配的程序、配置、身份、凭据及业务状态，不能单独降级 Helper 或混用旧状态。

首次部署、升级和回退均须由镜像项目在实际目标环境验证登录隔离、双会话切换、Home 恢复及真实显示和键鼠可用性。构建成功、包安装成功或协议 `ready=true` 都不能替代这项验证。
