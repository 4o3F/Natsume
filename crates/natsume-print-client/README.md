# Windows 打印客户端

`natsume-print-client` 面向 Windows 10／11 x64、已登录用户会话，包含正常应用入口、Server 配对与执行协议、本地任务记录和原生打印后端。本轮提供控制台入口，不包含完整图形界面、托盘或安装包；尚未取得 Windows 程序运行、凭据存储和真实打印的验收证据。完整产品目标及业务规则以[架构第 23 节](../../docs/architecture.md#23-单赛事运营平台扩展)为准。

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

首次启动提示输入管理员签发的配对码，输入不回显。令牌保存到当前用户的 Windows `PasswordVault`，本地记录只保存非秘密身份和执行状态。只有 Server 确认且本地已保存确认事实，配对才完成；中断配对不会自动恢复，未完成凭据会被清除，管理员需清理 Server 残留并重新签发码。凭据丢失或本地记录损坏时停止领取，不把重新配对当作恢复旧尝试的方法。

首次配对后默认暂停领取。在控制台输入以下单个字符并按回车：`r` 恢复领取，`p` 暂停新领取，`q` 正常退出。暂停只影响新领取，已经发出的领取请求仍需核实，当前打印、观察和结果通知继续进行；暂停状态跨重启保留。正常退出不再启动原生提交，并等待已经运行的原生提交返回，保留尚未结束的观察或通知供下次启动恢复。驱动调用可能阻塞，不能保证立即退出，也不能通过强行退出证明没有出纸。

## 本地记录与恢复

`state_directory` 中的 `client.lock` 提供同一目录的单实例锁，`state.jsonl` 是仅追加的状态记录。每次阶段变化和暂停操作均写入递增序号、完整状态和 SHA-256 校验后同步文件；领取请求 ID 先于请求保存，Windows Job ID 先于 Server 登记和第一页保存，最终结果先于通知保存。记录不包含令牌、源码或 PDF 内容。校验用于发现意外损坏，不代替目录访问控制，也不是对恶意修改的认证。

读取记录时检查完整历史及尾部，不忽略半条记录、不截断损坏内容，也不回退到较早的空闲状态。文件丢失、损坏、身份不一致或中断提交没有准确 Job ID 时停止领取并提示处理。不要删除记录后假装空闲；现场恢复必须先核实 Server 原尝试、Windows 原作业和绑定归属。当前不提供自动修复、历史压缩、手动重印或重置次数入口；长期运行需监控记录文件的磁盘占用。

重新启动后，有准确 Job ID 的记录只恢复原作业的精确观察和结果补报，不再调用打印提交。已发出的领取请求只以原请求 ID 核实，结果通知丢失只补报原结果，不重新打印。任务消失、删除或标签不匹配均记录为结果未知，`COMPLETE` 不作为成功；只有 `PRINTED` 才生成待报告的完成结果。Server 返回原终态后才清除当前本地工作，不用迟到结果修改后续尝试。

## 后端边界

渲染使用操作系统自带的 `Windows.Data.Pdf`，打印使用 GDI 和现场队列驱动，不依赖额外下载的 PDF 运行库或 RAW PDF 打印能力。输入按普通文件读取，最多 8 MiB；同一份不可变字节用于本次渲染，文件随后被替换不会改变打印内容。这些检查不代替 Server 的完整受理校验。

未加密的 A4 PDF 按页渲染为约 300 DPI 的灰度位图，并整体缩放到可打印区域。解码时将位图转换为固定物理像素尺寸：纵向为 2480 × 3508，横向为 3508 × 2480，不把 Windows 显示缩放下的逻辑尺寸直接当作最终像素尺寸。驱动设置请求并复查 A4、黑白、单份和固定缩放；单双面保留现场队列配置。只有队列未报告故障、未设置离线、且没有未完成作业时才允许领取和提交。队列总任务数与未完成作业数分开记录；明确为 `PRINTED` 且没有处理中、故障、删除或未知状态的已完成作业可以保留，不占活动容量。仅为 `COMPLETE` 或仅有保留标记不代表完成，查询失败或无法完整读取作业时不允许领取或提交。容量检查不删除任何作业，也不是物理就绪证明或对其他进程的容量预留。

`submit` 将 `StartDocW` 直接返回的 Job ID 交给调用者提供的同步回调，回调成功后才开始提交页面。调用者负责按实际领取协议保存尝试身份；后端不维护独立诊断日志、重试队列或恢复状态。回调或后续操作失败时，后端尽力中止未结束的文档，并释放设备上下文和打印机句柄。

任务观测同时核对所选队列、Job ID 与 `Natsume/<attempt_id>` 标签。`SentToDevice` 仅表示 Windows 已交给设备，`SpoolerReportedPrinted` 仅表示 spooler 报告打印；任务消失、删除或标签不匹配均不能归类为成功。驱动可能已输出部分页面，提交失败或中止也不证明没有出纸。原生调用可能阻塞，后端没有驱动执行超时、进程隔离或绝不重复打印保证。

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

Windows PowerShell 不会因外部程序返回非零退出码而自动停止，因此示例显式检查前一步是否成功。准备失败时先处理脚本报告的问题，不继续运行离线命令。版本错误会显示当前版本、最低版本和 Deno 执行文件路径；Rust 下载失败会保留 Cargo 原始输出，并提示检查镜像、离线设置和网络，不修改锁文件或改用预发布依赖来绕过错误。准备阶段允许下载 Rust 依赖；后面的命令使用 `--frozen`，不会隐式安装输入。Windows 的 Rust 准备不下载 Linux 工位的 Skia 库。持续集成中的 Windows 作业只选择该包进行编译检查和测试，不构建依赖 D-Bus／Wayland 的 Linux 包，也不发布独立工具。

Linux 可运行 `cargo test --frozen -p natsume-print-client` 验证不可变输入、尝试身份、整页布局和保守容量判断。准备好 Windows 标准库后，可运行 `cargo clippy --frozen -p natsume-print-client --target x86_64-pc-windows-msvc --all-targets -- -D warnings` 检查 Windows 代码和测试；这不会链接或运行 Windows 程序。

Windows 上的 `cargo test` 还会实际调用系统 PDF 渲染器，使用 `tests/fixtures/a4.pdf` 验证纵向和横向 A4、输出尺寸及灰度转换。常规自动化测试不提交打印任务，不需要真实打印机，也不会出纸。托管 Windows runner 不代替 Windows 10／11 桌面和实际驱动验收。

本轮 `cargo test --frozen -p natsume-print-client` 在 Linux 上通过 36 项测试，覆盖模拟 HTTPS 服务下的执行流程、配对中断及凭据清理失败、领取与登记回复丢失、结果补报、重启只观察、暂停、精确身份、记录损坏及写入失败；`cargo clippy --frozen -p natsume-print-client --all-targets -- -D warnings` 通过，`prek run --all-files` 的七项检查也全部通过。打印使用测试替身，不触发真实设备。Linux 到 MSVC 的交叉检查因缺少 Windows 原生 C 编译工具及 SDK，在 TLS 的 `ring`／`aws-lc-sys` 构建阶段失败，尚未完成本轮 Windows 应用代码的编译验证；不能用 Linux 测试代替这项结果。

完整客户端交付前仍需验证 Windows 编译及运行、`PasswordVault` 配对失败清理、断电与重启时的记录可靠性、驱动设置、纸张和缩放、准确 Job ID、忙闲及故障观测、网络恢复和真实出纸。当前代码和 Linux 测试不代表这些能力已经完成现场验收。
