# 实施计划：迁移到 GNOME 原生双会话

| 项目 | 内容 |
| --- | --- |
| 日期 | 2026-09-07 |
| 状态 | R0 已实现、检查并通过审查；R1～R5 未开始 |
| 代码基线 | `c8379d0`；其中 `8a47763` 提交新架构、PRD 与历史 VM 验证记录 |
| 目标权威 | [主架构](architecture.md) §4.3～4.4、§9.4～9.6、§14.3、§18～19 |
| 产品与验收 | [PRD v1.3](prd-gnome-dual-session.zh-CN.md)，尤其 §7～8、§11～14 |
| 机制证据 | [GNOME 双会话验证记录](gnome-dual-session-validation.zh-CN.md)；原型证据不等于正式实现通过 |

本计划只定义从当前代码到已接受架构的实施顺序、修改范围和完成条件，不建立第二套架构规则。采用预发布 flag day：Server、Client、数据库契约、生成接口与镜像配置作为同一版本交付；不支持旧 lock/unlock 与新前台选择混用。工作包是开发和审查边界，不是可以分别投放工位的独立版本。

## 1. 当前实现与实际差距

下表记录实施前 `c8379d0` 的真实调用链差距；R0 完成范围见 §3.1，不以文档中的目标状态推断后续运行能力已经具备。

| 代码落点 | 当前行为 | 本次迁移内容 |
| --- | --- | --- |
| [Session Proto](../crates/device-protocol/proto/device_control_session.proto) | `lock_state`、Active/Locked；Actual 只有生命周期和 terminate 完成 epoch | 前台目标、Running、实际前台与两边显示就绪；保留 epoch 语义 |
| [Server Session Component](../server/src/component/session.rs)、[DB 访问](../server/src/component/session/db.rs)、[Target 编码](../server/src/device_control/state.rs) | 持久化 locked/unlocked，默认 unlocked，编码旧枚举 | 统一改名，默认 contest，仍由 Session Component 独占目标事务 |
| [Server convergence](../server/src/device_control/convergence/session.rs) | unlocked 用“不是 Locked”判断，可把 None/Starting 判为已收敛 | 必须验证真实前台、显示就绪和依赖；不能靠枚举改名继承旧判断 |
| [Helper Session](../client/privileged-helper/src/session.rs) | 只找 contest；后台 contest 被判 Ambiguous；调用 logind Lock/Unlock | 两个固定角色独立识别、精确激活、GDM 登录与故障恢复 |
| [Helper Home 窗口](../client/privileged-helper/src/home/window.rs)、[启动入口](../client/privileged-helper/src/main.rs) | v1 窗口含 `restart_display`；停止整个 display manager；Home 恢复成功才发布服务就绪 | contest 专用门禁和新窗口；拆开服务可用、Home 许可与图形启动 |
| [SnapshotReconciler](../client/device-daemon/src/reconcile.rs) | 先 Session、后 Home，再重采样；访问要求 Active/Locked 和精确 epoch | reset 优先于比赛登录/呈现；普通切换与 Home 维护分支明确 |
| [Session 收敛](../client/device-daemon/src/reconcile/session.rs)、[Home 收敛](../client/device-daemon/src/reconcile/home.rs) | 已有精确 terminate pending 和 durable completion；没有受控重新登录闭环 | 保留完成屏障，接入 GDM 准备和最新前台目标；reset 自身结束旧 contest |
| [Device1 / Binding](../client/device-daemon/src/reconcile/binding.rs) | 绑定 contest，注册只接受 GetSessionByPID 直接命中；后续方法没有统一绑定总线调用者连接 | waiting 身份、严格 GNOME scope 映射、所有 lease 操作的 caller 校验与独立输入资格 |
| [Agent 入口](../client/session-agent/src/main.rs)、[UI](../client/session-agent/src/ui.rs) | 读取 XDG session identity；未显示 Binding 或失联时 Hidden；窗口按需创建 | waiting 常驻静态全屏、首帧确认、失联保留占位和 GNOME 注册 |
| [Device1 policy](../packaging/client/rootfs/usr/share/dbus-1/system.d/org.natsume.Device1.conf)、[XDG entry](../packaging/client/rootfs/etc/xdg/autostart/org.natsume.SessionAgent.desktop) | policy 只允许 contest；entry 没有 AutoRestart | 权限迁到 waiting；配套运行身份限制、GNOME AutoRestart 和独立 dconf |
| [Client 包配置](../packaging/client/nfpm.yaml)、[package smoke](../packaging/ci-package-smoke.sh)、[VM runbook](../packaging/target-vm/home-reset.md) | 打包和测试要求全局 GDM Home 互锁；正式 Home 模板仍有 TODO | 替换旧断言、门禁和启动配置，交付正式模板及新语义验收 |

