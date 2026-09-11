# 验收与交付记录

本页是 image builder 的完整通过标准，不依赖外部 PRD 编号或旧 VM 日志。先执行构建检查，再在新镜像安装出的系统上验证；需要 Server 操作的项目使用 [inputs.md](inputs.md) 中的兼容验收环境。实施步骤见 [integration.md](integration.md)。

## 1. 构建与离线 root 检查

| 项目 | 通过标准 |
| --- | --- |
| 交接目录 | 从单独归档解压后运行 `python3 image/check.py` 成功，无原仓库/Git/目录外资料依赖 |
| Client 输入 | 可信 SHA-256、包名/架构正确；`check.py --deb` 通过且包内不含 site.toml 或 CA；缺少站点文件时仍可预装完整包及 Depends |
| 站点注入 | 安装 Client 后注入部署方的 site.toml 和两个 PEM CA；路径、root:root/0644、站点/证书匹配；缺失或不匹配使构建失败；Client 重装/移除/purge 保留这些文件 |
| 必需输入拒绝 | 启用时缺包、错误摘要/架构、半对/非法端点、目录与 Deb 不匹配，均使构建失败；禁用时不要求 Deb |
| 内容缓存 | 相同输入使用同一内容键；保持文件名不变但修改 Deb、站点配置或任一 CA 会失效；CONFIG_DIR 的读取/键/只读挂载一致 |
| 账号 | waiting/teams 独立 UID/GID/Home，与管理员不冲突；不在管理组；密码锁定、无 linger 文件或可重新拉起 UID 的任务 |
| Client 固定文件 | inputs.md 列出的程序/PAM/unit/IPC/config 存在，root 所有，执行位/配置模式正确；只有一个 Agent 启动所有者 |
| 全部 30 项配置 | 按 manifest.tsv 逐项记录实际路径、替换后的占位符、owner/group/mode、内容摘要；不能只记录“模块执行成功” |
| 官方栈 | GDM/PAM 合并保留官方完整栈、全部 smartcard alternatives、root 工具 account；无程序/资源补丁及不受控覆盖 |
| 最终模板 | 每档安装源最终 Browser/IDE/skel 进入模板；镜像根和内容是实际 teams UID/GID、正常 owner 写位；摘要/metadata/选择版本一致 |
| 启动依赖 | Helper Wants/After 正确指向唯一模板 mount；三项离线 enable；无 Requires/global GDM Home gate、第二个 Agent 或独立 prepare enable |
| Live/安装源 | Live 保留可见安装器和管理员、Client/template 不自动启动、waiting 不被 ExecStartPre 重新自动登录；安装源保留正式启动链 |
| 首次状态 | 未启动服务，没有克隆设备身份/私钥/Enrollment/Binding/Home epoch/恢复预算；机器身份按首次启动初始化 |
| 旧链清理 | OOBE/probe/登录锁屏工具/旧 Agent/global gate 及后续覆盖全部清点，提供逐项最终处置 |

从最终可交付 ISO/镜像完成一次真实安装，再检查安装后 root。只检查构建 chroot、压缩包文件列表或把 Deb 安装到旧 VM 均不满足本节。

## 2. 新镜像的桌面与入口检查

记录 waiting/teams 和管理员的实际身份，使用正常管理员通道收集 logind、journal、mount 与包事实。图形查询使用目标用户自己的实际 DISPLAY/XAUTHORITY/用户总线，不能用宿主显示环境、固定测试 session ID 或任意跨用户总线替代。

