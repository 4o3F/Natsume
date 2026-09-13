# PRD：原生 GNOME 双会话与受控 Home 重置

| 项目 | 内容 |
| --- | --- |
| 文档版本 | 1.4 |
| 日期 | 2026-09-10 |
| 状态 | 已接受的产品要求；固定比赛账号为 teams，Client 持续安装；不代表发行验收通过 |
| 适用系统 | Natsume Client、Operator Panel、ICPC Contest Image |
| 基线 | 当前 ICPC 镜像，官方发布的 GDM/GNOME，原生 X11 |
| 功能目标 | 两个独立会话常驻；平常只切换；Home reset 时仅重建比赛会话 |

## 1. 产品结论

工作站运行两个不同 Unix 用户的原生 GNOME 会话：`waiting` 承载等待界面，`teams` 承载比赛桌面。业务角色仍命名 waiting/contest，固定 CLI/PAM 标识与协议目标不随 Unix 账号改名，比赛 Home 为 `/home/teams`。两个会话均由 GDM 创建和管理，各自拥有 Xorg、图形运行环境、用户总线和 Home。waiting 使用官方 GNOME Kiosk，contest 使用完整 GNOME Shell。业务操作命名为“显示等待界面”和“显示比赛桌面”，分别将 `foreground_target` 设为 waiting 和 contest；正常操作只改变前台，不调用 GNOME 锁屏/解锁，不结束比赛应用。

waiting 首期默认显示全屏纯黑色块，也可由镜像随包放置单张静态 ICPC logo，图片不可用时回退纯黑。占位复用现有 Session Agent 的 Slint/Skia 能力；复杂动态等待页面属于后续内容演进，不是本期交付条件。既有 Binding 界面作为部署时的专用页面保留。

Home reset 是一个完整业务操作：切到 waiting，关闭 contest 登录入口，结束比赛会话并清理残留进程，重置比赛 Home，再请求 GDM 登录 contest。比赛桌面就绪后，按最新有效的前台目标选择继续等待或返回比赛。

重建期间允许切到 greeter、黑屏、闪屏或短暂显示新比赛桌面；不要求 waiting 始终占据前台，但正常重建不得结束或重建 waiting。Natsume 通过受限本地 API 操作 GDM、logind 和 Home 维护能力；GDM 负责桌面的启动与生命周期。

## 2. 已确认约束与本期决定

### 2.1 用户已确认的约束

| 编号 | 约束 |
| --- | --- |
| C-01 | 正常状态同时存在 waiting 和 contest；普通切换不得通过登出、重新登录实现 |
| C-02 | Home reset 必须先转入 waiting，再结束 contest、清理 Home、重新登录 |
| C-03 | waiting 与 contest 必须是独立原生会话；禁止嵌套桌面、共享父 compositor 或 Xephyr 承载 contest |
| C-04 | 不修改 GDM、GNOME Shell、Mutter 的源码、程序、JavaScript 资源；只使用官方发布版本及发行版正式软件包 |
| C-05 | 优先 X11；本期不依赖 Wayland、不要求升级图形栈 |
| C-06 | GDM/GNOME 行为由镜像配置；Natsume 通过 API 间接操作，不成为比赛桌面进程监督者 |
| C-07 | 镜像可以修改和重新构建，实验 ISO 不是最终交付物 |
| C-08 | Home 重建期间允许前台切换和闪屏；正常等待状态仍应稳定显示全屏 waiting |
| C-09 | 业务操作改名为“显示等待界面 / 显示比赛桌面”，目标字段为 foreground_target，不再沿用 lock/unlock 业务命名 |
| C-10 | 本期 waiting 为纯黑或单张 ICPC logo 占位，复杂 Skia 等待页面延后 |
| C-11 | 比赛 Unix 用户名为 teams，Home 为 /home/teams；业务角色仍为 contest |
| C-12 | Client 安装后持续保留，不以卸载场景作为本次设计或验收门槛 |

### 2.2 本 PRD 选定的实现语义

以下是为完成闭环而选定的产品规则，不代表当前代码已经实现：

1. 本期只支持一个物理 seat：`seat0`，以及固定的 `waiting`、`teams` 两个账户。单显示器是验收基线，不提供多 seat 调度。
2. `foreground_target=waiting` 收敛为“waiting 占位已就绪且位于前台”，`foreground_target=contest` 收敛为“contest 桌面已就绪且位于前台”。GNOME LockedHint 不代表该业务目标。
3. waiting 普通等待、重置与恢复期间使用同一静态占位；详细进度和错误在 Panel/Actual 展示。本期不增加等待动画、状态文案页面或远程素材协议；既有 Binding UI 仍复用原协议。
4. 未绑定设备停留 waiting 完成现有绑定流程；Server 为新设备初始化 waiting 目标，绑定成功不改写目标，默认继续等待管理员选择 contest。已有持久目标在绑定、重连或重启时保留；重启先展示 waiting，收到当前有效目标且依赖就绪后恢复 Server 保存的前台选择，不另建业务状态。
5. Home reset 不隐式改写 `foreground_target`，不新增 `return_mode`。需要重建后继续等待时，管理员先成功提交 waiting 目标，再提交 reset；需要立即返回时保持 contest 目标。
6. 开机没有当前有效 Server Target 时，默认显示 waiting。镜像的固定启动流程可完成本机 Home 恢复及比赛会话预备，预备完成后回到 waiting，不额外锁定 contest；不得凭旧 lease 或磁盘缓存自动放行比赛。这项本地启动行为不产生新的远程 Target。
7. 显式管理员 terminate 和会话崩溃属于常态切换之外的维护/故障场景。不得为了普通前台切换使用 terminate。

命名统一如下；旧名称只用于说明迁移和历史证据，不作为并行接口保留：

| 原命名 | 当前产品操作 | 前台目标 |
| --- | --- | --- |
| lock / locked | 显示等待界面 | `foreground_target=waiting` |
| unlock / unlocked | 显示比赛桌面 | `foreground_target=contest` |

Target 仅允许两个受管角色；Actual 的 `foreground` 继续描述实际前台，可包含 greeter/other/none/unknown。操作更新持续目标，不新增两条 Command 或 toggle API。

### 2.3 非目标

- 重建全程零闪屏、固定 VT 或 waiting 永久占据 DRM master。
- 任意用户、任意桌面、任意 seat、LightDM/Wayland 的通用适配框架。
- Natsume 直接执行 `Xorg`、`gnome-session`、`gdm-session-worker` 或手动建立比赛 PAM/logind 会话。
- 将全屏 UI、GNOME 锁屏或会话切换当作对抗恶意本地用户的完整安全隔离。
- 重建过渡阶段“任何一帧都不能显示比赛桌面”或“任何瞬间都不可能收到输入”的保证。当前可行性试验没有证明这两项；若新增此要求，必须重新验证，不能由“允许闪屏”推导为已满足。
- 业务审计账本、新操作队列、另一份 durable permission 状态、远程通用 shell 或新的凭据体系。
- 本期等待进度页面、动画、主题系统、页面插件、远程图片上传/下载及通用 Skia 页面协议。