保留现有 OverlayFS generation、fsync/真实挂载验证、宿主 mount namespace 校验、设备身份隔离、Binding negotiation/submission epoch 和控制 lease。现有 [connection 调度器](../client/device-daemon/src/control/connection.rs)已经用受跟踪任务执行完整 Target 计划，并保留 current/queued plan；本次复用这些边界，不另建任务队列或通用资源框架。

## 2. 工作包与依赖

| 工作包 | 交付结果 | 依赖 | 对应 PRD 阶段 |
| --- | --- | --- | --- |
| R0：统一契约与回归基线 | 明确类型、事实来源和断言，固定新旧语义切换范围 | 当前架构 | B/C/D 的共同前置 |
| R1：Helper 会话与登录互锁 | 精确角色观测/激活、固定 GDM 客户端、正式 PAM 门禁 | R0 | A/B |
| R2：contest Home 维护与启动恢复 | 不停 GDM 的完整维护窗口、同 epoch 恢复和开机预备 | R1 | A/B |
| R3：waiting Agent 与 Binding | 原生全屏占位、可信注册/续约、首帧就绪及独立输入资格 | R0、R1；恢复联调依赖 R2 | C/D |
| R4：完整 Target 编排与 Panel | 普通切换、reset、terminate、离线及展示投影闭环 | R2、R3 | C/D |
| R5：正式交付与发行验收 | 配套 Deb/镜像、升级检查、全矩阵证据和 runbook | R1～R4 | E |

建议按 R0 → R1 → R2 → R3 → R4 → R5 实施。R2 先验证 Helper 维护与恢复机制，涉及正式 waiting UI/lease 的联合门槛在 R3 接入后验证；脚本占位或 mock 不能替代这部分验收。PAM、systemd 和包内权限配置随 R1～R3 一起开发和验证，不能留到最后才接入。共享类型与消费者发生编译联动时同批修改；不通过返回假成功、把 waiting 映射为旧 Lock 或永久保留两套接口来过渡。只有 R5 通过后才交付可部署版本。

## 3. R0：统一契约与回归基线

**改动范围：** Proto、`crates/local-control-api`、Server Session/HTTP/convergence 类型及测试夹具、Daemon Target 验证与本地 IPC 消费者、Web 生成类型的源定义。

1. 将目标统一为 `foreground_target=waiting|contest`；保留 `terminate_epoch` 和 `HomeTarget.reset_epoch`。操作继续使用现有 session-control PUT，不增加 login、toggle 或显示 Command。只有两个受管角色可以作为 Target，greeter/other/none/unknown 只允许出现在 Actual。
2. contest 生命周期使用 None/Starting/Running/Terminating/Ambiguous/Error；删除业务 Active/Locked。Helper 观测同时给出两个角色的精确 boot/session、桌面就绪、锁屏诊断及 seat0 前台。后台合法 contest 必须仍可识别。
3. Wire Actual 补充 `foreground`、`waiting_ready`；为 Server 判断比赛显示是否可用增加有直接消费者的 `contest_ready`。Running 只表示生命周期，不能独自证明桌面可交互。就绪为当前观测，不写入业务表；缺失、未知或互相矛盾的事实不得放行。最终字段与默认值约束由 Proto 和对应测试承载。
4. 本地 UI 保留 BindingPrompt/BindingPending，以静态 waiting 占位替代业务 Hidden；定义绑定当前 caller/lease/boot/session 的展示确认，携带首帧完成及实际尺寸。确认不等于激活完成，不能借此接受另一个 session 的展示结果。
5. 明确 Server convergence 的输入依赖：当前 Session Target、新鲜 Session Actual、Home Target/Actual 和当前 Binding 事实。未绑定阻塞 contest 呈现时展示“等待绑定”；Home 完成与桌面/前台完成分开展示。
6. Server 持久字段、组件方法、HTTP 请求/响应、OpenAPI operation/schema 命名、Target 编码、Web 操作及测试夹具统一迁移。默认 unlocked 对应新默认 contest；effective waiting 由本地条件推导，不写成第二份 Server 目标。保留现有管理员权限、幂等 PUT、Dirty 通知和事务提交后发布顺序。

**验证：** 加入能区分旧行为的测试：后台 contest 不歧义；None/Starting/未就绪不能满足 contest 目标；LockedHint 不能满足 waiting；未知目标拒绝；epoch 必须精确匹配；缺少展示确认不能得到 waiting_ready。类型变更后更新本地 D-Bus signature 测试及完整 snapshot 编解码测试，不能只改 UI 标签。

