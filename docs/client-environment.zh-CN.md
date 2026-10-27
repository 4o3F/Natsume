# Client 运行环境要求

本页规定 Natsume Client 的运行条件。镜像项目负责镜像构建、发行版适配、安装器、配置合并和目标机器验收；本仓库提供 Client Deb，不提供镜像模板、适配脚本或检查器。

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
- 镜像完成受管用户隔离策略和旧状态处理后，最后写入 `/etc/natsume/contest-reset-policy`，内容精确为 `1\n`。文件及所有父目录必须归 root 所有、不可由非特权用户写入，且均不得是符号链接；该静态镜像契约与站点配置和 Home reset 进度独立。缺失、版本不符或无法确认可信时，Helper/PAM 保持比赛登录关闭，包括使用已有 permit 的情况。
- 系统设置 `RemoveIPC=yes` 和 `UserStopDelaySec=0`，两个受管 UID 均禁止 linger。Helper 仅在 logind `GetUser` 返回精确的 `NoSuchUser` 错误后执行扩展清理，以满足现有 8 秒操作预算。清理在关闭比赛登录、排空旧进程后的维护窗口内进行，保留 waiting 和 GDM。
- 除比赛 Home 外，reset 清理 `/tmp`、`/var/tmp`、`/run/lock` 和可选 `/var/crash` 内已确认属于 teams UID 的对象、固定范围内精确 teams `user_id` 的遗留 FUSE，以及 teams 的 user/user-session/persistent keyrings。遍历不跟随链接、不跨子挂载，保留共享目录和其他用户拥有的对象。无法安全遍历、teams 目录内含其他用户拥有的对象、待删除对象的上级目录可被其他用户修改或重命名、挂载忙碌或超时时，保持比赛登录关闭，等待维护处理。
- keyring worker 由固定 `/usr/bin/setpriv`（util-linux）直接执行 Helper，以 teams UID/GID、空补充组、无有效/许可/inheritable/ambient capabilities 和 `NoNewPrivileges` 运行。在同一私有 session keyring 中，安全 Rust keyutils API 取得所需权限，逐一核验密钥的实际属主并与内核记录的该 UID 密钥总数比较；所有旧密钥和 keyring（包括仅被限制规则引用的对象）失效后，还须在原预算内确认旧 serial 及描述已被内核回收，才允许完成清理。运行时无需系统 keyctl CLI，Helper 的 capability set 保持不变。权限不足或 keyring ACL 拒绝清理时，保持比赛登录关闭；恶意移除 WRITE/SETATTR/SEARCH 权限后可能需要维护或重启，不能仅凭 root 权限或新 keyring 为空判定旧内容已不可读取。
- 镜像必须限制受管用户修改 colord、NetworkManager 和 power-profile 的持久状态，保留只读功能和管理员操作；停用并 mask 本地 CUPS 的 service/socket/path/D-Bus 激活，保留 DOMjudge HTTP 打印；限制 privileged FUSE/utempter executables，并确保包升级不会恢复受限权限。journald 使用 `SplitMode=none`；迁移时撤销旧 per-UID journal 的受管用户 ACL，保护 utmp/wtmp/lastlog 的读权限及其重建、轮转行为，保留管理员和共享日志。
- 扩展清理的完成记录与 reset epoch 绑定，当前 `cleanup-complete` 格式为 `2\n<reset_epoch>\n`。旧版仅验证 Home 的 `Verified` 和仅证明 CLEAR 成功的 v1 记录不足以放行；升级已部署设备后须保留旧进度并显式提交新的 reset epoch，让 Helper 捕获和清理旧 contest。当前版本清理完成后，同 epoch 重放和健康 Helper 重启保留比赛桌面及之后写入的 Home、临时文件和 keyring 数据；重启后若挂载丢失，仅恢复原 generation，不重复清空。
- 通用镜像不得携带设备身份、私钥、Enrollment、Binding、Home 恢复记录或队伍展示缓存；首次身份在实际机器生成。升级已部署工位时则必须保留这些状态，不能按新镜像清空。

## 部署输入与服务所有权

部署方在首次业务启动前提供 `/etc/natsume/config.toml`、`/etc/natsume/trust/control-ca.crt` 和 `/etc/natsume/trust/local-origin-ca.crt`。文件为 root:root/0644，父目录为 root:root/0755；配置中的 Server IP、端口、Gateway 域名与证书必须匹配配套 Server。完整格式见随包 `config.example.toml` 和[部署运维手册](operations-deployment.zh-CN.md)。

Client 包不生成或改写这些部署输入，不创建 waiting/teams，不合并发行版桌面配置。包可提前预装，业务启动前须补齐部署输入。只启用 Helper 和 Daemon 的主机服务；Caddy 归 Daemon 管理，会话准备实例不单独启用，Agent 随 Kiosk 会话启动。

Gateway 的 hosts 映射、Firefox 主页和 `/etc/natsume/submit.env` 归 Helper 维护；镜像不得重复写入赛事域名。Firefox 必须实际读取 `/etc/firefox/policies/policies.json` 并信任部署 CA。若提供 DOMjudge `submit`，启动器每次调用读取 `submit.env`，仅为该进程设置 Local Origin CA，不在 Home 模板中保存队伍密码。

## 维护要求

Client、Server、协议、桌面配置与 Home 模板必须使用经过目标环境验证的兼容组合。维护前保留管理员入口、完成进行中的 Home 恢复、退出受管会话并停止控制连接；替换模板前排空相应用户与挂载。备份和恢复必须覆盖匹配的程序、配置、身份、凭据及业务状态，不能单独降级 Helper 或混用旧状态。

首次部署、升级和回退均须由镜像项目在实际目标环境验证登录隔离、双会话切换、Home 恢复及真实显示和键鼠可用性。