## 3. 背景与适用边界

本功能将会话控制统一为两个独立原生会话的前台选择，并使 Home reset 只重建比赛会话。组件所有权和安全边界以[主架构](architecture.md)为准；本文规定产品行为、交付范围与验收条件。

前期原型和阶段 VM 的结果只适用于各自被测版本。正式发行必须按 §11～12 验证匹配的 Client、镜像、模板和配置；旧账号、旧锁屏语义或本地候选的通过结果不能直接算作 teams 新镜像验收。

## 4. 用户、场景与可见结果

| 用户/场景 | 操作 | 可见结果 |
| --- | --- | --- |
| 现场志愿者开机 | 启动工作站 | 首个稳定业务界面是 waiting；需要绑定时显示既有席位输入 |
| 管理员开始比赛 | 显示比赛桌面 | 切到已有 contest，比赛应用继续运行 |
| 管理员暂停使用 | 显示等待界面 | 显示 waiting 静态占位；contest 留在后台并保留进程、文件和窗口 |
| 管理员清理工位 | 提交 Home reset | 旧比赛应用退出，Home 恢复模板，GDM 创建新比赛会话 |
| 清理后等待放行 | reset 时前台目标为 waiting | 新 contest 已准备并留在后台，等待后续“显示比赛桌面”目标 |
| 清理后立即继续 | reset 完成时前台目标为 contest | 确认新桌面就绪后直接返回比赛 |
| 管理员排障 | 查看设备详情 | 区分 Home 完成、桌面启动、等待放行和故障，不把 HTTP 成功当作本机完成 |
| 网络短暂中断 | 无操作 | 不清 Home，不结束健康比赛应用；控制能力与上游访问按既有离线规则关闭 |

## 5. 职责与部署边界

| 组件 | 负责 | 不拥有 |
| --- | --- | --- |
| GDM | 用户登录、PAM 事务、原生桌面进程链、greeter | 业务前台目标、Home reset epoch、业务放行条件 |
| GNOME/Xorg | 各自会话中的桌面、锁屏、窗口、输入 | 对另一个会话的业务管理 |
| logind | 会话枚举、精确会话身份、激活及结束；锁屏观测用于诊断 | 认证并启动 GNOME 桌面、解释业务 foreground_target |
| 镜像 | 两个账户、GDM/PAM/dconf/Kiosk 配置、Home 模板、固定登录 API 入口、包依赖 | Server 的赛事目标和凭据 |
| Device Daemon | 根据最新完整 Target 编排顺序、租约 fencing、状态上报与 UI 选择 | 直接托管 Xorg/GNOME、任意 root 命令 |
| Privileged Helper | 固定角色会话查询和操作、Home 维护窗口、挂载验证、调用固定登录入口 | 网络信任判断、赛事密码、通用用户/unit 管理 |
| Session Agent | waiting 内的静态全屏占位和既有 Binding UI、准确的 UI 就绪/失联状态 | GDM 认证、logind 控制、Home 操作 |
| Server/Panel | 前台目标、reset/terminate epoch、管理员操作与当前收敛展示 | 工作站会话 ID 的权威、工作站文件和进程生命周期 |

“独立”指不同用户、不同原生桌面进程与运行目录，结束或重置 contest 不会沿父进程链结束 waiting。两个会话仍共享内核、硬件、GDM 和系统总线，本功能不承诺物理资源故障隔离。

## 6. 状态与关键不变量

### 6.1 状态必须分开观测

必须区分三个事实，不能使用一个 `Active` 布尔量代表全部状态：

- contest 生命周期：不存在、启动中、桌面运行、结束中、歧义、错误。
- 桌面显示可用性：就绪或异常；意外 GNOME 锁屏属于异常诊断，不映射为 waiting 前台。
- seat0 前台：waiting、contest、greeter、其他、无、未知。

waiting 还需观测原生会话身份、GNOME 就绪、UI 进程及有效展示 lease。`waiting_ready` 不能仅由进程存在或 IPC 注册成功推导，必须包含 Agent 对首个全屏画面完成展示的确认。后台 waiting 可以继续更新 lease 和展示状态；只有前台、未锁定的 waiting 才能接收 Binding 输入。

每个角色在 seat0 最多一个受管用户图形会话。waiting 与 contest 各一个不构成 Ambiguous；同一角色出现多个候选、角色账户不匹配或无法证实身份时，禁止猜测目标。

### 6.2 展示状态

下表是由真实会话、Home、当前 Target 和现有 Binding 状态推导的产品状态，不另建数据库状态机。

| 产品状态 | contest | 前台 | 可以报告的结果 |
| --- | --- | --- | --- |
| 启动准备 | 不存在或启动中 | waiting/greeter | 准备中 |
| 待绑定 | 已准备 | waiting | 可按既有资格提交 Binding |
| 等待放行 | 已准备并留在后台 | waiting | waiting 目标已收敛 |
| 比赛中 | 已准备且可交互 | contest | contest 目标已收敛 |
| 清理中 | 结束中或不存在 | waiting/greeter/过渡画面 | reset 进行中 |
| Home 已完成、桌面重建中 | 启动中 | 允许过渡画面 | Home 已完成，整体尚未完成 |
| 恢复失败 | 无法确认 | waiting，必要时 greeter | 错误/恢复需要，禁止报告就绪 |

### 6.3 必须成立的不变量

| 编号 | 不变量 |
| --- | --- |
| I-01 | 普通前台切换只激活目标角色，不调用桌面 Lock/Unlock，不改变双方会话 ID、Xorg/GNOME PID、contest Home generation |
| I-02 | 正常 Home reset 不改变 waiting 会话、Xorg/GNOME/UI PID，不重启 GDM |
| I-03 | 未建立 contest 登录互锁、仍有可能进入的登录事务或仍有 contest 残留进程时，不得卸载、替换 Home |
| I-04 | 登录互锁覆盖整个 Prepare/Apply/Verify/Recover，并在 Helper 崩溃后继续生效 |
| I-05 | Home `Verified` 必须来自本次启动的真实宿主挂载和文件验证；历史 marker 不足以放行 |
| I-06 | 基于 Server Target 的登录和返回 contest 只允许当前 plan 发起；旧调用已发出时必须重新观测。固定开机预备最终回 waiting，不恢复旧业务授权 |
| I-07 | 结束会话必须绑定 boot ID 和捕获的 logind session ID；旧 terminate 不得追逐 replacement |
| I-08 | `completed_reset_epoch` 与 `completed_terminate_epoch` 只在各自 durable completion 后推进，不互相代替 |
| I-09 | Home reset 仅影响 contest Home 和为结束该用户运行环境所必需的临时资源，不删除设备身份、凭据、Binding 或 waiting 数据 |
| I-10 | API 调用返回、`SessionOpened` 或进程存在均不单独等同于桌面就绪/前台切换完成 |