**完成门槛：** 所有新增事实都有明确生产者和消费者，目标样例与拒绝条件确定；R1～R4 复用同一契约。旧命名清理属于 R4 的切换门槛，最终只存在于迁移说明、拒绝测试和历史证据中。R0 完成不意味着运行行为已经完成。

### 3.1 R0 实施记录

本阶段统一了 Proto、本地 IPC、Server 持久字段/组件/Target/HTTP/OpenAPI、Daemon 和 Panel 的契约。默认目标为 contest，旧 `lock_state` 请求拒绝；terminate/reset 各自的 epoch、Session Component 事务及提交后 Dirty 通知保持原有所有权。Proto 直接按新契约重构，SessionState 连续编号，不为旧 Locked 保留 `reserved` 编号或名称；descriptor、Diesel schema 和 Web 类型从各自源定义生成。

| 事实或操作 | 生产者与消费者 | R0 落地及后续接入 |
| --- | --- | --- |
| `foreground_target`、`terminate_epoch` | Server Session Component → 完整 Target → Daemon Session | 只接受 waiting/contest；保留精确 terminate pending 和 durable completion，不追逐 replacement |
| 双角色生命周期、boot/session、`locked_hint` | Helper → Daemon Session / Device1 | 独立枚举 waiting/contest；唯一 seat0 X11 为 Running，后台会话不再判歧义；错误 seat、Wayland 或重复候选不视为缺席。R1 补齐 UID 与运行环境核对 |
| `desktop_ready`、实际 `foreground` | Helper → Daemon → Server convergence / Panel | 类型和消费者已接入；真实 GNOME/seat0 采样在 R1，当前明确返回 false/unknown |
| `contest_ready` | Daemon 合成 → 访问资格、Server convergence / Panel | 需 Running、精确身份、Helper 桌面就绪且未锁屏；不因后台而失效。当前真实 Helper 不提供桌面就绪，故不能开放访问 |
| `SessionPresentation`、`waiting_ready` | Agent 首帧证据 + Helper 系统事实 → Device1 → Daemon → Server / Panel | 定义 lease、boot/session、revision、首帧与实际宽高。R3 接入调用者认证和展示确认；当前确认入口明确拒绝，waiting_ready 固定 false，注册不能冒充已就绪 |
| Waiting UI snapshot | Daemon / Agent 本地回退 → 现有 Slint/Skia 窗口 | Hidden 改为黑色 Waiting，占位保持窗口、申请全屏、拒绝普通关闭；失联撤销 Binding 数据。全局 autostart 按真实本地用户筛选 waiting，GNOME 注册、AutoRestart、可信 scope 映射和尺寸验收仍属 R3 |
| Session convergence | 当前 Session/Home Target 与新鲜 Actual、Binding Target/Actual → HTTP / Panel | contest 必须满足真实前台、桌面就绪、精确 Home/terminate epoch 和 Binding；未绑定显示等待绑定。waiting 呈现独立于 contest/Home/Binding 健康，仍要求 waiting 就绪、真实前台及 terminate epoch 匹配 |

普通 Session 路径已删除旧 Lock/Unlock 调用，尚未执行前台激活；目标未满足时保留观测并重试。Home 仍使用旧的全局 GDM 维护互锁，R2 才替换。Device1 policy 仍属旧打包配置，R3 才迁到 waiting 并完成所有 lease 操作的 caller 校验。上述未完成接入均不能由当前 mock 或 UI 的 fullscreen 请求代替，**R0 是不可部署的中间版本**。

回归覆盖后台会话、意外 LockedHint、未知目标、旧请求、缺失/矛盾 readiness、精确完成 epoch、Home/Binding 依赖、UI 失联占位和跨设备请求隔离。本阶段不安装 VM 软件包，不改动 GDM/GNOME/PAM/镜像配置；正式系统验收随 R1～R5 进行。

本地检查记录（2026-09-07）：

| 检查 | 结果 |
| --- | --- |
| `just pre-commit` | 通过：工作区 Clippy、Web ESLint、426 项 Rust 测试及 12 项 Web 单测；原有 packaged Caddy 用例按 `#[ignore]` 跳过 1 项，本阶段未修改该用例 |
| 完整 Actual snapshot 编解码补充后的 Proto 定向测试及 Clippy | 7 项测试通过，descriptor golden 一致；补充后未改变运行代码 |
| `cargo fmt --all --check`、修改的 Web 文件 Prettier、`git diff --check` | 通过 |
| Schema / Diesel | 工作区测试已执行全部 6 项 schema-contract 测试；`just diesel-schema` 通过 |
| OpenAPI / TypeScript | 重新运行 export-openapi 与 api:generate，前后逐字节一致；OpenAPI lint 通过 |
| Web typecheck / build | 通过 |
| `pnpm --filter @natsume/web exec playwright test e2e/operator.spec.ts` | 13 项通过，含等待绑定展示、新请求体和跨设备 mutation 隔离 |

