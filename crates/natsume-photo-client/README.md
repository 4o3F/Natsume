# Windows 摄影客户端

`natsume-photo-client` 面向 Windows 摄影工作台，Rust 包位于 [`backend/`](backend/)，Slint 界面和窗口适配位于 [`ui/`](ui/)。不带子命令运行时打开摄影工作台；后端提供本地工作区、队伍拍摄组、自动收图、确认成品及离线 ZIP 导出。产品目标以[架构第 23 节](../../docs/architecture.md#23-单赛事运营平台扩展)为准；Windows 便携包的构建入口见[打包说明](../../packaging/README.md#windows-portable-packages)。

## 当前能力

| 功能 | 当前行为 |
|---|---|
| 非秘密名单 | 从授权 Server 下载并验证队伍身份和中英文资料，拒绝密码、座位及未知字段 |
| 本地工作区 | 创建/打开 SQLite 工作区，保存名单、格式版本及资料完成标记 |
| 展示资源 | 缓存学校校徽，按内容哈希去重，打开时校验 |
| 拍摄组 | 选队与开始组分开，组内锁定队伍，未结束的已知任务阻止关闭组 |
| 自动收图 | 原生文件通知加后台自动对账，处理源版本变化、重复发现和故障重试 |
| 源副本 | 只复制、不移动或删除源照片；不确定归属留待人工核对 |
| 候选图片处理 | JPEG/PNG 哈希及完整解码校验、EXIF 方向、RGB ICC → sRGB、原尺寸规范副本和缩略图；当前组快照区分通过与拒绝 |
| 候选选择 | 优先 EXIF 原始拍摄时间，回退源修改时间；稳定倒序、自动最新、手动保持及显式恢复自动模式 |
| 呈现协议 | 绑定窗口实例、组、队伍、候选、哈希和呈现修订；拒绝旧回执，确认准备不写成品 |
| 确认成品 | 重验确认意图与规范文件，完整发布后事务提交每队唯一关联；重复确认幂等，补拍失败保留旧成品 |
| 成果契约 | 共享 team_results 清单、JCS + Ed25519 签名及可信上下文验签；原授权密钥签名、固定读取快照、重试清单身份与流式单 ZIP 导出 |

原生 UI 已接入初始化、会话准备、队伍检索、候选选择、独立选手展示、口头确认、重拍、完成清理、待归组、本地成品查看和 ZIP 导出。Skia/WGPU 的 `AfterRendering` 回调发送准确帧回执，文件加载成功不代替展示。`snapshot.candidates()` 可读取当前组按拍摄时间排序的候选，`snapshot.selection` 表达自动或手动选择；选中候选不代表已展示或确认成品。

正式首次初始化已支持从 Server 下载精简名单和校徽，校验后保存到本地工作区。JSON 读取只用于内部准备和测试夹具，正式 CLI 不提供本地 JSON 初始化入口。没有照片时展示留空，不配置默认图片。

仅首次初始化联网；日常启动读取本地工作区，拍摄、重拍、确认和导出按离线流程实现，不每次重新下载。Server 来源授权与下载契约见 [Server README](../natsume-server/README.md#photo-initialization)。Client 已接入配对、Windows 凭据库、HTTPS 下载、完成回执及重试恢复；成果 ZIP 导出已实现；摄影 UI 已接入，Server ZIP 接收仍待实现。

## 启动与操作

以下命令在仓库根目录的 Windows 原生 PowerShell 中执行。使用仓库 `rust-toolchain.toml` 指定的 Rust、对应 MSVC 构建工具和 Windows SDK。Windows 内置编译 SQLite，不要求安装数据库服务。

```powershell
cargo build -p natsume-photo-client --locked
if ($LASTEXITCODE -ne 0) { throw 'Build failed; do not continue.' }

cargo run -p natsume-photo-client --locked -- --help
```

不带参数启动 Slint 工作台：

```powershell
cargo run -p natsume-photo-client --locked
```

界面使用原稿的米白、墨黑、信号红及直角排版，使用 Noto Sans SC 和 Cascadia Mono 系统字体；未安装时由系统回退。首次选择目标工作区并使用 HTTPS 与配对码初始化，以后选择原工作区并点击“打开已有工作区”离线进入。会话准备要求空的外部热文件夹；双屏选择不同的操作屏和展示屏，单屏需确认“选择单屏启动”。选手窗口无队伍时为空；有队伍无照片时只展示队伍资料，不显示等待页。

按“已核对，开始本队拍摄”后接收新照片，点击缩略图会切换并保持展示；可以恢复自动最新。选手窗口真实绘制当前图、操作员勾选口头满意后，才可确认。完成本队后清理本地候选，热文件夹原图保留。重拍和放弃需要二次确认。关闭主窗口会等待已开始的文件与导出任务收尾。当前 UI 在导出期间禁止其他人工操作，后台收图仍继续。模板迁移与差异见 [PORTING.md](ui/PORTING.md)。

保留三个 CLI 子命令：

| 命令 | 操作 |
|---|---|
| `initialize <path> --server <HTTPS origin> [--ca-certificate <PEM path>]` | 正式首次初始化，或恢复同一目标的未完成初始化；Windows 凭据库保存秘密 |
| `doctor <path>` | 只读离线校验工作区、名单、校徽、授权上下文及签名密钥，不创建或迁移数据库 |
| `export <path> <output.zip>` | 离线导出当前成品及签名清单到一个 ZIP |

正式初始化示例：管理员先通过 Photo 管理 API 创建来源并生成一次性配对码（当前未提供 Web 管理页面），然后在 Windows PowerShell 中运行：

```powershell
cargo run -p natsume-photo-client --locked -- initialize C:\PhotoData\Contest --server https://natsume.example:8443 --ca-certificate C:\PhotoData\server-ca.pem
```

`C:\PhotoData` 必须已存在，目标 `Contest` 尚不存在。首次执行会提示输入隐藏的配对码，不把代码作为命令行参数。使用操作系统已信任的证书时可省略 `--ca-certificate`，不提供跳过 TLS 校验选项。拒绝 HTTP、带用户名／密码或业务路径的 URL、HTTP 重定向和超限响应。

Client 在同级 `.Contest.photo-init` 目录记录非秘密恢复日志、暂存校徽与待发布工作区。Ed25519 签名种子和临时 token 放在当前 Windows 用户的凭据库中，不写入 JSON／SQLite。必须保留恢复目录，并使用同一 Windows 用户恢复；仅复制工作区不能复制凭据库中的签名能力。

中断后重新执行同一命令：已有绑定不会重新配对；下载失败可重新确认并下载同一固定快照；本地数据保存成功后若完成响应丢失，只重放同一个完成请求，不重新下载。收到完成回执后才发布正式工作区，普通 `doctor`／工作区打开不联网。已完成后再次执行 `initialize` 也只校验本地资料，不读取 CA 文件或请求 Server。

配对响应与凭据都未保存时，管理员需明确替换 pending enrollment 再提供新码；若签名密钥已丢失，Client 拒绝自动生成替代密钥。已确认但未完成且凭据过期，或尚未完成的快照名单错误时，管理员可在同一来源显式替换绑定并发新码；已完成来源禁止替换。替换后用新目标路径重新 initialize，原失败目录保留，不自动改绑旧日志或覆盖已完成工作区。失败留下的暂存目录保留供排查，不自动删除已有文件或覆盖已有工作区。Windows 使用文件同步和本地重命名发布，不宣称父目录 fsync 的断电落盘保证。

正式工作区另存 `initialization.json`（原始下载快照）和 `authorization.json`（非秘密来源、公钥及上下文）。授权资料不写入名单表；名单和队伍—校徽关系仍使用原有工作区表，确认成品使用独立关联表。

本地练习通过 `cargo test -p natsume-photo-client` 使用临时目录和 [两队测试名单](backend/tests/fixtures/roster.json)，不留下正式工作区。

工作区位置由调用者指定，必须使用绝对路径，父目录已存在，创建目标尚不存在。示例的 `target` 仅用于练习；正式摄影数据应放在独立的数据目录。创建失败保留不完整目录供检查，不自动删除、覆盖或重建已有工作区。

名单是 JSON 数组，每项包含 `account_id`、`account`、`organization_id`、学校与队伍中英文名称。`account_id` 是内部稳定身份，`account` 是外部 Team ID；两者分别不能为空且唯一。英文名可省略并默认为空字符串，不接受 null。身份比较保留原值，不按名称、座位或列表行号猜测队伍。

仅私有测试夹具使用以下清单；生产初始化直接消费已验证的类型化下载资料，例如清单目录中有 `logos/school.png`：

```json
{
  "school_logos": [{ "organization_id": 1, "path": "logos/school.png" }]
}
```

路径相对清单所在目录，禁止绝对路径、`..` 和链接越界。学校 ID 必须出现在名单中，不能重复配置；校徽列表可为空。展示资源只接受真实 PNG/JPEG，每张最多 16 MiB、宽高各不超过 4096 像素、解码分配上限 64 MiB。未配置返回空引用；已配置但缺失或损坏时拒绝打开，不静默忽略。

## 本地数据与重启

```text
工作区/
├─ workspace.sqlite3
├─ resources/                      校徽缓存
├─ captures/<session_id>/
│  ├─ <sha256>.source              原始拍摄源副本
│  └─ <photo_id>.candidate/
│     ├─ image.jpg 或 image.png    原尺寸、已应用方向的 sRGB 候选
│     └─ thumbnail.png            最长边 512 像素的缩略图
└─ finals/<sha256>.jpg 或 .png    数据库当前成品引用的完整文件
```

当前有 `photo_roster`、`photo_workspace`、`photo_team_results` 三张业务表，另有 Diesel 迁移记录。新表按 account_id 每队保存一条当前成品，包含稳定确认/成果/提交身份、文件路径/哈希/长度/格式/尺寸和递增修订；不保存未确认候选或图片 BLOB。`format_version` 仍为保留元信息，`ready` 仅表示本地资料完整提交，不代表正式授权或拍摄准入。新表由创建工作区时的新增 migration 安装，不改原有表；开发工作区按约定重建，不实现旧库兼容或自动迁移，打开时检查当前成品文件。本轮没有删除、覆盖或自动重建任何已有工作区。

会话、活动组、任务、源版本、归档观察的归属和待核对列表只在内存中，不恢复未完成拍摄。已确认成品及其关联持久化；`Workspace::confirmed_photos()` 和只读/可写重开都会检查路径、哈希、长度与真实格式/尺寸。新会话仍要求空热文件夹，残留由使用者自行整理，Client 不自动删除或作为新来图导入。

`finish_group` 在成品提交成功且已知任务结束后，关闭组并清理本组归档副本、规范候选和缩略图，成品目录保留各队当前确认文件。热文件夹原图始终保留，校徽及其他队伍资料不受影响。清理失败独立报告，不撤销确认；调用 `retry_cleanup()` 重试。共享归档及其他队伍仍引用的成品会保留，打开中的读取句柄阻止删除。

## 拍摄后端边界

[`capture::Session`](backend/src/capture.rs) 定义组规则；[`capture_session::CaptureSession`](backend/src/capture_session.rs) 协调真实目录收图。调用方依次 await `Workspace::open`、`CaptureSession::prepare`、`select_team`、`start_group`。结束使用时 await `shutdown()`，等待已开始的文件任务退出；Drop 仅发送取消。准备时先建立原生监听，再检查热文件夹完全为空；目录必须与工作区互不包含。构造裸 Session 不代表真实目录、来源或显示条件已通过。

安全 WinRT 文件通知不依赖系统索引服务；通知和 Tokio 定时对账共用扫描入口，只处理顶层 `.jpg`、`.jpeg`、`.png`，扩展名大小写不敏感。运行中出现子目录/链接会暂停，其他扩展名保留但不导入。每次等待 250ms 超时也会对账，扫描耗时会延长周期，不保证照片在固定时间内完成归档。

Windows 通过只共享读取的源句柄防止复制时并发写入或替换；被写入者占用时等待。一次处理一个源文件，最多 50 MiB，使用 64 KiB 缓冲区。版本稳定后先写临时文件，核对字节数、版本与哈希，同步后不覆盖地发布副本。版本变化则重试，复制/监控故障暂停切组并自动恢复。源文件始终保留，已有副本不因放弃组或重拍而删除。

首次观察时固定组归属；无活动组、监控不确定期间及同路径跨组覆盖的来图先待核对，由 `assign_reviewed` 显式关联到当前组。组边界前先扫描，避免已在目录但通知延迟的文件首次挂到新组。全新路径在新组开始后才到达时，仍无法自动证明真实队伍归属；任务归零或安静间隔都不能证明相机已经结束传图。

完整目录对账确认某个待归档源已消失时，释放它的组任务，并在 `HotFolderSnapshot.missing_sources` 保留源路径和原组身份供核对，不伪造成功归档；同轮新路径先待核对。真实目录读取失败仍暂停，已有副本不受影响。

归档后，每轮最多对一份已归组副本执行处理；使用 Tokio blocking 工作线程，同一次解码生成规范候选和缩略图，快照不保留像素。输入最多 50 MiB、80 百万像素，解码器分配、解码输出和规范像素缓冲各限制 256 MiB；这些预算不等于进程 RSS 上限。`photo_checks` 只列出当前组的 `PhotoCheck`，通过时返回 `PhotoCandidate`，包含源尺寸、规范文件和缩略图的路径/哈希/长度/尺寸，失败保留分类错误及来源。内容拒绝缓存且不阻止其他照片；共享冲突等临时 I/O 失败至少间隔一秒重试，保留组任务，未完成处理阻止关闭组。归档缺失等永久错误单独拒绝，不永久占用组任务。

输入必须在文件末尾包含 JPEG/PNG 结束标记，不接收尾随数据。处理应用全部八种 EXIF 方向，保留原尺寸；JPEG 按质量 92 重编码，PNG 输出 8 位并保留透明度；缩略图最长边 512，不放大小图。RGB ICC 配置实际转换像素到 sRGB，输出只嵌入标准 sRGB ICC，不保留源 EXIF/GPS/XMP。未声明色彩时按 sRGB 处理；Gray/CMYK ICC、HDR/CICP，以及缺少 ICC/sRGB 声明但带自定义 PNG gamma/色度的输入明确拒绝。ICC 解析上限 4 MiB，规范图片和缩略图各不超过 50 MiB。

两份派生文件在私有目录中写完并同步后才一起发布，不覆盖已有候选目录。处理不修改归档或热文件夹，也不写入队伍—成品关系。退出不恢复候选归属；已确认并结束的组会清理其工作副本，未完成或放弃的组暂不自动清理。

候选在移除 EXIF 前提取 `DateTimeOriginal`，支持时区偏移和小数秒；无时区时按 Windows 本地时区解释，夏令时歧义、非法日期、缺失或超出 1 MiB 解析预算时回退到归档时记录的源文件修改时间。`captured_at().source()` 区分 EXIF 与文件时间，不用归档创建或处理完成时间推断拍摄先后；相机与电脑时钟仍需正确设置。同时间按候选 ID 倒序打破平局，失败项不参与选图。

`select_candidate(group_id, photo_id)` 只接受当前未暂停组中已准备好的候选，手动选择不会被新来图或迟到处理结果抢占。`use_latest_candidate(group_id)` 显式恢复自动最新，成功开始/放弃组时清除手动选择；旧组、其他组、失败或未完成的副本不能被选中。所有选图状态只在内存，不写数据库，返回命令结果前先发布快照。

后端呈现 API 使用 `attach_display()` / `detach_display(display_id)` 管理唯一展示窗口实例。`snapshot.presentation` 区分 Unavailable、Blank、Pending 和 Presented：未连接或无当前候选时没有可确认的照片；这些是后端事实，不要求 UI 显示 Waiting 页面。窗口适配器必须先校验并实际渲染请求指定的规范文件，再调用 `acknowledge_presentation(receipt)`；仅文件读取完成、缩略图显示或命令返回都不构成呈现。换图、换组、暂停后恢复或窗口重新挂接需要新的回执，旧窗口回调不能关闭新窗口。相同当前回执重复提交是幂等的。

同一当前窗口报告不匹配帧时，后端撤销原呈现资格并发布更高修订，防止迟到加载覆盖后仍确认旧事实；不同/旧窗口的回执只拒绝，不影响新窗口。renderer 仍须自行阻止旧加载实际覆盖新图。输出的标准 sRGB ICC 使用固定配置日期，相同规范像素不因编码时间改变哈希。

摄影师明确核对后调用 `prepare_confirmation(expected_request)`，只在当前未暂停且完整请求已有匹配回执时返回 `ConfirmationIntent`。`confirm_photo(intent)` 再次核对，锁定读取源文件并复核哈希/长度/真实格式/尺寸，以 64 KiB 缓冲复制到临时成品并同步、无覆盖发布，再用短事务更新每队唯一关联；只有这些成功后才标记组 Confirmed，返回稳定 ResultId/SubmissionId 并冻结选择。重复同一确认返回既有成品；失败保留旧成品，不把临时文件当成功。`finish_group(group_id)` 等全部已知工作结束后关闭组；同队补拍显式开启新组，新确认成功前旧成品继续有效。数据库提交失败可能留下完整但未引用的文件，结束已确认组或调用 retry_cleanup 时重新核对当前引用后清理。

当前测试仅模拟 renderer 回调，不能证明真实屏幕或选手满意，生产 CLI 没有假呈现或快捷确认命令。

`snapshot` 和 `wait_until` 只读取或等待后台事实，不是手动刷新。暂停保留组及任务，禁止选队、开始和关闭，仍允许已知工作结束。未确认组在任务排空后可显式放弃；正常结束要求已确认保存的事实，当前没有生产假确认接口。

## 构建与测试

```powershell
cargo test -p natsume-photo-client --locked
if ($LASTEXITCODE -ne 0) { throw 'Tests failed; do not continue.' }

cargo clippy -p natsume-photo-client --all-targets --locked -- -D warnings
if ($LASTEXITCODE -ne 0) { throw 'Clippy failed; do not continue.' }

cargo fmt -p natsume-photo-client -- --check
```

只运行真实文件系统收图测试：

```powershell
cargo test -p natsume-photo-client --lib capture_session::integration_tests --locked
```

现有 Windows 自动化覆盖名单验证、工作区重开及事务回滚、资源校验、组锁定、重复/旧回调、原生文件通知、分段写入、归档失败重试、监控目录恢复和重启残留，也覆盖成品文件发布失败、确认事务回滚、补拍保护、取消等待、重复确认及共享引用校验。测试使用临时目录和虚构名单，不需要真实相机，不修改正式摄影资料。真实相机、实际展示确认、物理断电和批量性能尚未验收。

仓库统一预提交入口为 `prek run --all-files`。它覆盖整个 Natsume，本包测试通过不能代替全仓检查。当前 Windows `photo-client` 构建/打包目标和 CI 已接入，具体包定义见 [`tools/process.ts`](../../tools/process.ts)。

## 代码组织与文档维护

| 位置 | 职责 |
|---|---|
| `backend/src/main.rs` | CLI、日志初始化和错误报告 |
| `backend/src/initialization/` | HTTPS 授权、下载、凭据、恢复与只读诊断 |
| `backend/src/roster.rs` | 名单类型与身份验证 |
| `backend/src/workspace/` | 工作区、私有数据库访问及展示资源缓存 |
| `backend/src/capture.rs` | 内存会话、拍摄组与任务屏障 |
| `backend/src/capture_session.rs` | 拥有拍摄会话，顺序协调操作、监控与归档；操作发布快照后应答 |
| `backend/src/capture_session/ingestion.rs` | 增量对账、有界内容复核及来源归属事实 |
| `backend/src/hot_folder/` | 私有原生通知和复制归档实现 |
| `../natsume-photo-protocol/` | 初始化 DTO、规范值类型及成果清单/签名契约；token 对 Client 不透明 |
| `backend/migrations/`、`backend/src/diesel_schema.rs` | 本地数据库迁移和生成 schema |
| `backend/tests/` | 公共接口、文件系统及 CLI 集成测试；`unit/` 保存私有模块单元测试，`fixtures/` 保存测试数据 |

本文件作为 Photo Client 的持续功能总览。新增或改变功能时，同步更新对应的当前能力、调用/启动方式、数据保存行为与测试范围；尚未实现或未验收的能力继续明确标注，不把开发计划写成已支持。后端细节另见 [`backend/README.md`](backend/README.md)。

### Review 后的初始化与拍摄约束

快照获取使用 POST，资源下载使用 GET。Server 容量耗尽时，Client 对这两类可重放请求按 Retry-After 有界重试；不会自动重试配对、确认或完成。完成前以及工作区发布前，均核对原始 manifest、名单、校徽和凭据库中的签名密钥。授权 JSON 拒绝未知字段，解析错误不输出被拒绝的字段值。

同一工作区只允许一个可写所有者，拍摄会话会延长写锁生命周期。doctor 使用只读数据库连接；`.writer.lock` 和目标同级 `.NAME.photo-init.lock` 是持久锁文件，不要在进程运行时删除。锁文件保留不表示进程仍在运行，系统在句柄关闭或进程退出后释放锁。

热文件夹周期扫描先检查元数据，不每轮读取全部相机原图；全局每三十秒至多额外复核一个未变文件，覆盖保留时间戳的改写，复核发现延迟随文件数增长。消失后的来源重现仍需明确归属，告警确认不会让它自动归给新队伍。Session 由 `CaptureSession` 管理，监控/归档处理不拥有队伍选择策略；成品确认由呈现协议与独立保存命令控制，窗口适配器现通过真实绘制回调回报，双屏混合 DPI、拔插和真实相机仍需在目标现场验收。

拍摄 API、错误和状态统一位于 `capture_session`，底层 `hot_folder` 为私有实现。工作区完整性检查与授权诊断是两个边界；`open` / `inspect` 检查本地数据，首次初始化恢复和 `doctor` 还核对服务器来源及本机签名密钥。测试直接建库的辅助函数只在 `tests/unit/` 编译，不是生产 API。

### 确认后的清理结果

`finish_group(group_id)` 返回 `CleanupReport { removed_files, pending_paths, failures }`。组转换被拒绝时返回 Err；已结束组的文件清理失败放在 report 中，确认仍有效，也可开始下一队。快照的 cleanup_pending / cleanup_failures 表示最近一次清理结果，零 pending 不代替检查 failures。

预览应先调用 `CandidateImage::open()` 并保持返回的读取句柄，使用结束再 drop。句柄采用 Windows FILE_SHARE_READ，阻止并发写入和删除；仅保留路径或旧快照不会延长文件寿命。后续导出也需保持同样的读取句柄。组结束后旧候选路径可能失效，展示协议发布 Blank。

成品对账校验当前数据库引用元数据，本组副本清理只校验相关队伍的成品，再扫描保留的 finals/<规范哈希>.jpg/png 命名空间；不扫描删除陌生名称，不递归删除目录。发布、提交、引用固定与清理共用 Workspace 所有的互斥锁；快照整图检查在固定读取句柄后移到锁外，正常组清理不再哈希所有既有成品。当前组或待核对照片仍共用的归档不会删除；源文件观察记录继续保留，A 的原图不会因工作副本已删除而重新出现在 B 的候选中。

清理队列只在当前 CaptureSession 内存中，结束会话前应检查结果并重试；进程中断遗留的旧拍摄目录及临时文件目前不自动扫除，未完成组不恢复。无引用、完整命名的成品可在后续 retry_cleanup 中重新发现。未声称工作区能在异常退出后自动清空所有残留。

成果格式由 `natsume-photo-protocol::results` 定义，包含 JCS 规范化、Ed25519 签名和外部可信上下文验签。当前已接入原密钥、固定文件快照、导出重试记录与 ZIP 写入；current_result_entries 仍只返回元数据。详见后端 README。

## 离线导出单个 ZIP

```powershell
cargo run -p natsume-photo-client -- export C:\PhotoData\Contest C:\PhotoExports\photos.zip
```

输出父目录需已存在，目标为工作区之外的本地绝对路径。只打包当前已确认成品，包含 manifest.json 与去重的 images/<sha256>.jpg/png；不包含原片、候选、SQLite、校徽或密钥。图片使用 STORE，64 KiB 缓冲；完整 ZIP 上限 8 GiB。没有结果、授权资料变化或原密钥缺失/不匹配时失败，不自动生成替代密钥，不发起 Server 请求。

导出持有同一快照所有图片的只读句柄，补拍可继续，正在导出的旧文件暂缓清理。相同内容在工作区 exports/<内容指纹>.json 保存已签名清单，失败重试或重开后复用 manifest_id；成功关闭、逐项校验并无覆盖发布 ZIP 后，才写 .complete.json。输出已有完全相同字节可复用，不同内容返回冲突；补拍后的新包使用新路径，不覆盖已交付旧包。本地成功不表示 Server 接收。

临时 ZIP 在目标同目录生成，写入/关包失败不发布；完成记录失败可能留下完整 ZIP，但返回失败，重试核实同一文件后补记。没有宣称 Windows 父目录 fsync 或物理断电保证。导出记录不得手工重写来绕过校验。
导出在授权及快照读取前尝试准入；同工作区已有导出时返回 Busy，不先建立第二份全量快照。已受理操作即使等待者取消也继续保持租约。ZIP 已发布但完成记录失败时，PublishedButUnrecorded 错误携带已发布包信息和记录失败原因，调用者可准确提示并重试；清单/包格式保持不变。
