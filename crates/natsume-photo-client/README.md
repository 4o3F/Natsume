# Windows 摄影客户端

`natsume-photo-client` 面向 Windows 摄影工作台，Rust 包位于 [`backend/`](backend/)。当前实现无 UI 的离线后端：本地名单与资源工作区、队伍优先拍摄组、热文件夹自动收图及源文件复制归档。产品目标以[架构第 23 节](../../docs/architecture.md#23-单赛事运营平台扩展)为准；Windows 便携包的构建入口见[打包说明](../../packaging/README.md#windows-portable-packages)。

## 当前能力

| 功能 | 当前行为 |
|---|---|
| 非秘密名单 | 从开发 JSON 读取并验证队伍身份和中英文资料，拒绝密码、座位及未知字段 |
| 本地工作区 | 创建/打开 SQLite 工作区，保存名单、格式版本及资料完成标记 |
| 展示资源 | 缓存学校校徽和可选默认图，按内容哈希去重，打开时校验 |
| 拍摄组 | 选队与开始组分开，组内锁定队伍，未结束的已知任务阻止关闭组 |
| 自动收图 | 原生文件通知加后台自动对账，处理源版本变化、重复发现和故障重试 |
| 源副本 | 只复制、不移动或删除源照片；不确定归属留待人工核对 |

当前没有拍摄 UI 或交互式收图 CLI。图片完整解码与规范化、缩略图和候选选择、真实展示确认、确认成品持久化、签名成果 ZIP、Server 首次初始化尚未实现。已归档源副本不能直接视为有效候选或确认成品。

当前本地测试暂时从 JSON 文件读取队伍名单，并通过本地资源清单加载校徽和可选默认图。接入 Server 后，正式首次初始化将改为从 Server 下载精简名单、校徽和可选默认图，校验后保存到本地工作区；当前 JSON 输入仅用于开发测试，不是最终正式初始化方式，导入成功也不代表来源登记完成。

仅首次初始化联网；日常启动读取本地工作区，拍摄、重拍、确认和导出全程离线，不每次重新下载。管理员通过 Web Panel 上传完整成果 ZIP。Server 下载和来源登记目前尚未实现。

## 启动与操作

以下命令在仓库根目录的 Windows 原生 PowerShell 中执行。使用仓库 `rust-toolchain.toml` 指定的 Rust、对应 MSVC 构建工具和 Windows SDK。Windows 内置编译 SQLite，不要求安装数据库服务。

```powershell
cargo build -p natsume-photo-client --locked
if ($LASTEXITCODE -ne 0) { throw 'Build failed; do not continue.' }

cargo run -p natsume-photo-client --locked -- --help
```

当前 CLI 只有三个子命令：

| 命令 | 操作 |
|---|---|
| `check-roster <path>` | 检查名单，不创建工作区 |
| `create-workspace <path> --roster <path> [--resources <path>]` | 创建尚不存在的新工作区，可同时缓存展示资源 |
| `inspect-workspace <path>` | 打开工作区并校验版本、完成标记、名单及配置资源 |

使用随代码提供的[两队测试名单](backend/tests/fixtures/roster.json)创建练习工作区：

```powershell
cargo run -p natsume-photo-client --locked -- check-roster crates/natsume-photo-client/backend/tests/fixtures/roster.json
if ($LASTEXITCODE -ne 0) { throw 'Roster validation failed; do not continue.' }

$photoWorkspace = Join-Path $PWD ('target\photo-workspace-' + [guid]::NewGuid().ToString('N'))
cargo run -p natsume-photo-client --locked -- create-workspace $photoWorkspace --roster crates/natsume-photo-client/backend/tests/fixtures/roster.json
if ($LASTEXITCODE -ne 0) { throw 'Workspace creation failed; do not continue.' }

cargo run -p natsume-photo-client --locked -- inspect-workspace $photoWorkspace
```

工作区位置由调用者指定，必须使用绝对路径，父目录已存在，创建目标尚不存在。示例的 `target` 仅用于练习；正式摄影数据应放在独立的数据目录。创建失败保留不完整目录供检查，不自动删除、覆盖或重建已有工作区。

名单是 JSON 数组，每项包含 `account_id`、`account`、`organization_id`、学校与队伍中英文名称。`account_id` 是内部稳定身份，`account` 是外部 Team ID；两者分别不能为空且唯一。英文名可省略并默认为空字符串，不接受 null。身份比较保留原值，不按名称、座位或列表行号猜测队伍。

可给创建命令增加 `--resources <资源清单路径>`。例如资源清单所在目录有 `logos/school.png` 和 `default.jpg`：

```json
{
  "school_logos": [{ "organization_id": 1, "path": "logos/school.png" }],
  "default_image": "default.jpg"
}
```

路径相对清单所在目录，禁止绝对路径、`..` 和链接越界。学校 ID 必须出现在名单中，不能重复配置；校徽列表和默认图均可不配置。展示资源只接受真实 PNG/JPEG，每张最多 16 MiB、宽高各不超过 4096 像素、解码分配上限 64 MiB。未配置返回空引用；已配置但缺失或损坏时拒绝打开，不静默忽略。

## 本地数据与重启

```text
工作区/
├─ workspace.sqlite3
├─ resources/                      校徽和可选默认图缓存
└─ captures/<session_id>/
   └─ <sha256>.source              未解码的拍摄源副本
```

当前只有 `photo_roster` 和 `photo_workspace` 两张业务表，另有 Diesel 自身的迁移记录。名单行保存校徽相对路径/哈希，工作区元信息保存默认图引用，不把图片写成 SQLite BLOB。格式版本当前为 1；`ready` 仅表示本地资料完整提交，不代表正式授权或拍摄准入。打开支持旧库的兼容迁移，但拒绝未知格式、未完成资料和损坏引用。

会话、活动组、任务、源版本、归档观察的归属和待核对列表只在内存中。退出后保留数据库、展示资源和源副本文件，不恢复未完成拍摄状态；当前队伍重新拍摄。新会话仍要求空热文件夹，残留由使用者自行整理，Client 不自动删除或作为新来图导入。确认成品及其可靠保存属于后续功能，当前尚无队伍—确认照片关联表。

后续确认功能采用以下保留规则：每队明确确认且成品安全保存、关联提交成功后，待已知任务结束，工作区只保留该队当前成品，清理本组未采用副本和多余派生图。热文件夹原图始终保留，校徽/默认图及其他队伍资料不受影响。补拍成功替换前保留旧成品；共享或仍在使用的文件不能误删，清理失败保留成品并单独报告。当前 W03 尚未实现此确认后清理流程。

## 拍摄后端边界

[`capture::Session`](backend/src/capture.rs) 定义组规则；[`hot_folder::HotFolder`](backend/src/hot_folder.rs) 协调真实目录收图。调用方先 `Workspace::open`，再 `HotFolder::prepare`、`select_team`、`start_group`。准备时先建立原生监听，再检查热文件夹完全为空；目录必须与工作区互不包含。构造裸 Session 不代表真实目录、来源或显示条件已通过。

后台通知和自动对账共用扫描入口，只处理顶层 `.jpg`、`.jpeg`、`.png`，扩展名大小写不敏感。运行中出现子目录/链接会暂停，其他扩展名保留但不导入。每次等待 250ms 超时也会对账，扫描耗时会延长周期，不保证照片在固定时间内完成归档。

Windows 通过只共享读取的源句柄防止复制时并发写入或替换；被写入者占用时等待。一次处理一个源文件，最多 50 MiB，使用 64 KiB 缓冲区。版本稳定后先写临时文件，核对字节数、版本与哈希，同步后不覆盖地发布副本。版本变化则重试，复制/监控故障暂停切组并自动恢复。源文件始终保留，已有副本不因放弃组或重拍而删除。

首次观察时固定组归属；无活动组、监控不确定期间及同路径跨组覆盖的来图先待核对，由 `assign_reviewed` 显式关联到当前组。组边界前先扫描，避免已在目录但通知延迟的文件首次挂到新组。全新路径在新组开始后才到达时，仍无法自动证明真实队伍归属；任务归零或安静间隔都不能证明相机已经结束传图。

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

现有 Windows 自动化覆盖名单验证、工作区重开/迁移、事务失败、资源校验、组锁定、重复/旧回调、原生文件通知、分段写入、归档失败重试、监控目录恢复和重启残留。测试使用临时目录和虚构名单，不需要真实相机，不修改正式摄影资料。真实相机、实际展示确认、成品中断保存和批量性能尚未验收。

仓库统一预提交入口为 `prek run --all-files`。它覆盖整个 Natsume，本包测试通过不能代替全仓检查。当前 Windows `photo-client` 构建/打包目标和 CI 已接入，具体包定义见 [`tools/process.ts`](../../tools/process.ts)。

## 代码组织与文档维护

| 位置 | 职责 |
|---|---|
| `backend/src/main.rs` | CLI、日志初始化和错误报告 |
| `backend/src/roster.rs` | 名单类型与身份验证 |
| `backend/src/workspace/` | 工作区、私有数据库访问及展示资源缓存 |
| `backend/src/capture.rs` | 内存会话、拍摄组与任务屏障 |
| `backend/src/hot_folder.rs`、`backend/src/hot_folder/` | 自动监控、对账和复制归档 |
| `backend/migrations/`、`backend/src/diesel_schema.rs` | 本地数据库迁移和生成 schema |
| `backend/tests/` | 公共接口、文件系统及 CLI 集成测试 |

本文件作为 Photo Client 的持续功能总览。新增或改变功能时，同步更新对应的当前能力、调用/启动方式、数据保存行为与测试范围；尚未实现或未验收的能力继续明确标注，不把开发计划写成已支持。后端细节另见 [`backend/README.md`](backend/README.md)。