这些结果证明 R0 的类型、状态判定和界面消费者一致，不证明正式 GNOME 全屏、会话切换或 Home reset 流程已完成。

## 4. R1：Helper 会话能力、GDM 与 PAM

**改动范围：** `client/privileged-helper/src/session.rs`、`lib.rs`、`main.rs`，必要的 Helper 私有登录/门禁实现，`crates/local-control-api`，Client rootfs 中的固定启动入口、PAM、权限和包依赖。

### 4.1 观测和 mutation 所有权

- 只识别固定 waiting/contest UID、seat0、本地 user/X11 会话；按角色判唯一性，前后台独立观测。副作用前重新核对 boot/session，角色或身份变化时拒绝；显式 terminate 仍只追踪捕获的旧 contest。
- 激活通过 logind 精确会话 API；以再次观测实际前台确认结果。保留意外锁屏诊断，正常业务路径删除 Lock/Unlock 调用。
- 桌面就绪从真实 GDM/logind 和对应 GNOME 用户运行环境取得；`SessionOpened`、进程存在、固定 sleep 均不是充分条件。Daemon 将 Helper 的精确观测与正式 Agent 的当前展示确认合成为 waiting_ready；Helper 不拥有 UI lease，不在持有 mutation 排他权时回调 Device1 等待展示，避免相互等待。
- `PrivilegedService` 继续拥有本地 mutation 的串行边界；固定开机入口、GDM 登录入口、PAM 子进程和 Home 维护还必须共享跨进程门禁规则。不能只依靠当前 `&mut self` 对 D-Bus 方法的串行化。

### 4.2 固定 GDM 登录入口

将已验证的 GDM/libgdm API 流程实现为 root 安装、以固定 gdm 身份运行的单次客户端：创建或复用正确 greeter，等待就绪，选择 `ubuntu-xorg`，通过固定 `gdm-contest` 或故障恢复专用 `gdm-waiting` 服务认证，收到 SessionOpened 后请求启动。

调用方只能选择封闭业务能力，不传任意用户名、PAM service、session entry、unit、DISPLAY、Xauthority 或环境变量。Helper 保持单飞与结果查询；客户端退出后桌面归 GDM 管理。调用超时先重采样，存在正确会话就复用，不重复创建第三个会话。

正式交付不依赖 `/tmp` 下 Python 原型，不增加常驻登录 keeper。若使用 Helper 二进制提供一次性子命令，CLI 分派必须在长期服务初始化之前完成：PAM 门禁不要求启动 D-Bus 服务或取得 service 的 fd 3；只有真正拥有 Home 操作的服务/恢复入口执行原宿主 namespace 校验。不同模式不得借此获得任意 root 操作能力。

### 4.3 PAM 生命周期互锁

- 官方 pam_exec 调用固定 Helper 门禁子命令。针对允许的 contest 登录栈，在 auth/account/open_session 的共享短锁内校验本次 boot 的许可并原子登记精确 GDM worker 的 boot/PID/start-time。
- 登记保留至精确 worker 退出；pam_exec 返回、API 客户端退出或 close_session 本身不证明 worker 已消失。PID 重用必须通过启动时间辨别，损坏/未知记录按拒绝处理。
- Helper 关闭许可后取消并排空登录事务，再取得同一锁的排他权核对登记和真实运行环境。等待 GDM/PAM 回调时不能持有它们完成退出所需的锁，避免取消过程死锁。
- waiting/greeter 不受 contest 门禁阻塞。镜像关闭两个受管用户的其他登录入口及 linger；所有保留的 contest 入口都必须有等价门禁，不能仅验证自动登录路径。
- 公共 IPC 只暴露封闭能力；PAM 子命令校验执行身份与真实进程关系，不能信任自报 worker PID 或可伪造环境来建立许可。

**验证：** 角色匹配、旧 boot/session、重复角色、PID 重用/损坏登记的单元与 IPC 测试；当前 QEMU 内用正式入口分别暂停 auth/account/open_session，证明 logind 尚无 contest 时也不能进入 Home 修改区间。验证关闭门禁、客户端崩溃、取消超时和重复请求；GDM/waiting 实例保持不变。对应 AT-02、AT-10、AT-13～14、AT-21～22。

**完成门槛：** 正式代码和正式 PAM 配置共同证明无登录事务可跨入维护区间；普通激活不锁屏；不存在宽泛 root 参数或直接托管桌面的路径。

## 5. R2：Home 维护窗口和开机恢复

**改动范围：** `client/privileged-helper/src/home.rs`、`home/window.rs`、`lib.rs`、`main.rs`，Helper/启动 units、tmpfiles 和升级检查；OverlayFS 核心只修改与新窗口耦合的部分。

