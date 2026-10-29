# Windows 摄影客户端

`natsume-photo-client` 面向 Windows 摄影工作台，Rust 包位于 [`backend/`](backend/)。当前实现无 UI 的离线后端：本地名单与资源工作区、队伍优先拍摄组、热文件夹自动收图及源文件复制归档。产品目标以[架构第 23 节](../../docs/architecture.md#23-单赛事运营平台扩展)为准；Windows 便携包的构建入口见[打包说明](../../packaging/README.md#windows-portable-packages)。

## 当前能力

| 功能 | 当前行为 |
|---|---|
| 非秘密名单 | 从授权 Server 下载并验证队伍身份和中英文资料，拒绝密码、座位及未知字段 |
| 本地工作区 | 创建/打开 SQLite 工作区，保存名单、格式版本及资料完成标记 |
| 展示资源 | 缓存学校校徽，按内容哈希去重，打开时校验 |
| 拍摄组 | 选队与开始组分开，组内锁定队伍，未结束的已知任务阻止关闭组 |
| 自动收图 | 原生文件通知加后台自动对账，处理源版本变化、重复发现和故障重试 |
| 源副本 | 只复制、不移动或删除源照片；不确定归属留待人工核对 |

当前没有拍摄 UI 或交互式收图 CLI。图片完整解码与规范化、缩略图和候选选择、真实展示确认、确认成品持久化、签名成果 ZIP 尚未实现。已归档源副本不能直接视为有效候选或确认成品。

正式首次初始化已支持从 Server 下载精简名单和校徽，校验后保存到本地工作区。JSON 读取只用于内部准备和测试夹具，正式 CLI 不提供本地 JSON 初始化入口。没有照片时展示留空，不配置默认图片。

仅首次初始化联网；日常启动读取本地工作区，拍摄、重拍、确认和导出按离线流程实现，不每次重新下载。Server 来源授权与下载契约见 [Server README](../natsume-server/README.md#photo-initialization)。Client 已接入配对、Windows 凭据库、HTTPS 下载、完成回执及重试恢复；成果 ZIP 导出／接收及摄影 UI 仍待实现。

## 启动与操作

以下命令在仓库根目录的 Windows 原生 PowerShell 中执行。使用仓库 `rust-toolchain.toml` 指定的 Rust、对应 MSVC 构建工具和 Windows SDK。Windows 内置编译 SQLite，不要求安装数据库服务。

```powershell
cargo build -p natsume-photo-client --locked
if ($LASTEXITCODE -ne 0) { throw 'Build failed; do not continue.' }

cargo run -p natsume-photo-client --locked -- --help
```

当前 CLI 提供两个子命令：

| 命令 | 操作 |
|---|---|
| `initialize <path> --server <HTTPS origin> [--ca-certificate <PEM path>]` | 正式首次初始化，或恢复同一目标的未完成初始化；Windows 凭据库保存秘密 |
| `doctor <path>` | 只读离线校验工作区、名单、校徽、授权上下文及签名密钥，不创建或迁移数据库 |

正式初始化示例：管理员先通过 Photo 管理 API 创建来源并生成一次性配对码（当前未提供 Web 管理页面），然后在 Windows PowerShell 中运行：

```powershell
cargo run -p natsume-photo-client --locked -- initialize C:\PhotoData\Contest --server https://natsume.example:8443 --ca-certificate C:\PhotoData\server-ca.pem
```

`C:\PhotoData` 必须已存在，目标 `Contest` 尚不存在。首次执行会提示输入隐藏的配对码，不把代码作为命令行参数。使用操作系统已信任的证书时可省略 `--ca-certificate`，不提供跳过 TLS 校验选项。拒绝 HTTP、带用户名／密码或业务路径的 URL、HTTP 重定向和超限响应。

Client 在同级 `.Contest.photo-init` 目录记录非秘密恢复日志、暂存校徽与待发布工作区。Ed25519 签名种子和临时 token 放在当前 Windows 用户的凭据库中，不写入 JSON／SQLite。必须保留恢复目录，并使用同一 Windows 用户恢复；仅复制工作区不能复制凭据库中的签名能力。

中断后重新执行同一命令：已有绑定不会重新配对；下载失败可重新确认并下载同一固定快照；本地数据保存成功后若完成响应丢失，只重放同一个完成请求，不重新下载。收到完成回执后才发布正式工作区，普通 `doctor`／工作区打开不联网。已完成后再次执行 `initialize` 也只校验本地资料，不读取 CA 文件或请求 Server。

配对响应与凭据都未保存时，管理员需明确替换 pending enrollment 再提供新码；若签名密钥已丢失，Client 拒绝自动生成替代密钥。已确认但未完成且凭据过期，或尚未完成的快照名单错误时，管理员可在同一来源显式替换绑定并发新码；已完成来源禁止替换。替换后用新目标路径重新 initialize，原失败目录保留，不自动改绑旧日志或覆盖已完成工作区。失败留下的暂存目录保留供排查，不自动删除已有文件或覆盖已有工作区。Windows 使用文件同步和本地重命名发布，不宣称父目录 fsync 的断电落盘保证。

正式工作区另存 `initialization.json`（原始下载快照）和 `authorization.json`（非秘密来源、公钥及上下文）。本次不新增 Client 数据库表，名单和队伍—校徽关系仍使用现有工作区表。

本地练习通过 `cargo test -p natsume-photo-client` 使用临时目录和 [两队测试名单](backend/tests/fixtures/roster.json)，不留下正式工作区。

工作区位置由调用者指定，必须使用绝对路径，父目录已存在，创建目标尚不存在。示例的 `target` 仅用于练习；正式摄影数据应放在独立的数据目录。创建失败保留不完整目录供检查，不自动删除、覆盖或重建已有工作区。

名单是 JSON 数组，每项包含 `account_id`、`account`、`organization_id`、学校与队伍中英文名称。`account_id` 是内部稳定身份，`account` 是外部 Team ID；两者分别不能为空且唯一。英文名可省略并默认为空字符串，不接受 null。身份比较保留原值，不按名称、座位或列表行号猜测队伍。

内部资源准备及测试使用以下清单，例如清单目录中有 `logos/school.png`：

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
└─ captures/<session_id>/
   └─ <sha256>.source              未解码的拍摄源副本
```

当前只有 `photo_roster` 和 `photo_workspace` 两张业务表，另有 Diesel 自身的迁移记录。名单行保存校徽相对路径/哈希，工作区元信息保存本地资料就绪状态，不把图片写成 SQLite BLOB。`format_version` 为保留的元信息，不用于旧格式判定；`ready` 仅表示本地资料完整提交，不代表正式授权或拍摄准入。开发工作区直接重建，不实现旧库兼容迁移或版本拒绝检查；打开时检查资料完成状态及损坏引用。

会话、活动组、任务、源版本、归档观察的归属和待核对列表只在内存中。退出后保留数据库、展示资源和源副本文件，不恢复未完成拍摄状态；当前队伍重新拍摄。新会话仍要求空热文件夹，残留由使用者自行整理，Client 不自动删除或作为新来图导入。确认成品及其可靠保存属于后续功能，当前尚无队伍—确认照片关联表。

后续确认功能采用以下保留规则：每队明确确认且成品安全保存、关联提交成功后，待已知任务结束，工作区只保留该队当前成品，清理本组未采用副本和多余派生图。热文件夹原图始终保留，校徽及其他队伍资料不受影响。补拍成功替换前保留旧成品；共享或仍在使用的文件不能误删，清理失败保留成品并单独报告。当前 W03 尚未实现此确认后清理流程。

## 拍摄后端边界

[`capture::Session`](backend/src/capture.rs) 定义组规则；[`hot_folder::HotFolder`](backend/src/hot_folder.rs) 协调真实目录收图。调用方依次 await `Workspace::open`、`HotFolder::prepare`、`select_team`、`start_group`。结束使用时 await `shutdown()`，等待已开始的文件任务退出；Drop 仅发送取消。准备时先建立原生监听，再检查热文件夹完全为空；目录必须与工作区互不包含。构造裸 Session 不代表真实目录、来源或显示条件已通过。

安全 WinRT 文件通知不依赖系统索引服务；通知和 Tokio 定时对账共用扫描入口，只处理顶层 `.jpg`、`.jpeg`、`.png`，扩展名大小写不敏感。运行中出现子目录/链接会暂停，其他扩展名保留但不导入。每次等待 250ms 超时也会对账，扫描耗时会延长周期，不保证照片在固定时间内完成归档。

Windows 通过只共享读取的源句柄防止复制时并发写入或替换；被写入者占用时等待。一次处理一个源文件，最多 50 MiB，使用 64 KiB 缓冲区。版本稳定后先写临时文件，核对字节数、版本与哈希，同步后不覆盖地发布副本。版本变化则重试，复制/监控故障暂停切组并自动恢复。源文件始终保留，已有副本不因放弃组或重拍而删除。

首次观察时固定组归属；无活动组、监控不确定期间及同路径跨组覆盖的来图先待核对，由 `assign_reviewed` 显式关联到当前组。组边界前先扫描，避免已在目录但通知延迟的文件首次挂到新组。全新路径在新组开始后才到达时，仍无法自动证明真实队伍归属；任务归零或安静间隔都不能证明相机已经结束传图。

完整目录对账确认某个待归档源已消失时，释放它的组任务，并在 `HotFolderSnapshot.missing_sources` 保留源路径和原组身份供核对，不伪造成功归档；同轮新路径先待核对。真实目录读取失败仍暂停，已有副本不受影响。

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
cargo test -p natsume-photo-client --test hot_folder --locked
```

现有 Windows 自动化覆盖名单验证、工作区重开及事务回滚、事务失败、资源校验、组锁定、重复/旧回调、原生文件通知、分段写入、归档失败重试、监控目录恢复和重启残留。测试使用临时目录和虚构名单，不需要真实相机，不修改正式摄影资料。真实相机、实际展示确认、成品中断保存和批量性能尚未验收。

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
| `backend/src/hot_folder.rs`、`backend/src/hot_folder/` | 增量对账、有界内容复核、原生通知和复制归档 |
| `../natsume-photo-protocol/` | 两端共用的初始化 DTO、凭据格式及校验上限 |
| `backend/migrations/`、`backend/src/diesel_schema.rs` | 本地数据库迁移和生成 schema |
| `backend/tests/` | 公共接口、文件系统及 CLI 集成测试；`unit/` 保存私有模块单元测试，`fixtures/` 保存测试数据 |

本文件作为 Photo Client 的持续功能总览。新增或改变功能时，同步更新对应的当前能力、调用/启动方式、数据保存行为与测试范围；尚未实现或未验收的能力继续明确标注，不把开发计划写成已支持。后端细节另见 [`backend/README.md`](backend/README.md)。

### Review 后的初始化与拍摄约束

快照获取使用 POST，资源下载使用 GET。Server 容量耗尽时，Client 对这两类可重放请求按 Retry-After 有界重试；不会自动重试配对、确认或完成。完成前以及工作区发布前，均核对原始 manifest、名单、校徽和凭据库中的签名密钥。授权 JSON 拒绝未知字段，解析错误不输出被拒绝的字段值。

同一工作区只允许一个可写所有者，拍摄会话会延长写锁生命周期。doctor 使用只读数据库连接；`.writer.lock` 和目标同级 `.NAME.photo-init.lock` 是持久锁文件，不要在进程运行时删除。锁文件保留不表示进程仍在运行，系统在句柄关闭或进程退出后释放锁。

热文件夹周期扫描先检查元数据，不每轮读取全部相机原图；全局每三十秒至多额外复核一个未变文件，覆盖保留时间戳的改写，复核发现延迟随文件数增长。消失后的来源重现仍需明确归属，告警确认不会让它自动归给新队伍。Session 由 `CaptureSession` 管理，监控/归档处理不拥有队伍选择策略；尚未实现选定成品与展示确认。