## 7. 功能需求

### FR-01：账户、启动与双会话准备

1. 镜像预置固定 waiting、teams 账户，不在每次 reset 中删除和重建 Unix 用户。
2. GDM 自动登录 waiting；禁用 timed login 作为比赛登录触发器。两个受管会话均禁用自动锁屏和普通锁屏入口；waiting 另禁用屏幕空闲导致的占位消失和普通用户退出入口。意外桌面锁屏报告显示异常，不由“显示比赛桌面”自动绕过。
3. contest 只通过受控入口自动登录。首次准备和 reset 后均选择镜像中实际存在的 X11 GNOME session entry；当前镜像是 `ubuntu-xorg`。
4. 在 Home 许可、会话唯一性、GDM 入口就绪均确认后预备 contest；正常情况下建立两个会话后回到 waiting，直至满足呈现条件。固定启动预备与业务操作复用相同 Helper 排他规则；一旦有 reset 窗口，不得由启动流程绕过门禁创建会话。
5. GDM/waiting 的启动不能以 contest Home 恢复成功为必要条件。Home 恢复失败时仍应能展示 waiting；门禁默认仅拒绝 contest。
6. 登录入口按一次操作执行。已存在正确 contest 时返回其真实状态，不再创建第三个图形会话。
7. 启动顺序已选定：GDM 自动登录 waiting；独立 Home 恢复任务在宿主 mount namespace 验证 Home；固定会话预备任务等待两者的实际结果，只在 Home 许可后请求 GDM 登录 contest，最后激活 waiting。Home 恢复失败不传播为停止 GDM/waiting 的依赖。固定预备是 Helper 的受限本地能力，镜像 systemd unit 只是调用者，不成为第二个业务编排器。

### FR-02：waiting 静态占位与既有 Binding

1. 复用 Slint/Skia Session Agent，只在固定 waiting 会话内承载全屏静态占位；默认纯黑，可随镜像提供一张居中、等比缩放的 ICPC logo，剩余区域仍为黑色，素材不可用时回退纯黑。contest 不再显示旧的 Binding 遮罩。
2. waiting 选择发行版正式 `gnome-kiosk-script-xorg` 会话；使用官方 `org.gnome.Kiosk.Script.service` 的 root 安装 drop-in，直接执行 `/usr/bin/natsume-session-agent run` 并设置 `ConditionUser=waiting`。服务是唯一启动所有者，删除旧系统 XDG entry，不通过用户 Home 中可改写的示例脚本启动，不增加第二个 keeper。
3. UI 使用 Slint `Window.set_fullscreen(true)`，根布局随实际窗口尺寸变化，窗口初始尺寸使用 preferred size，不能固定根内容大小。必须覆盖目标显示器可用区域，无窗口装饰；在 1280×800、1920×1080 和镜像支持的缩放下布局可用。全屏最终状态必须在真实 GNOME/X11 中验证，不能只检查应用设置值。首帧渲染确认、当前尺寸和有效 Agent lease 共同参与就绪判断；渲染回调不能单独证明窗口在前台。
4. 本期等待、清理、桌面启动、恢复失败均可使用同一占位，不增加状态文案页面、进度条和动画；详细状态由 Panel/Actual 提供。未来复杂 Skia 页面只改变 Agent 的内容实现，不改变双会话职责及切换 API，本期不预建页面协议或通用渲染抽象。
5. BindingPrompt/BindingPending 复用现有 negotiation、submission epoch、错误码和提交机制；只在部署窗口、未绑定、Home 就绪、contest 已准备以及当前 plan 允许时开放输入。
6. waiting 转到后台时关闭 Binding 输入资格但保留 Agent lease；再次转到前台需重新验证资格。contest 身份不得注册或续约 waiting 的 UI lease。
7. IPC 断开、lease 失效或 Snapshot 不可验证时，立即禁止输入并撤销旧 Binding 资格，保留静态占位。不能沿用当前实现的无条件 Hidden，使 waiting 桌面暴露为正常可操作界面。纯黑由真实已渲染窗口提供，不能把黑屏或无信号当作 UI 已就绪。
8. 普通关闭窗口操作通过 Slint close callback 保持窗口。Kiosk Script 服务配置 `Restart=always`、`RestartSec=1`、`StartLimitIntervalSec=60` 和 `StartLimitBurst=5`。Agent 不再使用 RegisterClient/AutoRestart，避免 GNOME session 和 systemd 同时重启同一应用。新机制需用当前 QEMU 验证，历史完整 GNOME 的 AutoRestart 证据不算新机制通过。
9. 持续 UI 故障时撤销展示资格；等待不少于 60 秒后，允许通过固定 GDM 入口仅重建 waiting 一次。自动恢复次数归属本次 boot 的受管本地恢复状态，不因 Daemon 重连或 lease 更新而清零；再次失败在 Panel/Actual 保留错误，等待管理员维护或重启。故障恢复不结束 contest、不重启 GDM，不用于普通前台切换或正常 Home reset。恢复期间可显示 greeter，旧 waiting Agent lease 立即失效。
10. waiting 使用 GNOME Kiosk 及独立 dconf profile，不启动完整 GNOME Shell 或其 ArcMenu 扩展；contest 保留比赛桌面默认配置并增加受管会话策略。已验证 Kiosk 46 实际选择 `gnomekiosk` profile；镜像应配置 `/etc/dconf/profile/gnomekiosk` 及专用 system 数据库和 locks，保留发行版 file-db，用户运行环境也使用这一 profile。不能仅设置未被 Kiosk 采用的 `natsume_waiting`。系统级 Xorg `DontVTSwitch` / `DontZap` 配置限制键盘 VT 切换和终止 X server，仍允许受控 logind 激活；显示、空闲及环境配置见独立镜像清单。

### FR-03：显示等待界面

触发：当前有效 `foreground_target=waiting`，或者未绑定设备需要展示 Binding。

1. 解析 waiting 和 contest 的精确身份，确认 waiting GNOME 与全屏 UI 可用。
2. 通过 Helper 激活捕获的 waiting 会话，不向 contest 发出 Lock，也不向 waiting 发出 Unlock。
3. 完成条件是 waiting 首帧占位已就绪且实际在前台显示；LockedHint 和 GNOME 锁屏画面均不能证明 waiting 目标完成。
4. 不终止 contest 的比赛程序、不重置 Home、不重建任一会话。
5. waiting 不可用时报告切换失败，必要时显示 greeter；不以额外锁定 contest 冒充成功。不得为修复 waiting 重启 GDM 或结束 contest。