1. 新窗口显式版本化，记录 reset epoch、阶段及捕获的 contest boot/session，移除 `restart_display` 责任。正常 reset 的 Prepare 接受已有 contest：Daemon 先确认 waiting 可展示并激活，向 Helper 传递捕获的 waiting 精确身份；Helper 重查角色/boot/session/实际前台，再持久化窗口、关闭 contest 许可并结束旧运行环境。展示确认来自当前 lease，Helper 的系统观测不以调用者自报身份替代。
2. 排空精确登录 worker、contest session、UID 进程、用户 manager 和 Home 占用后，才进入现有 Prepare/Apply/Verify/Recover 文件及挂载步骤。只操作固定 contest；未知残留或超时保持门禁，不使用 force/lazy umount。
3. 保留 generation 的 durable marker、父目录 fsync、宿主 namespace fd 校验、模板验证及旧 generation 回收。Verify 必须基于本次启动的真实挂载，不能用历史 Verified 直接开门。
4. Home 的 durable completion 完成后才发布登录许可；GDM 登录失败不撤销已完成 Home，也不重新清空同一 generation。Helper 的 root 窗口和 Daemon 的 completed_reset_epoch 各自保持原所有权，重试先查询并补齐发布屏障。
5. 显式 terminate 的 pending 捕获仍归 Daemon Session completion；reset 窗口对旧 contest 的捕获归 Helper。二者同时出现时先完成或对齐已有精确 pending，结束同一个旧会话后分别确认各自条件；不得为了补另一个 epoch 再杀新 contest，也不得因 reset 发生就无条件推进 terminate epoch。
6. GDM 自动登录 waiting 与 Home 恢复分开启动。调整当前 `restore_home_before_ready`：Home 恢复失败应保留门禁、报告可诊断/可重试的恢复状态，不能让 waiting 依赖 Home 成功才能获得服务。宿主 namespace/身份等服务前置条件失败仍拒绝不安全的 Helper 启动；Agent 即使失联也能先显示本地占位。
7. 固定启动预备通过同一 Helper 排他边界等候实际 Home 和 waiting 就绪，再请求 GDM 建立 contest，最终回 waiting。启动预备的最终回 waiting 必须先于本次 boot 的首次业务呈现；业务计划等待预备完成后再按最新 Target 激活，避免迟到的启动动作覆盖比赛前台。不得恢复旧 lease 的业务呈现；健康 Helper 重启先观察已有运行环境，不重建健康桌面或重新 reset。
8. 与正式 PAM 门禁一起删除旧 [display-manager drop-in](../packaging/client/rootfs/usr/lib/systemd/system/display-manager.service.d/50-natsume-home.conf)及 Helper 的 StopUnit/StartUnit GDM 逻辑。新 boot 顺序不能保留全局 `ConditionPathExists=home-ready`，也不能先删除旧保护再部署新保护。

**验证：** 正式 Helper 的窗口写入、Prepared、Applied、Verified 各阶段崩溃和硬复位；模板缺失、磁盘/挂载失败、残留进程、同 epoch 重试和新 epoch 到达。核对健康重启保留双方 session/PID；reset 保留 waiting/Agent/GDM，Home generation 只按新 epoch 推进。对应 AT-05～09、AT-15～20、AT-25～26。

**完成门槛：** 没有 Server、Home 恢复失败时 waiting 仍能展示；修复后同 epoch 恢复；Home 已完成而 GDM 登录失败时不再次清理；正常 reset 全程不重启 GDM。

## 6. R3：waiting Agent、展示就绪与 Binding

**改动范围：** `client/session-agent/src/main.rs`、`src/ui.rs`、`ui/session_agent.slint`，`crates/local-control-api`，Daemon `reconcile/binding.rs` 和就绪观测，XDG/dconf/Device1 policy。

### 6.1 UI 先独立启动

- Agent 只在固定 waiting 用户中运行；非 waiting 的全局 autostart 安静退出，不打开窗口、不循环注册。XDG 环境只提供启动线索，不作为 Daemon 的鉴权依据。
- 启动即渲染无装饰全屏纯黑；镜像可提供单张本地 ICPC logo，等比居中、缺图回退黑色。窗口根布局跟随真实尺寸，阻止普通关闭。Binding 复用同一个窗口；离线、lease 过期、无有效 snapshot 时撤销输入并恢复占位。
- Agent 自己保持 GNOME RegisterClient 的用户总线连接；entry 配置 AutoRestart，不标 RequiredComponent、不增加 user service。首帧与实际尺寸经当前 lease 确认后才能报告 waiting_ready。
- 先注册/显示占位，再评估是否展示 Binding；注册不要求 contest 已就绪、已绑定、Caddy READY 或目标前台已收敛。后台 waiting 继续续约，前台变化仅撤销/重新评估输入资格。