| 项目 | 实际检查 |
| --- | --- |
| 首次启动 | 无当前 Server Target/断开控制网络时，第一个稳定业务界面为 waiting；模板失败也保留 waiting/Helper 诊断，teams 门禁关闭 |
| 注册与业务闭环 | 新设备按真实首次硬件身份生成记录；Provisioning Gate/人工审批后连上 Server，在 waiting 真实输入 Binding，再访问配置的网关/测试上游 |
| 双会话 | 每角色仅一个 seat0/X11 会话，各自 Xorg/桌面/总线/Xauthority/Home；GDM 管理双方，后台比赛会话不判歧义 |
| 前台与输入 | Panel/Actual、logind Active/VT、实际画面一致；真实键盘/指针可用，前台 XInput2 实体 slave 有 Device Node，不能只看 ready=true |
| 待机与快捷键 | 连续 660 秒无输入，waiting 保持全屏黑色；Alt+F4、Super、Alt+Tab、运行/锁屏/退出入口、Ctrl+Alt+Fn/Backspace 不逃出普通受管路径 |
| dconf/环境 | 当前有效值及 locks 符合配置；waiting/teams 的 profile 正确且输入源仅 US；disable-log-out=false；无专用 IBus 环境覆盖；scale 环境仅影响指定进程 |
| 键盘/字体/缩放 | waiting 真实英文输入、Enter 提交、拒绝后重输、焦点和点击区域；teams 英文输入正常；中文文本显示完整、无缺字方框；至少 1280×800/120 DPI 与 1920×1080/192 DPI，再覆盖实际发行支持的模式 |
| Xorg 空闲 | 两个 Xorg 屏保 timeout 与 DPMS 三项时间为 0；受控前台切换仍可用 |
| 输入/显示故障 | DPMS、输出/分辨率变化、Xorg 暂停或真实输入不可用时撤销错误就绪；恢复后真实输入有效；waiting 故障只按有界预算重建自身 |
| PAM/入口 | 真实 auth/account/open_session 正负例；固定入口、普通密码/指纹/智能卡、SSH 密码/密钥/证书、TTY、cron/at、other 缺服务/phase、polkit/linger 全部覆盖；管理员 SSH/TTY/root 维护仍可用 |
| 模板/reset | 打开实际 Browser/IDE、修改默认文件并写 canary；reset 后默认恢复、canary 消失，waiting 数据/设备身份/凭据保留 |
| 维护 | GDM 正常重启、发行版 GDM/PAM 升级及 Client/配置交接后复查前台、门禁、实际画面/输入、配置持久性和无旧链回写 |

独立记录睡眠策略与睡眠恢复测试。交付状态保持四项 sleep 禁止；若临时放行 S3 测恢复，结束后还原并核对。既有 S3 成功只限官方 VGA/bochs QEMU 组合；virtio/QXL 和真实 GPU/驱动不能沿用它的结论，不要求用试验内核或软件光标替换正式镜像策略。

## 3. 同一最终基线的 26 项行为用例

以下均记录实际新镜像结果，状态使用“通过/失败/未执行/不适用及原因”。配置直接相关项由 image builder 验证；跨 Server、并发及精确持久化故障项目在同一配套验收环境中与 Natsume 交付方共同完成，不能把旧实现专项日志标成新镜像已通过。

| 编号 | 用例与通过标准 |
| --- | --- |
| AT-01 | 新镜像无当前 Target 启动：首个稳定业务界面 waiting，没有未经授权的业务访问 |
| AT-02 | 自动预备双方会话：两账户各一个原生 X11，会话独立且都归 GDM |
| AT-03 | waiting→contest→waiting 100 轮：双方 session/PID/Home generation、比赛文件和窗口保持，真实键鼠可用，无实例累积 |
| AT-04 | 普通切换只激活角色，不调用 GNOME Lock/Unlock；不能以 LockedHint 或后台状态代替实际前台事实 |
| AT-05 | waiting 目标下 reset：比赛精确身份和 Home generation 改变，waiting/GDM 不变，新比赛留后台 |
| AT-06 | contest 目标下 reset：Home 验证后新比赛桌面就绪并前台，旧会话和脏数据消失 |
| AT-07 | reset 中依次提交 contest、waiting：最终按最新 waiting 收敛，无旧计划持续抢回前台 |
| AT-08 | reset 后等待一段时间才显示比赛：双方保持存活，切回不重新登录或清 Home |
| AT-09 | 重复下发同一 Target/epoch：无多余 generation、登录事务或第三个受管会话 |
| AT-10 | 固定 API 登录入口退出/崩溃：已由 GDM 创建的比赛桌面存活，重试先观察实际结果 |
| AT-11 | waiting Binding：实际提交可用，后台/失联/伪造身份和旧连接不能提交 |
| AT-12 | waiting 全屏首帧、真实输入：普通关闭或 IPC/lease 失联不移除占位，不暴露桌面，Binding 资格按实际状态撤销 |
| AT-13 | 并发登录与 reset：所有允许的 teams 图形入口均受三阶段互锁，登录事务不跨入 Home 修改区间；普通密码入口按策略拒绝 |
| AT-14 | 在 PAM 检查后、logind 注册前暂停真实 worker：reset 不能误判无会话而修改 Home；取消/排空后才继续 |
| AT-15 | 比赛 UID 残留、user manager、外部 cwd 占用：有界清理或明确拒绝，无 force/lazy umount，waiting 保持 |
| AT-16 | 缺模板、错误版本/源/所有权、非只读、磁盘满或挂载失败：无新登录许可和虚假完成，修复后原 epoch 恢复 |
| AT-17 | Helper 在持久窗口写入、Prepared、Applied、Verified 四个精确点分别退出：重启后同 epoch 恢复，真实挂载验证前不放行 |
| AT-18 | 上述四个点分别 VM 硬复位和断电：新 boot waiting 可用，teams 许可在恢复前关闭，旧 boot 身份失效；区分两种故障证据 |
| AT-19 | 正常重启 Daemon/Helper：健康双方桌面/Home 保持，新观测后恢复业务资格 |
| AT-20 | reset 期间断网/control lease 替换：本地已捕获责任可安全完成，旧 lease 不再发起比赛呈现，新 Target 决定前台 |
| AT-21 | 激活/登录调用超时后结果晚到：观察未知结果，不重复破坏性操作，旧操作不能永久覆盖当前目标 |
| AT-22 | 额外比赛候选或伪造 Agent：歧义/拒绝，不误操作 waiting/其他用户，不错误提交 Binding |
| AT-23 | waiting 程序/会话故障：报告展示不可用，仅按每 boot 有界预算恢复 waiting，不停止 GDM/teams，不重置预算 |
| AT-24 | GDM 重启和官方包升级：默认回 waiting，重新核对身份/API/PAM/Home；无实验资源、旧配置写入或全局 Home gate |
| AT-25 | 私有 mount namespace、损坏 marker、伪造 Verified：无法产生假成功/登录许可，实际宿主 Home 恢复后才通过 |
| AT-26 | reset 前上游 BLOCKED，完成后新观察才恢复；设备身份、Binding、凭据、waiting Home 保留 |

