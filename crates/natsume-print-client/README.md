# Windows 打印后端

`natsume-print-client` 当前提供客户端内部使用的打印后端库，目标为 Windows 10／11 x64、已登录用户会话。库包含队列枚举、队列状态读取、PDF 渲染、打印提交，以及按准确 Job ID 和尝试标签观测任务。目前没有可执行入口、托盘界面、Server 注册、任务领取或恢复流程。完整产品目标以[架构第 23 节](../../docs/architecture.md#23-单赛事运营平台扩展)为准。

## 后端边界

渲染使用操作系统自带的 `Windows.Data.Pdf`，打印使用 GDI 和现场队列驱动，不依赖额外下载的 PDF 运行库或 RAW PDF 打印能力。输入按普通文件读取，最多 8 MiB；同一份不可变字节用于本次渲染，文件随后被替换不会改变打印内容。这些检查不代替 Server 的完整受理校验。

未加密的 A4 PDF 按页渲染为约 300 DPI 的灰度位图，并整体缩放到可打印区域。驱动设置请求并复查 A4、黑白、单份和固定缩放；单双面保留现场队列配置。只有队列报告的状态和任务数均为零、且未设置离线时才允许提交；这不是物理就绪证明，也不是对其他进程的容量预留。

`submit` 将 `StartDocW` 直接返回的 Job ID 交给调用者提供的同步回调，回调成功后才开始提交页面。调用者负责按实际领取协议保存尝试身份；后端不维护独立诊断日志、重试队列或恢复状态。回调或后续操作失败时，后端尽力中止未结束的文档，并释放设备上下文和打印机句柄。

任务观测同时核对所选队列、Job ID 与 `Natsume/<attempt_id>` 标签。`SentToDevice` 仅表示 Windows 已交给设备，`SpoolerReportedPrinted` 仅表示 spooler 报告打印；任务消失、删除或标签不匹配均不能归类为成功。驱动可能已输出部分页面，提交失败或中止也不证明没有出纸。原生调用可能阻塞，后端没有驱动执行超时、进程隔离或绝不重复打印保证。

手写 `unsafe` 仅允许出现在 `src/backend/` 内，用于必要的 Windows 原生接口；其余模块继续遵守 workspace 的禁止规则。

## 构建与测试

使用仓库 `rust-toolchain.toml` 和 `.deno-version` 指定的版本。在安装了对应 MSVC 构建工具的 Windows x64 环境，从仓库根目录运行：

```powershell
deno task prepare rust
cargo clippy --frozen -p natsume-print-client --all-targets -- -D warnings
cargo test --frozen -p natsume-print-client
```

准备阶段允许下载 Rust 依赖；后面的命令使用 `--frozen`，不会隐式安装输入。Windows 的 Rust 准备不下载 Linux 工位的 Skia 库。持续集成中的 Windows 作业只选择该包进行编译检查和测试，不构建依赖 D-Bus／Wayland 的 Linux 包，也不发布独立工具。

Linux 可运行 `cargo test --frozen -p natsume-print-client` 验证不可变输入、尝试身份、整页布局和保守容量判断。准备好 Windows 标准库后，可运行 `cargo clippy --frozen -p natsume-print-client --target x86_64-pc-windows-msvc --all-targets -- -D warnings` 检查 Windows 代码和测试；这不会链接或运行 Windows 程序。

Windows 上的 `cargo test` 还会实际调用系统 PDF 渲染器，使用 `tests/fixtures/a4.pdf` 验证纵向和横向 A4、输出尺寸及灰度转换。常规自动化测试不提交打印任务，不需要真实打印机，也不会出纸。托管 Windows runner 不代替 Windows 10／11 桌面和实际驱动验收。

完整客户端交付前仍需验证实际驱动的设置接受情况、纸张和缩放、准确 Job ID、忙闲与故障观测，以及领取协议中的尝试预算、旧结果失效和重启恢复。当前后端库不代表这些能力已经完成。