### 6.2 Device1 鉴权与 Binding 资格

- 将 Device1 policy 从 contest 迁到 waiting。注册、续约、读 UI、展示确认及 SubmitBinding 都绑定真实总线调用者连接及精确 waiting boot/session/lease；获知 lease ID 的另一个连接不得复用。
- 总线取得 PID/UID。GetSessionByPID 成功必须精确命中 waiting；只有明确 NoSession 才使用 PRD §8.4 的严格映射：GetUserByPID、系统 GetUnitByPID、唯一会话及 User.Display 同时符合固定 waiting/seat0/X11。超时、权限错误、其他 session 不走降级路径。
- 分开维护展示就绪和输入许可。输入要求当前有效 plan、部署/未绑定、Home Steady、健康 contest、两个 completed epoch 精确匹配，以及 waiting 前台、未锁屏、Agent lease 有效。不能要求 contest 前台、Caddy READY 或 requested foreground 已完成，否则未绑定流程会循环等待。
- SubmitBinding 在提交边界重新核对当前资格、caller 与 lease；保留 negotiation ID、submission epoch、Seat 持久化先于 Input 发布的顺序。提交成功只等待 Server 确认，Agent 不自行切换会话。

### 6.3 有界 waiting 恢复

先由 GNOME 自动恢复 Agent。持续故障达到至少 60 秒冷却后，Helper 本次 boot 最多重建 waiting 一次；次数不得因 Daemon 重连、Helper 重启或 lease 更新清零。先撤销旧展示/Binding 资格，排空旧 waiting session、user manager 和 UID 进程，再通过固定 GDM 入口创建 replacement 并重新注册。再次失败保留展示错误；不结束 contest、不重启 GDM。

**验证：** 两种分辨率的真实全屏、尺寸切换、普通关闭、断 IPC、lease 过期、后台续约和前台重新输入；GNOME scope 的真实 Device1 注册；伪造 caller、错误 UID/session、其他连接复用 lease、waiting replacement 后旧进程/旧 lease、Binding submit 与 reset/切换竞态。断连时截图仍是占位，注册本身不能令 waiting_ready 成立。对应 AT-11～12、AT-19、AT-22～23。

**完成门槛：** 无 Binding/无 Server 时 waiting 也能显示；未绑定默认 contest 目标时仍能在 waiting 完成绑定；绑定成功后由 Daemon 按最新目标呈现；黑屏/无信号不能冒充首帧就绪。

## 7. R4：Daemon 编排与 Server/Panel 闭环

**改动范围：** `client/device-daemon/src/reconcile.rs`、`reconcile/session.rs`、`reconcile/home.rs`、`reconcile/binding.rs`、`control/connection.rs` 的必要接入及测试；Server Session Component、Target 编码、convergence、HTTP/OpenAPI；`web/src/pages/targets-page.tsx`、`devices-page.tsx`、生成类型和 E2E。

### 7.1 在现有 SnapshotReconciler 中明确执行顺序

| 触发条件 | 执行顺序 | 完成判据 |
| --- | --- | --- |
| 普通 waiting/contest 目标 | 观察唯一会话及依赖 → 计算有效前台 → 必要时精确激活 → 重采样 | 目标角色真实前台且显示就绪；双方 ID/PID/Home generation 不变 |
| 新 reset epoch | 禁止 Binding 输入并确认 Caddy BLOCKED → waiting 就绪并激活 → Helper 关闭登录/结束旧 contest/重置/Verify → durable Home completion → 允许 GDM 准备 contest → 按当前有效目标呈现 | Home 完成和 Session 呈现分别上报；waiting/GDM 保持实例 |
| 显式 terminate epoch | 关闭访问 → 持久化精确 pending → 结束捕获的 contest → durable terminate completion → 无 reset 时重新准备 contest → 当前目标呈现 | 旧 epoch 不追逐 replacement；不隐式清 Home |
| 本地已持有未完成维护窗口 | 继续/恢复同一窗口到安全完成点 → 重新取当前目标 → 决定后续登录与呈现 | 新 epoch 不覆盖旧窗口；失效计划不恢复 READY 或发起返回比赛 |

新 reset 存在时先处理维护，不能沿用“完整 Session 收敛后才 Home”的顺序，提前登录或返回 contest。普通切换只改变前台；若目标会话缺失，转入明确的准备/故障流程，不能把重新登录算成一次普通切换成功。