### FR-04：显示比赛桌面

触发：最新有效 `foreground_target=contest`，设备已绑定，Home 当前目标已完成，且没有未完成的 reset/terminate。

1. 已有唯一、就绪 contest 时，激活该精确会话；不调用 GNOME/logind Unlock，不重新登录。
2. 只有真实前台为 contest 且比赛桌面可交互才报告 contest 目标已收敛；API 返回不是完成判据。意外锁屏、greeter 或桌面未就绪时报告未完成，不以 LockedHint 的变化代替前台验证。
3. 若 contest 尚在启动，则保持准备状态，完成后重新读取最新 Target；不得保存一个独立“稍后无条件切回”的命令。
4. 等待期间新的 waiting Target 必须阻止旧计划继续切回；已发出的调用不能撤销时，重新观测并按当前 waiting 目标收敛。

### FR-05：完整 Home reset

管理员只需提交一次 reset，不需要自行编排 terminate、清理和 login。

| 步骤 | 必须执行与验证 | 失败处理 |
| --- | --- | --- |
| 1. 接受目标 | 使用 Server 单调 reset epoch；撤销 Binding 输入并确认 Caddy 已 BLOCKED | 未确认阻断不得开始破坏性操作 |
| 2. 展示 waiting | 确认 waiting 占位可用并激活该会话 | 常规 reset 不自动继续；保留旧比赛环境并报告等待界面不可用 |
| 3. 建立窗口 | Helper 持久化 epoch、阶段及原 contest 精确身份，关闭 contest 登录许可，停止/取消正在进入的登录事务 | 窗口不完整、门禁状态未知时不操作 Home |
| 4. 结束 contest | 结束捕获的比赛会话；在固定账户范围内清理用户 manager、运行进程及残留会话；等待实际消失 | 超时保留门禁；不以粗暴卸载绕过 |
| 5. 重置 Home | 获得排他维护权，普通卸载旧 OverlayFS，按版本模板创建并挂载新 generation | busy、缺模板、空间不足或挂载失败进入恢复需要 |
| 6. 验证并提交 | 校验宿主 mount namespace、lower/upper/work、所有权、模板和脏数据清除；完成 durable progress | 不能仅写 marker 报成功 |
| 7. 允许受控登录 | 验证后发布当前启动的许可，释放维护窗口对新登录的排斥；按最新目标调用登录入口 | 不因登录失败再次清空已验证的同一 generation |
| 8. 等待桌面 | 确认 GDM session、用户桌面就绪及精确身份 | 超时报告启动失败，允许对同一事实重试 |
| 9. 恢复呈现 | 最新前台目标为 waiting 则激活 waiting；为 contest 且已绑定则激活 contest | 保持真实未收敛状态，不推进虚假的展示成功 |

步骤 3 开始到步骤 9 完成期间允许 GDM 自行切屏。不得保留实验旧脚本中“登出后立即抢回 waiting，且必须一直 Active”的前置条件；该操作已被实测证明可能与新 greeter 的 VT 切换竞争。

重建完成判定分成两个层次：Home 完成可上报 `HomeState=Steady` 和新 `completed_reset_epoch`；工作站可使用还必须满足比赛桌面与呈现目标。Panel 必须分别显示，不能在 Home Verify 后就宣告全部完成。

### FR-06：contest 登录互锁

这是替代“停止整个 GDM”的发布必需项，不能只靠 Daemon 的调用顺序。

1. 必须覆盖当前镜像中所有允许 contest 进入的图形登录路径，包括专用登录服务、普通密码登录和自动登录。对于不允许的账户入口，镜像应配置为不可用。
2. waiting、greeter 的 PAM 事务不能被 contest Home 门禁阻塞；门禁只针对固定 contest 身份。
3. 必须处理“PAM 已通过检查但尚未注册 logind session”的竞态；单次检测 ready 文件后立刻放行，不满足本要求。
4. 进入维护窗口后，排空/取消已开始的 contest 登录事务；在确认没有事务、会话、UID 进程和 Home 占用后才允许挂载变更。
5. 状态目录由 root 独占，许可每次启动重建；Helper 异常退出不能令默认策略变成允许登录。
   镜像还必须禁用 contest 的非受管 SSH、TTY 登录和能自行重启的用户任务；否则仅有图形 PAM 门禁不足以证明该 UID 不会在 Home 修改期间再次产生进程。管理员维护使用独立账户。
6. 同一 epoch 恢复复用已有进度；另一个 epoch 不得覆盖进行中的维护记录。新 Target 可以等待当前不可中断的维护安全结束，然后执行最新目标。
7. 选定发行版 `pam_exec.so` 调用 Helper 提供的固定本地门禁子命令，不交付自定义 PAM `.so`。在允许的 PAM 栈的 auth、account、open_session 阶段执行，非 contest 和 close_session 直接通过；所有其余官方 PAM 检查仍保留。`gdm-contest`、`gdm-autologin`、`gdm-password` 的组合不得存在绕过该检查的 sufficient 分支。
8. 门禁在短时共享文件锁内检查本次启动的 Home 许可，并在返回 PAM 前原子登记调用它的 GDM worker：boot ID、PID、进程启动时间。状态由 root 独占；调用者必须验证为实际 GDM worker。登记一直保留到该精确进程退出，不在 pam_exec 返回或 close_session 时提前删除。PAM 环境变量不能直接作为身份凭据。
9. Helper 先关闭许可，再取得同一文件的排他锁；在锁内验证没有仍存活的已登记 worker、contest 会话、该 UID 进程和 Home 占用，才允许修改挂载。`pam_exec` 返回后文件锁可能已空闲，因此“拿到 flock”不等于已排空登录事务。取消超时或记录损坏时保持关闭并报错，不能忽略记录、按裸 PID 杀进程或继续清 Home。
10. `/run` 许可和 worker 登记在开机重新建立；持久窗口与完成 epoch 继续由 Helper 管理。异常退出释放内核文件锁，但不会自动产生许可，也不会删除仍存活的 worker 登记。此方案已通过三阶段 PAM 暂停及调用者崩溃试验；正式固定子命令、权限与升级交付仍须进行产品集成验收。

### FR-07：通过现有 GDM API 请求登录

当前版本采用已实测的 greeter API 路径：

