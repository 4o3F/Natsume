# Windows 打印客户端

`natsume-print-client` 面向 Windows 10／11 x64 的已登录用户。便携 ZIP 构建与 CI 交付见[打包说明](../../packaging/README.md#windows-portable-packages)，不提供安装器。产品目标及业务规则以[架构第 23 节](../../docs/architecture.md#23-单赛事运营平台扩展)为准。

## 启动与操作

准备一个由当前 Windows 用户管理的 TOML 配置，服务器地址只接受 HTTPS origin，队列名必须与 Windows 已安装队列一致。配置和本地目录应由该用户及管理员控制，不在公共可写目录中运行，也不要为同一物理打印机启动多个不同配置的客户端。

```toml
server_url = "https://print.example.test"
queue_name = "Contest Printer"
state_directory = 'C:\Users\Contest\AppData\Local\NatsumePrint'
ca_certificate = 'C:\Users\Contest\AppData\Local\NatsumePrintCA.pem'
```

`ca_certificate` 可省略；指定时增加该 PEM 文件中的信任根，仍检查服务器证书和主机名。请求不使用系统代理、不跟随重定向，不提供跳过 TLS 校验选项。配置不包含配对码、客户端令牌或管理员会话。

```powershell
natsume-print-client.exe --config C:\Natsume\print-client.toml
```

指定 `--config` 时，窗口启动该已有配置；不指定时仅打开配置界面，默认配置路径为 `%LOCALAPPDATA%\NatsumePrint\client.toml`，默认新部署状态目录为其下的 `state`。界面可列出已安装 Windows 队列，并在明确点击后创建新配置；已有配置文件不会被覆盖。已有部署始终使用原配置与状态目录，不通过新建目录、删除记录或重新配对恢复任务。启动已有配置后，界面显示实际加载的 origin、队列、状态目录和 CA 路径。

界面固定使用浅色配色，页面正文、标准输入框和确认弹层共用同一调色板，不跟随系统切换为深色；无需修改 Windows 的主题设置。

需要配对时，在八格输入框中输入管理员签发的配对码，支持整码粘贴、自动前进、方向键和退格；输入不回显，发送后清空输入框。令牌保存到当前用户的 Windows `PasswordVault`，不会进入界面快照，本地记录只保存非秘密身份和执行状态。只有 Server 确认且本地已保存确认事实，配对才完成；中断配对不会自动恢复，未完成凭据会被清除，管理员需清理 Server 残留并重新签发码。凭据丢失或本地记录损坏时停止领取，不把重新配对当作恢复旧尝试的方法。

首次配对后默认暂停领取。窗口或托盘中的“恢复领取”恢复领取，“暂停领取”暂停新领取，“正常退出”在确认后退出。暂停只影响新领取，已经发出的领取请求仍需核实，当前打印、观察和结果通知继续进行；暂停状态跨重启保留。界面显示本地阶段、任务号、尝试号和 Windows Job ID。

工作台将领取开关与当前任务分开展示，允许暂停与当前任务继续处理同时存在。任务展示准备、提交、观察和报告四个阶段；报告失败或未知结果不会显示 `PRINTED`。设置弹层只读显示实际配置，修改配置前应先正常退出。需要人工核实时可打开 Windows 原队列或复制不含凭据、配对码和文档内容的诊断摘要；摘要包含已知任务、尝试、Job ID 及原作业标签。

托盘创建成功后，关闭窗口只隐藏界面，托盘的“打开主窗口”可重新打开；托盘不可用时，关闭窗口打开正常退出确认，不留下不可见的领取进程。正常退出不再启动原生提交，并等待已经运行的原生提交返回，保留尚未结束的观察或通知供下次启动恢复。驱动调用可能阻塞，不能保证立即退出。当前保留控制台承载普通 tracing 输出；请使用窗口或托盘正常退出，不关闭该控制台或强行终止进程。日志保存、控制台隐藏、安装及登录启动策略待确定。

## 本地记录与恢复

`state_directory` 中的 `client.lock` 提供同一目录的单实例锁，`state.jsonl` 是仅追加的状态记录。每次阶段变化和暂停操作均写入递增序号、完整状态和 SHA-256 校验后同步文件；领取请求 ID 先于请求保存，Windows Job ID 先于 Server 登记和第一页保存，最终结果先于通知保存。记录不包含令牌、源码或 PDF 内容。校验用于发现意外损坏，恶意修改防护依赖目录访问控制。

读取记录时检查完整历史及尾部，不忽略半条记录、不截断损坏内容，也不回退到较早的空闲状态。文件丢失、损坏、身份不一致或中断提交没有准确 Job ID 时停止领取并提示处理。不要删除记录后假装空闲；现场恢复必须先核实 Server 原尝试、Windows 原作业和绑定归属。当前不提供自动修复或历史压缩；长期运行需监控记录文件的磁盘占用。

重新启动后，有准确 Job ID 的记录只恢复原作业的精确观察和结果补报，不再调用打印提交。已发出的领取请求只以原请求 ID 核实，结果通知丢失只补报原结果，不重新打印。任务消失、删除或标签不匹配均记录为结果未知，`COMPLETE` 不作为成功；只有 `PRINTED` 才生成待报告的完成结果。Server 返回原终态后才清除当前本地工作，不用迟到结果修改后续尝试。

## 后端边界

渲染使用操作系统自带的 `Windows.Data.Pdf`，打印使用 GDI 和现场队列驱动，不依赖额外下载的 PDF 运行库或 RAW PDF 打印能力。输入按普通文件读取，最多 8 MiB；同一份不可变字节用于本次渲染，文件随后被替换不会改变打印内容。

未加密的 A4 PDF 按页渲染为约 300 DPI 的灰度位图，并整体缩放到可打印区域。解码时将位图转换为固定物理像素尺寸：纵向为 2480 × 3508，横向为 3508 × 2480。驱动设置请求并复查 A4、黑白、单份和固定缩放；单双面保留现场队列配置。只有队列未报告故障、未设置离线、且没有未完成作业时才允许领取和提交。队列总任务数与未完成作业数分开记录；明确为 `PRINTED` 且没有处理中、故障、删除或未知状态的已完成作业可以保留，不占活动容量。仅为 `COMPLETE` 或仅有保留标记不代表完成，查询失败或无法完整读取作业时不允许领取或提交。容量检查不删除任何作业。

`submit` 将 `StartDocW` 直接返回的 Job ID 交给调用者提供的同步回调，回调成功后才开始提交页面。调用者负责按实际领取协议保存尝试身份；后端不维护独立诊断日志、重试队列或恢复状态。回调或后续操作失败时，后端尽力中止未结束的文档，并释放设备上下文和打印机句柄。

任务观测同时核对所选队列、Job ID 与 `Natsume/<attempt_id>` 标签。`SentToDevice` 仅表示 Windows 已交给设备，`SpoolerReportedPrinted` 仅表示 spooler 报告打印；任务消失、删除或标签不匹配均不能归类为成功。原生调用可能阻塞，后端未设置驱动执行超时，也未隔离驱动进程。

手写 `unsafe` 仅允许出现在 `src/backend/` 内，用于必要的 Windows 原生接口；其余模块继续遵守 workspace 的禁止规则。

## 构建与测试

使用仓库 `rust-toolchain.toml` 指定的 Rust，以及不低于 `.deno-version` 的同一主版本稳定版 Deno；当前 Deno 基线为 2.9.8，同一主版本的更新稳定版不要求降级。在安装了对应 MSVC 构建工具和 Windows SDK 的 Windows x64 原生 PowerShell 中，从仓库根目录运行，不使用 WSL 的 Linux 工具链：

```powershell
deno task prepare rust
if ($LASTEXITCODE -ne 0) { throw 'Dependency preparation failed; do not continue.' }

cargo clippy --frozen -p natsume-print-client --all-targets -- -D warnings
if ($LASTEXITCODE -ne 0) { throw 'Clippy failed; do not continue.' }

cargo test --frozen -p natsume-print-client
```

PowerShell 示例显式检查外部程序退出码；准备失败时须先处理报告的问题，再运行离线命令。版本错误显示当前版本、最低版本和 Deno 路径；Rust 下载失败保留 Cargo 输出，须核对镜像、离线设置和网络，保留锁文件及依赖版本。准备阶段下载 Rust 依赖，后续命令通过 `--frozen` 使用已有输入；Windows 准备不下载 Linux Skia 库。持续集成在工作流中直接声明 GitHub Actions 原生模块矩阵，Server、工位 Client 和 DOMjudge 接入包分别在 Ubuntu 运行，打印客户端在 Windows 项中依次运行统一的准备、检查、测试、release 构建和便携 ZIP 打包入口，不构建依赖 D-Bus／Wayland 的 Linux 包，不下载 nFPM 或 Linux 工位资产。ZIP 经解压和逐文件摘要核对后上传为 CI 产物；标签 Release 在全部检查通过后将其与 Debian 包一同发布。

Linux 可运行 `cargo test --frozen -p natsume-print-client` 验证不可变输入、尝试身份、整页布局、容量判断和共享界面控制器。交叉运行 `cargo clippy --frozen -p natsume-print-client --target x86_64-pc-windows-msvc --all-targets -- -D warnings` 除 Windows Rust 标准库外，还需要能编译 TLS 等原生依赖的 Windows C 工具链和 SDK。

Windows 上的 `cargo test` 还会实际调用系统 PDF 渲染器，使用 `tests/fixtures/a4.pdf` 验证纵向和横向 A4、输出尺寸及灰度转换。常规自动化测试不提交打印任务，不需要真实打印机，也不会出纸。

执行核心已通过 Windows 原生测试和 Clippy，并完成 Adobe PDF 单任务、连续双任务软件联调。此前 Windows 图形版本已确认编译、启动、窗口调整、队列列表、托盘行为及浅色文字显示正常。中文输入、文件选择、配置加载、长路径、显示缩放、系统快捷操作和界面内打印执行待验证。

Linux 单元测试使用本地 HTTPS 服务、打印与凭据替身及无桌面显示的软件窗口替身，覆盖执行流程、配对中断、回复丢失、结果补报、重启只观察、暂停、记录损坏、配置不覆盖、退出请求及界面渲染与交互。网络与中断恢复按单元测试验收。真实打印机的驱动、纸张、缩放、故障观测及出纸待验收。