激活/登录/Home 每次副作用前检查当前 plan 和必要身份；调用超时按结果未知处理。Helper 继续串行维护，Daemon 等待旧任务并重采样后再落实新目标，不能将取消 token 当作撤销远端操作。长任务保持受跟踪、可查询与有界调用，不把 GDM/PAM 等待搬回 WSS heartbeat/deadline 循环内联执行。

### 7.2 分开访问、展示和输入

保留 `local_access_is_allowed` 的精确 Home/terminate epoch 和新鲜观测约束，将 Active/Locked 改为唯一 Running 且 contest_ready。Caddy 上游资格继续依赖 Gateway/Binding/Runtime；前台 waiting 本身不撤销 Binding，也不必重载 Caddy。

比赛呈现另要求当前有效 Target、已绑定、没有未完成 reset/terminate 和 contest 显示可用。开机尚无当前 Target 时回 waiting；比赛过程中失联沿用既有关闭访问和 lease 撤销规则，保留现有前台与健康进程，不因断网自动切屏或 reset。维护期间失联可安全完成已持有的本地窗口，但旧 plan 不能据此启动新的业务呈现。

周期观测只撤销资格，不授予 READY 或 Binding 输入；只有当前 Target 的收敛计划可以重新授予。维护后先重采样再开放，保留 BLOCKED 无法确认时的现有失败退出与 Caddy 停止处理。

### 7.3 Server 和 Panel 完成实际语义迁移

- Session Component 仍拥有前台 Target 与 terminate epoch 的事务；Home Component 仍独立拥有 reset epoch。两种完成互不代替，不增加跨组件的第二份业务操作状态。
- 同步修改唯一 initial migration 的目标列、Diesel 生成 schema、组件 DB 读写及测试；更新 `device_control/state.rs` 的完整 Target 编码。预发布测试数据库由新 schema 建立；需保留既有业务数据的部署在维护时一次性转换 locked→waiting、unlocked→contest 并保留 epoch，不能让运行时双读旧字段。
- convergence 同时消费前台、两边 readiness 和必要 Home/Binding 事实；设备离线或新 lease 尚未上报时维持 AwaitingActual，旧 Actual 不沿用。默认 contest 被 Binding 阻塞时显示等待绑定，不显示已收敛或会话故障。
- Panel 按当前语言风格呈现“显示等待界面 / 显示比赛桌面”，分别提交新的持续 Target；仍用原 terminate 和 Home reset 入口。展示请求目标、实际前台、Home 完成、桌面准备和失败原因；保留 reset 的删除数据确认与每设备 mutation 隔离。
- 经 export-openapi 和现有生成命令更新 OpenAPI/TypeScript，不手改生成文件；迁移请求 DTO、operation ID、响应及所有 mocks，旧 lock_state 请求明确拒绝，不作别名接受。

**验证：** 完整 Target 经 Server→Daemon→Helper→Actual→Panel 的回归；reset 中多次改前台目标、重复 epoch、同一 epoch 登录失败重试、reset/terminate 同时到达、lease 替换、旧调用晚到。观察 BLOCKED 先于破坏性操作，普通前台变化保留 Binding/Caddy 有效配置；回归跨设备 in-flight mutation 和阻塞外部调用时 heartbeat/deadline 仍运行。对应 AT-03～09、AT-19～22、AT-26。

**完成门槛：** 正式全链路完成两种普通切换、reset 后留 waiting、reset 后回 contest、延迟放行和无绑定启动；HTTP 成功与 Home Verified 都不被错误当作呈现完成。

## 8. R5：打包、升级与发行验收

**改动范围：** `packaging/client/nfpm.yaml`、rootfs、maintainer scripts、`packaging/ci-package-smoke.sh`、`packaging/target-vm`、[Agent 部署说明](../packaging/client/rootfs/usr/share/doc/natsume-client/session-agent-gui-startup.md)，以及 ICPC 镜像仓库对应配置与正式模板。