1. Helper 请求 GDM 的 `LocalDisplayFactory.CreateTransientDisplay` 创建 greeter，或复用正确的现存 greeter，并等待其可用。
2. 镜像提供固定的单次登录入口，以 `gdm` 身份运行且不继承管理员 SSH 会话。入口只调用已安装的 libgdm/GDM D-Bus 接口。
3. 入口选择 `ubuntu-xorg`，调用 `BeginVerificationForUser("gdm-contest", "contest")`，在 `SessionOpened` 后调用 `StartSessionWhenReady`。
4. 使用独立 PAM 服务名 `gdm-contest`，限制为 contest，并组合正式 PAM 栈及 FR-06 的门禁；不要直接把服务名写成 `gdm-autologin` 再传 contest，避免与全局 waiting 自动登录分支冲突。
5. 入口必须受限、单飞、有超时、退出状态明确。正常客户端退出不得结束 GDM 创建的桌面；调用超时后先观察是否已建立会话再决定重试。
6. 对外不接受用户名、session entry、PAM 服务名、环境变量或任意命令。由 Helper 调用固定 systemd unit/API 入口，普通 Agent 无权调用。
7. 不承诺这些 greeter 接口是跨 GDM 版本稳定的管理员 API；以当前发行包版本建立兼容性测试。GDM 的调用者 display/UID 检查见[官方 46.2 源码](https://raw.githubusercontent.com/GNOME/gdm/46.2/daemon/gdm-manager.c)。
8. waiting 故障恢复复用同一调用方式，但使用另一个固定角色入口及只接受 waiting 的 `gdm-waiting` PAM 服务。Helper 只允许显式的两种角色能力，不把任意用户名/PAM 名传给调用者。实测仅重建 waiting 时 contest 和 GDM 保持原实例。正常开机仍用 GDM waiting 自动登录。

镜像可配置的基础片段如下；它们不是包含门禁的完整发行配置：

```ini
# /etc/gdm3/custom.conf
[daemon]
WaylandEnable=false
AutomaticLoginEnable=true
AutomaticLogin=waiting
TimedLoginEnable=false
```

自动登录配置项来自[GNOME 官方管理文档](https://help.gnome.org/system-admin-guide/login-automatic.html)。专用 PAM 服务使用 `pam_succeed_if.so user = teams` 并包含带有 FR-06 门禁的 `gdm-autologin` 栈。上面的 GDM 配置不能单独代替 PAM 互锁。

### FR-08：恢复、离线与异常

| 情况 | 要求 |
| --- | --- |
| 重复 Target / 同一 epoch 重试 | 继续同一进度；不重复清 Home，不重复创建会话 |
| reset 中收到 contest 前台目标 | 只更新最新目标；Home 验证前不能提前登录/激活 |
| reset 中收到新的 waiting 前台目标 | 完成必要维护后保持 waiting；旧 contest 目标不再有资格 |
| 新 reset epoch 到达 | 不并行执行两次维护；完成当前已进入的安全阶段后处理最新 epoch，未开始的中间目标可合并 |
| Daemon 失联/重启 | 撤销旧 lease 的新业务副作用；不结束健康桌面、不自动清 Home；重连后重新观测 |
| Helper 在维护中崩溃 | 门禁保持关闭；重启恢复同一 epoch，不能凭旧 Verified 放行 |
| reset 中断网 | Helper 可完成已经取得所有权的本地维护以恢复文件系统一致性；无当前有效 Target 时不新发起返回比赛，回 waiting |
| 已在比赛中断网 | 保留桌面和比赛进程，沿用现有上游 BLOCKED/离线策略；不新增自动 logout/reset |
| GDM 重启/主机重启 | 双会话身份失效；重新建立 waiting，恢复 Home 后预备 contest；旧 boot/session 不能用于操作新会话 |
| contest 意外退出 | 关闭本地访问、显示 waiting；Home 健康且存在当前有效目标时可经同一入口重建，不隐式 reset |
| waiting UI/会话故障 | 报告不可用；仅恢复 waiting，不结束 contest 或重启整个 GDM 作为自动修复手段；必要时显示 greeter |
| 多个 contest 候选 | Ambiguous，关闭访问，拒绝自动选择；不扩大 terminate 范围清除未知会话 |
| Home 已验证但登录失败 | 保留已完成 epoch，显示桌面启动失败，重试登录；不重做同一 reset |
| 模板缺失/挂载 busy/状态损坏 | 保留门禁及可恢复进度，不强制卸载，不宣告成功 |

离线恢复与正常业务授权分开：恢复文件系统一致性不等于获准进入比赛。现场管理员已有的系统维护权限保留，但不添加面向选手的跳过门禁按钮。

## 8. API、状态上报与并发契约

### 8.1 Server API

复用现有 session-control 路径，请求使用 `foreground_target`；以下为接口及其目标行为。不增加通用 login/switch API：

| 接口 | 本功能语义 |
| --- | --- |
| `PUT /api/v2/devices/{device_id}/session-control`，`{"foreground_target":"waiting"}` | 显示等待界面，普通切换不结束会话 |
| 同接口，`{"foreground_target":"contest"}` | 当前依赖满足后显示比赛桌面，不调用桌面 Unlock |
| `POST /api/v2/devices/{device_id}/home/actions/reset` | 推进 reset epoch；Client 自动完成整个重置和重新登录流程 |
| `POST /api/v2/devices/{device_id}/session-control/actions/terminate` | 保留既有显式维护能力，结束捕获的会话；不清 Home，旧 epoch 不得影响随后建立的会话 |
| 现有 Device convergence 查询 | 同时查看前台 Target、Home 完成、contest 生命周期、waiting 就绪及实际前台 |

terminate 是一次 transition，不是“永远保持无比赛会话”的 level；本功能正常收敛可以在其 durable completion 后重新准备 contest。常规暂停只设置 waiting 前台目标，Home reset 不需要另外推进 terminate epoch。

HTTP 200 只表示目标已提交。相同 reset Target/epoch 的重放必须幂等；重复 POST reset 会创建新 epoch，不可将 HTTP 重试等同于同一操作重放。Panel 在请求结果未知时先重新读取当前目标，禁止自动重复提交破坏性操作。

### 8.2 本地能力

保留 `org.natsume.Privileged1` 和 `org.natsume.Device1` 边界，只新增当前消费者实际需要的固定能力。下表的新增名称为语义契约，最终 Rust/D-Bus 签名在实现中统一生成，不维护第二份 XML 权威。

| 能力 | 契约 |
| --- | --- |
| 查询受管会话 | 返回 waiting/contest 的精确身份、生命周期、桌面就绪及实际前台；意外锁屏是诊断，后台不等于歧义 |
| 激活受管角色 | 只允许 waiting/contest，绑定预期精确身份；不接受任意 session/seat |
| 确保 contest 已登录 | 固定用户/桌面/seat/PAM，门禁及 Home 允许后单飞执行；已存在则观察并复用 |
| 恢复 waiting | 仅用于观测到的 waiting 故障；绑定旧会话精确身份，有冷却和次数限制；通过固定 GDM waiting 入口建立 replacement，不结束 contest |
| Prepare/Apply/Verify/Recover Home reset | 延续现有 epoch 与分阶段能力；Prepare 建立门禁并负责捕获/结束 reset 对应的旧比赛运行环境 |
| waiting UI IPC | 精确 waiting Agent 注册、续约、读取 typed snapshot 和既有 Binding 提交 |

Helper 内 Session mutation、登录入口和 Home 维护必须共享一个有明确所有权的排他规则；不得仅在各自模块内单独加锁，使登录与重置仍可同时进入。等待长任务时不能阻塞 WSS heartbeat/deadline。

### 8.3 Target/Actual 与 UI 数据

1. Session 目标改为 `SessionControlTarget { foreground_target, terminate_epoch? }`，`foreground_target` 只接受 waiting/contest。`HomeTarget { reset_epoch? }` 不变。字段、枚举和持久目标的命名随 Proto、local-control-api、HTTP/OpenAPI、Server/Client、Panel 生成类型一起迁移，不保留 lock_state 的并行业务别名；不新增命令投递模型。
2. `SessionControlActualState.session_state` 仅描述 contest 生命周期；健康运行统一为 `Running`，不再以 `Active`/`Locked` 区分 GNOME 锁屏或业务前台。运行但在后台的合法 contest 不再编码为 Ambiguous。该目标变化须同步到 Proto、local-control-api、Server/Client 和生成接口，不能只改 Panel 标签。
3. Actual 最少补充 `foreground`（waiting/contest/greeter/other/none/unknown）及 `waiting_ready`。这些字段来自真实新鲜观测，不落入业务事实表；未知值不能解释为已收敛。
4. Helper 本地观测另包含两边精确 `GraphicalSession`、桌面就绪及异常诊断。锁屏观测只用于识别意外显示异常，不驱动正常业务切换。无需向 Server 暴露进程树、Xauthority、DISPLAY 或总线地址。
5. 前台目标的 convergence 必须验证目标角色的实际前台与显示就绪，不能只比较 `session_state`：waiting 目标要求 `waiting_ready`，contest 目标要求比赛桌面可交互及当前放行条件满足。Home 完成与桌面重建完成独立展示；contest 目标被“尚未绑定”阻塞时，Panel 显示等待绑定，不把预期 waiting 误报为会话故障，也不报告 contest 目标已完成。
6. `SessionUiSnapshot` 在已绑定时展示同一完整目标的队伍／学校资料和 Logo，未绑定时保留 BindingPrompt/BindingPending 或通用 waiting。失联保留非秘密缓存并标离线，不保留输入资格；资料、Logo 与缓存边界见[架构说明 §15.2](architecture.md)。详细会话恢复状态仍在现有 Actual/Panel 边界。
7. 继续使用现有 complete snapshot、current/queued plan 去重、lease fencing、单调 epoch 与 durable completion。不得另建命令队列或业务操作历史作为正确性来源。

### 8.4 本地访问和 Binding 资格

- Home Resetting/RecoveryRequired、contest None/Starting/Terminating/Ambiguous/Error，以及 epoch 未精确匹配时，保持现有 Caddy BLOCKED 和禁止新 Binding 的规则。
- waiting 正常位于前台、contest 健康地留在后台，不是访问异常。已绑定设备显示等待界面不因前台切换本身撤销 Binding 或必然重载 Caddy；切到 waiting 本身允许保留上游访问。
- 新 Binding 输入除了既有部署/Target/Home 条件，还必须要求 waiting 是前台、UI lease 有效、身份精确；保持旧 negotiation/submission fencing。
- 重置和登录后的 Actual 必须重新采样，不能复用维护前的运行或前台事实恢复 READY。

GNOME 46 实测会将 Agent 放入 `user@UID.service` 下的独立应用 scope，`GetSessionByPID(AgentPID)` 返回无会话；不能只替换旧鉴权逻辑中的用户名。身份解析选定如下规则：

1. 从系统总线的调用者凭据取得 PID 和 UID，校验固定 waiting UID、当前 boot 和声称的 session。不能从 Agent 自报的 UID、PID、`XDG_SESSION_ID` 或用户环境变量取得权威身份。
2. `GetSessionByPID` 成功时必须精确匹配 waiting；返回其他会话时直接拒绝。只有明确返回无所属会话，才进入 GNOME 应用 scope 分支；权限错误、超时不降级。
3. 在该分支中，`GetUserByPID` 必须返回 waiting；系统 systemd 的 `GetUnitByPID` 必须对应 waiting 的 `user@UID.service`；logind 中该账户只允许一个本地图形会话，且 `User.Display` 精确指向同一个 waiting/seat0/X11 会话。任一事实缺失或存在其他登录会话时拒绝注册和续约。这个分支是唯一受管用户运行环境到图形会话的映射，不宣称进程实际位于 `session.scope`。
4. 映射在前后台切换时保持有效，前台仅影响 Binding 输入资格。lease 继续绑定调用者连接及精确 boot/session；会话消失或 replacement 到来时旧 lease 失效。
5. 镜像禁用 waiting 的 linger、SSH/TTY 等其他登录入口。重建 waiting 前，必须排空其旧会话、用户 manager 和 UID 进程，验证旧 Agent 已退出，再创建 replacement，避免残留进程被映射到新会话。这不会结束 contest 的用户运行环境。

上述 logind/systemd 映射与后台事件循环已在当前 VM 验证；正式 Device1 的鉴权、伪造 caller 和旧 lease 拒绝仍属于集成测试，不能以该原型代替。

## 9. 镜像与打包需求

当前测试镜像不是最终发行镜像。具体修改位置、交付归属、配置建议和待验收项集中维护于 [镜像变更清单](gnome-session-image-requirements.zh-CN.md)。

| 交付项 | 要求 |
| --- | --- |
| 固定账户 | waiting 与 teams UID/Home 由镜像创建；不共享 Home、Xauthority 或用户总线；双方禁用 linger、其他登录入口及不受管后台服务 |
| 图形配置 | 官方 GDM/GNOME，X11 session entry，waiting 自动登录；无资源覆盖、源码补丁和替换 greeter |
| waiting 会话 | 独立 dconf profile 与 system 数据库，避免继承 contest 的 ArcMenu 等扩展锁定；全屏静态占位、禁用普通锁屏与退出路径；验证键盘、绑定输入、缩放、休眠/屏保策略 |
| 静态素材 | 默认纯黑，无外部资源依赖；选择 ICPC logo 时仅随镜像提供一张本地图片，等比缩放，读取失败回退黑色；不提供远程素材 API |
| contest 登录入口 | 固定 systemd/API 入口、gdm 身份、访问策略、并发与超时限制；入口退出不影响桌面 |
| PAM 门禁 | 官方 pam_exec + 固定 Helper 子命令 + 精确 worker 登记；对 contest 的各图形登录入口一致生效；包升级后仍生效；等待会话不被门禁阻塞 |
| Home 模板 | 版本化只读模板和完整 Browser/IDE 默认配置；明确来源、所有权、挂载位置与模板升级方式 |
| Home 状态 | 使用 Helper root-owned 状态根和宿主 mount namespace；窗口显式版本化且只解析当前格式，未知或损坏格式保持门禁关闭 |
| 原有 drop-in | 替换全局 `ConditionPathExists=home-ready` 与停 GDM 互锁；避免残留配置在启动/升级时结束 waiting |
| Agent 启动 | 接入官方 GNOME Kiosk 用户服务、waiting 资格检查并移除旧 XDG 启动入口；不添加外部 GUI runtime |
| 依赖 | 正式 Deb/镜像闭包提供所需 libgdm、图形/PAM 依赖；安装期不下载实验 Python 工具或临时编译产物 |
| 运维材料 | 保存发行包版本、镜像标识、配置校验、双会话/恢复日志与 GUI 证据；不采集秘密 |

不以全屏窗口宣称已解决所有快捷键逃逸。必须测试 Alt+F4、Super、Alt+Tab、会话切换入口及目标镜像启用的 VT 快捷键；常规入口通过官方 GNOME/Xorg 配置限制，并记录仍保留的管理员维护入口。

## 10. Operator Panel 与诊断

Panel 复用当前设备 Target/Actual 页面：

- 两个操作按钮分别命名为“显示等待界面”和“显示比赛桌面”，对应设置 waiting/contest 前台目标；说明普通切换会保留比赛应用，不使用锁定/解锁标签。
- reset 提交前明确说明“结束比赛应用并清除比赛 Home，按当前前台目标决定完成后显示的界面”；保留已有破坏性操作确认，不为普通切换新增确认流程。
- Panel 展示 Home reset epoch 及完成 epoch，并区分“正在清理”“Home 已完成，桌面准备中”“等待放行”“比赛桌面可用”“恢复失败”；这些文案不要求同步成为本期 waiting 页面。
- waiting 状态、实际前台或当前 lease 缺失时显示未知/离线；不把最后一次成功画面当作当前事实。
- 重试登录不推进 reset epoch；再次 reset 是新的破坏性请求，不能在错误页默认自动执行。

诊断使用现有日志/trace 和当前 Actual，记录阶段、精确 session/boot、epoch、调用结果及耗时；不记录密码、认证 cookie、Xauthority、私有总线地址和 Home 内容。日志不成为业务状态或完成判据，不新增业务审计系统。

## 11. 非功能要求与测量口径

以下是待验证的工程验收目标，不是前期脚本的实测性能结论。

| 项目 | 目标/口径 |
| --- | --- |
| 普通切换 | 双桌面已就绪时，从 Client 接受有效 Target 到目标画面可交互，100 轮测试 P95 ≤ 3 秒；排除网络传输时间 |
| contest 冷登录 | 从允许发起 GDM 登录到真实 GNOME 桌面就绪，当前 QEMU 系统 ≤ 30 秒；单独统计 |
| reset | 在版本固定的正式 Home 模板、无外部占用的工位上，全流程目标 ≤ 120 秒；记录文件数量、磁盘和模板版本，不把等待管理员另行设置 contest 目标算入耗时 |
| 超时 | 沿用本地 D-Bus 单调用 10 秒 deadline；较长工作按可查询进度继续，超时表示结果未知，不等于远端取消 |
| 失败可见性 | 一个受限调用/阶段到达超时后，在下一次 UI/Actual 更新中显示未完成或失败，不能无限显示成功/等待且无原因 |
| 常态稳定 | 100 轮普通切换后无额外受管 session、Xorg、Shell、Agent；会话及 Home generation 不变 |
| 重置稳定 | 20 个独立 reset epoch 后没有旧 contest 运行资源或未清理 generation 累积；waiting 及 GDM 保持同一实例 |
| 重启持久化 | 10 次冷启动/重启正确恢复 Home 与等待展示；不依赖 `/tmp` 脚本、手工环境变量或一次性运行许可 |

若性能目标未达到，记录实际分位数和原因；不能通过提前上报 Running/前台成功、跳过占位首帧或 Home 验证、使用 lazy/force umount 达标。按用户 2026-09-08 的验收范围，本次使用当前 QEMU 系统，不以物理机作为完成门槛；显示、输入、休眠结论限于实测模拟设备，不外推真实 GPU/驱动兼容性。

## 12. 验收用例

### 12.1 正常流程

| 编号 | 用例 | 通过标准 |
| --- | --- | --- |
| AT-01 | 新镜像无当前 Server Target 启动 | 首个稳定业务界面为 waiting；没有未经授权的业务放行 |
| AT-02 | 自动预备两个会话 | 两个账户各一个原生 X11 会话，独立进程链；GDM 是两边桌面的管理者 |
| AT-03 | 等待 → 比赛 → 等待 100 轮 | I-01 成立；桌面/编辑器文件和窗口保持；真实键鼠可用 |
| AT-04 | 检查前台切换副作用 | 只激活 waiting/contest，不调用桌面 Lock/Unlock；LockedHint 不能代替真实前台完成判据 |
| AT-05 | waiting 目标下 reset | contest 精确身份及 Home generation 改变，waiting/GDM 不变；新 contest 留在后台，前台 waiting 占位 |
| AT-06 | contest 目标下 reset | Home 验证后新比赛桌面就绪并位于前台；旧会话、脏数据消失 |
| AT-07 | reset 中先提交 contest、后提交 waiting 目标 | 最终按 waiting 目标收敛，无旧计划持续把 contest 激活 |
| AT-08 | reset 后延迟显示比赛桌面 | 延迟期间两会话存活；切回 contest 不再登录或清 Home |
| AT-09 | 同一 Target/epoch 多次下发 | 不产生多余 generation、登录事务或第三个受管会话 |
| AT-10 | API 登录入口退出/崩溃 | 已由 GDM 启动的 contest 正常存活；重试先观察实际结果 |
| AT-11 | waiting Binding | 原绑定提交语义保持；后台/失联/伪造身份不能提交 |
| AT-12 | 静态占位全屏 | 两种基线分辨率下纯黑正常显示；采用 logo 时等比缩放、缺图回退纯黑；首帧真实完成，普通关闭和失联不移除占位；既有 Binding 输入单独回归 |

### 12.2 互锁、崩溃与故障

| 编号 | 用例 | 通过标准 |
| --- | --- | --- |
| AT-13 | 并发专用登录/密码登录与 reset | 所有 contest 入口均受互锁；没有登录事务跨入 Home 修改区间 |
| AT-14 | PAM 检查后、logind 注册前暂停登录 | reset 不能误判“无会话”直接清理；取消/排空后才能继续 |
| AT-15 | contest 残留进程、user manager 或 Home cwd 占用 | 有界清理或明确拒绝；无 force/lazy umount；waiting 不受影响 |
| AT-16 | 模板缺失、磁盘满、挂载失败 | 无新登录许可、无虚假完成；修复后同 epoch 恢复 |
| AT-17 | Helper 在窗口写入、Prepared、Applied、Verified 后退出 | 重启后同 epoch 恢复；真实挂载验证前不能放行 |
| AT-18 | 在 AT-17 各阶段 VM 硬复位及断电重启 | 下次启动 waiting 可用，contest 门禁关闭至宿主 Home 恢复；旧 boot 身份失效；分别标明硬复位和断电证据 |
| AT-19 | Daemon/Helper 正常重启 | 健康 contest、waiting 不因服务重启被销毁；重新观测后恢复资格 |
| AT-20 | reset 期间断网/lease 替换 | 本地维护可安全结束，旧 lease 不再发起返回比赛；新 Target 决定呈现 |
| AT-21 | 激活/登录调用超时后晚到 | 旧操作不能永久覆盖当前前台目标；观察未知结果，不重复破坏性操作 |
| AT-22 | 两个 contest 或伪造 Agent | Ambiguous/拒绝，不能误操作 waiting/其他用户或提交 Binding |
| AT-23 | waiting 程序/会话退出 | 报告展示不可用并有界恢复；不以停止 GDM/contest 为自动修复 |
| AT-24 | GDM 重启和发行包升级 | 默认回 waiting，重新验证会话身份、API/PAM 配置和 Home；没有残留实验资源覆盖 |
| AT-25 | Host namespace 与 marker 欺骗 | 私有 mount namespace、损坏进度、仅写 Verified 均不能产生允许登录的假成功 |
| AT-26 | 上游与身份保留 | reset 中先 BLOCKED，完成后重新观测；设备身份、Binding、凭据和 waiting Home 未被清除 |

故障注入必须能定位到被测阶段，随机 kill 一次不能代替完整 crash-cut 覆盖。mock 测试不能替代当前 QEMU 系统中真实 PAM/logind/GDM/OverlayFS 和显示链的验证。

### 12.3 证据要求

每轮记录镜像/软件包版本、Target 与 epoch、前后双方 session ID 和 boot ID、Xorg/Shell/Agent PID、GDM InvocationID、Home generation 与验证结果、真实前台、锁屏状态、阶段耗时和错误结果。正常显示与关键重建阶段保留截图或视频；敏感目录不采集内容。

机制原型和阶段 VM 结果不能替代正式 Client、打包后的门禁和发行镜像的全矩阵验收。每项通过结论必须注明实际验证的版本与范围。

## 13. 实施拆分与交付门槛

| 阶段 | 工作与交付 | 完成条件 |
| --- | --- | --- |
| A：镜像能力定型 | 官方 GDM/GNOME 配置、固定登录 API 入口、正式 contest PAM 互锁、waiting 与模板配置 | 在无源码/资源 patch 下通过 AT-02、AT-10、AT-13～18；明确解决完整 PAM 生命周期竞态 |
| B：Helper 能力迁移 | 非前台会话识别、固定角色激活、登录单飞、仅 contest 的 Home 窗口与恢复 | 不停止 GDM；精确身份、并发和 durable recovery 测试通过 |
| C：Client 收敛 | reset 内自动结束/重建、最新前台目标呈现、epoch fencing、离线与访问规则 | AT-03～09、AT-19～22、AT-26 端到端通过 |
| D：UI 与 Panel | waiting 纯黑/单张 logo 全屏、既有 Binding 迁移、失联保留占位、前台 Target/Actual 与生成接口更新 | AT-11～12、AT-23；复杂 Skia 等待页面不在本期，无第二套绑定协议和 UI runtime |
| E：发行验收 | 正式 Deb/镜像、重启/升级/模拟设备回归、证据与运维 runbook | 100 轮切换、20 轮重置、10 次启动及全矩阵通过，完成包校验 |

实现涉及的现有边界：

- `client/privileged-helper/src/session.rs`、`home.rs`、`home/window.rs`、`lib.rs`。
- `client/device-daemon/src/reconcile.rs`、`reconcile/session.rs`、`reconcile/home.rs`、`reconcile/binding.rs`。
- `crates/local-control-api`、`crates/device-protocol` 及对应 Server convergence/HTTP schema 与 Web 生成类型。
- `client/session-agent`、`packaging/client/rootfs`、`packaging/image` 以及 ICPC 镜像仓库相关模块。

按实际变更执行相关 Rust 测试/Clippy/fmt、协议与 API 生成检查、Web 检查、package lifecycle 和目标 VM 验收。不要为本功能顺带重构无消费者的公共抽象或改动无关 Caddy 实现。

### 13.1 镜像交付

镜像项目的具体交付见 [IMG-01～08 要求](gnome-session-image-requirements.zh-CN.md)及[配置附录](gnome-session-image-configuration.zh-CN.md)。独立交付时提供完整 [packaging/image](../packaging/image/README.md) 目录；其中的实施要求和验收标准不依赖开发记录。

## 14. 发布、维护与回退

1. 本期按当前数据库、Home 窗口格式和 waiting/teams 账号全新部署，不提供重构前版本的迁移或兼容路径。在用工位只在维护窗口变更，先由当前 Helper 完成已持有的 Home 维护，保留完成 epoch 和设备身份。
2. 在可回滚 QEMU 系统完成端到端验收，并验证正式镜像从零安装；本次不等待目标物理机。保留工作站系统镜像和与当前业务匹配的必要备份。
3. 同次交付更新架构、Helper/Daemon/Agent、协议/Panel、镜像配置和运行说明；contest PAM 门禁与固定登录入口必须一起交付。
4. 升级安装不在选手比赛中静默重启 GDM；需要重新登录/重启的变更明确安排在维护阶段。
5. 回退以经过验证的完整 Client/Server、协议、数据库与镜像备份为单位，不单独降级 Helper 或混用状态。进行中的 Home 维护先安全恢复，不能回退到没有门禁的半配置环境。
6. Home reset 的内容删除不可逆，本功能不提供撤销/自动保存比赛文件。Panel 确认文案与运维流程必须说明这一点。
7. Client 安装后持续保留，remove/purge 不作为本期交付条件。维护仍须按本节完成会话、Home 与镜像配置的交接。

交付完成的定义：正式镜像能够从冷启动进入 waiting，建立两个独立原生会话，执行可重复的普通切换及完整 Home reset；全部关键故障保持门禁和可恢复性；Natsume 仅通过固定 API 编排，GDM 始终管理比赛桌面；相关源代码、配置、测试、生成接口、架构与运维文档一致。