AT-17/18 的注入点须在记录文件 fsync、rename、父目录 fsync 完成之后明确定位。应有四次 Helper 退出、四次硬复位、四次断电记录；随机 kill 或人为写入许可/完成标记不能代替。仅在专用验收工位/快照中注入，不改交付包的正式行为。

## 4. 次数、耗时与证据口径

| 项目 | 口径 |
| --- | --- |
| 普通切换 | 双桌面已就绪，从 Client 接受有效 Target 到目标实际可交互画面，100 轮 P95 ≤3 秒，排除网络传输；保留每次方向/原始耗时 |
| 比赛冷登录 | 从允许发起 GDM 登录到真实 GNOME ready，参考 QEMU 目标 ≤30 秒，独立统计 |
| Home reset | 最终模板、无外部占用，20 个独立 epoch 各 ≤120 秒；不把等待管理员另行提交 contest 目标计入；记录模板/文件数/磁盘 |
| 启动 | 同一正式基线 10 次冷启动/重启，自动恢复 Home 与 waiting，不依赖临时脚本、手工环境或一次性许可 |
| 超时/失败 | 单次本地 D-Bus 默认 10 秒，较长操作按进度继续；超时表示结果未知，不表示取消；实际失败可见，无提前上报成功 |

未达到目标时记录实际值和原因，不通过提前 ready、跳过首帧/Home 验证或强制卸载“达标”。使用 QEMU 可完成本期验收；物理 GPU 兼容性只按真实已测硬件另行声明。旧 Home 初始显示故障并未全部归因，不能把本包配置描述为已经证明消除所有驱动故障。

## 5. image builder 应交付的材料

交付一份本次结果文档和对应文件，至少包含：

1. 镜像/ISO 下载位置与 SHA-256，builder revision、实际安装源、构建方式、包来源和时间基准；Client/Server 版本、Deb 摘要、交接归档摘要。
2. 实际系统包清单（kernel、GDM/libgdm、GNOME/Kiosk、Xorg/输入驱动、PAM、中文字体/squashfs-tools 等），30 项清单的最终路径/owner/group/mode/内容摘要，以及所有厂商合并差异。
3. waiting/teams UID/GID/Home、管理员保留规则；各安装源 skel 来源和内容摘要、模板版本/SHA-256、包清单、mount 与 Helper 依赖。
4. 旧入口逐项清理、Live/安装源差异、服务 enable、首次身份初始化，以及实际升级/回退维护步骤和恢复验证结果。
5. 上述构建/桌面/AT-01～26 逐行结果；正式 100/20/10 原始计数与耗时；失败、重试、未测范围和原因。
6. 每轮关键证据：镜像/包/模板版本，Target/epoch，boot ID、双方 session ID、Xorg/Shell/Agent PID、GDM InvocationID、Home generation/验证状态、实际前台/输入、耗时及错误。正常显示与重建留截图/视频，日志保留失败。

证据不包含私钥、身份/控制凭据正文、比赛文件、cookies 或管理员密码。先在本地按必要字段核对，再输出摘要和受控日志。交接包完整只证明实施资料已齐备；实际镜像与该结果文档完成后，才可签收发行。