1. 明确交付归属：Client 包提供 Helper/Agent/固定 API 入口、IPC policy、root 状态目录与固定服务；镜像提供账户、官方 GDM/GNOME/X11、GDM/PAM/dconf 配置和版本化只读 Home 模板。两边安装出的入口与配置按同一版本校验，不由 Daemon 运行时改系统配置。
2. waiting 专用 environment.d/dconf profile 在用户运行环境建立前生效，避开镜像全局 ArcMenu 锁定；两个受管用户禁用自动/普通锁屏入口，waiting 另限制退出、空闲与菜单行为。应用官方 Xorg DontVTSwitch/DontZap 配置并核对受控激活仍有效。
3. 更新 nfpm 文件清单及实际 ELF/运行依赖，验证正式 GDM 客户端所需库。当前 rootfs/etc 并非整体打包，新增 PAM 等文件必须显式归属 Client 包或镜像构建；不能以“仓库里有文件”当作已交付。
4. 在安装替换旧 Helper/旧保护之前检查旧 v1 维护窗口；当前包只有 postinstall，需增加适当的安装前检查并接入 nfpm。发现进行中旧窗口即拒绝升级，由旧版本恢复结束后重试；新 Helper 也拒绝解释旧 `restart_display` 记录。保留设备身份及历史完成 epoch，不删除 root 窗口绕过检查。
5. 同步替换 package smoke 和 VM runbook 中“停止 display manager 才能 reset”“Agent 属于 contest”“失联隐藏窗口”等断言；验证 Home 失败不阻塞 waiting、无旧全局 GDM gate、PAM 配置可在正式软件包升级后继续生效。升级不在比赛中静默重启 GDM。
6. 维护时整体切换 Server/Client/协议/镜像配置；停止旧 lease，等待新版本重新上报事实。回退恢复兼容整套版本和必要数据库备份，先完成已持有 Home 维护，禁止只降级 Helper 或恢复旧 drop-in。

### 8.1 分层检查

| 检查层 | 执行入口/重点 | 证明范围 |
| --- | --- | --- |
| Rust 与静态检查 | 相关包定向测试、`cargo fmt --all --check`；集成后 `just pre-commit` | 类型、权限判定、epoch、取消和恢复分支；不能证明真实桌面行为 |
| 契约 | `just ci-contracts`：schema-contract、Diesel、OpenAPI/生成类型；补充本地 IPC 与 Proto 测试 | schema 与生产者/消费者一致，旧请求拒绝，生成产物无漂移 |
| Web | typecheck/build、更新后的 Operator E2E 和设备切换用例 | 新请求体、状态展示、权限及 mutation 隔离 |
| 包 | `just ci-packages` 及更新后的目标 VM package lifecycle | 真实 Deb 文件、依赖、模式/owner、安装前拒绝旧窗口和实际 systemd 配置 |
| QEMU/KVM | 使用当前 VM 的隔离快照，移除原型覆盖后安装正式包；另执行新镜像从零安装 | GDM/PAM/logind/OverlayFS、Agent caller、全屏和冷启动的正式闭环 |
| 目标硬件 | 每类投放 GPU/驱动组合，含键鼠、IME、缩放、休眠和真实断电 | QEMU 无法替代的显示、输入及持久化验收 |

### 8.2 正式 VM 验收顺序

先跑最小闭环：无 Target 冷启动 → waiting 占位 → 自动预备 contest → waiting Binding → 比赛 → waiting → reset 后等待 → 延迟返回比赛 → reset 后直接返回。全部使用正式 Server/Daemon/Helper/Agent 和真实 API，不由脚本直接改 marker 冒充成功。

然后按 PRD AT-01～26 执行故障矩阵，尤其是三个 PAM 暂停点、四个 Home crash-cut、旧 plan 晚到、waiting replacement、跨连接 lease 复用、旧窗口升级拒绝及 BLOCKED 失败处理。waiting 不可用时的常规 reset 必须在结束旧 contest 之前停止；已经进入维护的恢复责任则按持久窗口安全完成。

最后完成 100 轮普通切换、20 个独立 reset epoch、10 次启动，记录 PRD 的 P95 ≤ 3 秒普通切换、≤ 30 秒 contest 冷登录、≤ 120 秒 reset 目标及实际结果。历史原型次数不能抵扣正式验收；不得靠提前报告 readiness、跳过 fsync/挂载验证来满足性能目标。

每轮归档镜像/软件包版本、boot、双方 session/Xorg/Shell/Agent 身份、GDM InvocationID、Target 与完成 epoch、Home generation、实际前台和 readiness、阶段耗时、截图及必要日志。维护普通切换与 reset 的调用证据，证明没有业务 Lock/Unlock；数据和凭据目录不采集内容。证据随构建产物归档，不只保存在 `/tmp`。

**完成门槛：** PRD 正常及关键故障用例通过、正式模板交付、完整包安装/升级验证通过、至少一台目标硬件闭环通过后才制作发行镜像；所有投放 GPU/驱动类别完成验收后才能批准相应批次。更新主架构 WP9/WP10 的完成证据，不能只因代码合并而将发行验收标为完成。

## 9. 开始实施与审查口径

开始实施时先复核当前 HEAD 与本计划基线差异，再从 R0 的统一契约进入 R1 的固定能力实现。每个工作包提交说明应包含具体行为变化、修改边界、实际执行的检查及尚未覆盖的 VM/硬件条件；未完成项保持显式 TODO。

架构文档和 Caddy 模板已分别提交；Caddy 不属于双会话实现。R0 已通过审查；后续按工作包依次实施，每阶段完成后等待审查，不能据此将 R1～R5 或发行验收标为完成。
