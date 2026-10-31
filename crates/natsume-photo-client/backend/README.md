# Natsume Photo Client 后端

Windows 专用 Rust 后端。Slint UI 位于相邻的 [`../ui/`](../ui/)，通过公开后端 API 调用功能，不在界面中访问数据库。首次通过 HTTPS 授权并下载名单和学校校徽，之后本地操作离线。SQLite 随 Windows 构建，不需要安装数据库服务。启动、恢复及产品边界见[上级 README](../README.md)。

```powershell
cargo run -p natsume-photo-client -- initialize C:\PhotoData\Contest --server https://natsume.example:8443
cargo run -p natsume-photo-client -- doctor C:\PhotoData\Contest
cargo test -p natsume-photo-client --locked
```

初始化输入隐藏的一次性配对码。签名密钥和临时 token 保存在 Windows 凭据库；工作区及恢复日志只存非秘密资料。`doctor` 不联网、不写文件、不重新配对，验证名单、校徽、下载快照与授权上下文，并核对原签名密钥。JSON 读取只供内部准备和测试夹具使用，正式 CLI 没有本地 JSON 初始化命令。

## 持久化与调用关系

`Initializer::run().await` 完成配对、确认、下载、暂存、完成回执与工作区发布。初始化模块在持锁的单个阻塞任务中调用 `Workspace::create_with_logos()` 从类型化下载资料验证校徽（内部准备不经过 resources.json），调用 `storage::create()` 创建目录和安装 schema，再将名单、校徽引用和 ready 标记放在同一事务提交。`Workspace::open().await` 只打开现有文件，检查资料就绪、名单和校徽；不迁移、不检查旧格式版本。

| 模块 | 职责 |
| --- | --- |
| `initialization/` | HTTPS、协议验证、Windows 凭据、恢复日志、只读诊断 |
| `workspace/mod.rs` | 资料就绪、名单和校徽的业务校验 |
| `workspace/storage.rs` | 装配目录、工作区写所有权、连接和 schema；创建与发布的阻塞边界由初始化调用方持有 |
| `workspace/db.rs` | typed Diesel 查询、事务和持久化错误 |
| `workspace/resources.rs` | 校徽输入、路径、限额、解码和哈希校验 |
| `capture.rs` | 内存会话、队伍锁定、拍摄组与任务屏障 |
| `capture_session.rs` | 拍摄会话所有者与有序命令/快照协调 |
| `capture_session/ingestion.rs` | 不拥有 Session 的增量收图状态 |
| `hot_folder/native.rs` | 安全 WinRT 文件通知，不使用系统索引服务 |
| `hot_folder/archive.rs` | 有界复制、源版本复核及不覆盖发布 |

数据库有 `photo_workspace`、`photo_roster` 与 `photo_team_results` 三张业务表，另有 Diesel 迁移记录。`photo_workspace` 保存 singleton、保留的 format_version 和资料 ready 标记。`photo_roster` 保存七字段队伍资料与可空的校徽相对路径和哈希。`photo_team_results` 每队保存一条当前确认成品关系。没有默认图片字段、接口或输入；没有照片时留空。旧开发工作区按约定重建，不实现旧格式兼容或拒绝逻辑，也不自动删除已有目录。

数据库连接、迁移和事务实际运行在 `spawn_blocking` 中；异步调用者不持有 SQLite 锁。资源解码和归档复制同样在阻塞任务中完成。业务条件在 Rust 中检查，数据库保留主键及唯一性约束。`ready` 只表示本地资料提交完成，不能单独证明来源授权完成。

## 收图状态与生命周期

依次 await `CaptureSession::prepare`、`select_team`、`start_group`。准备时先注册通知，再验证热文件夹为空。目录与工作区不得互相包含。消息通道处理操作，Tokio 定时器和原生通知触发同一对账入口；WinRT 对象在一个 Tokio 管理的线程中创建、使用和释放，不添加 unsafe 或手写 Send。

`IngestionState` 区分 Running、RecoveringMonitor、PausedDirectory、PausedArchive、Stopped。`Error` 表示一次调用失败，状态表示持续运行情况。文件句柄共享限制防止读取正在写入的照片；通知可能晚于写入，周期扫描仍会发现待处理任务。复制、目录或监控故障阻止切组并重试，身份和任务不丢失。完整目录枚举成功后发现 pending 源消失，则解除对应任务，在 `missing_sources: Vec<MissingSource>` 记录源路径与组身份；不创建虚假 ArchivedPhoto。同轮出现的新路径先留待核对，目录失效仍暂停。

`snapshot()` 返回 `HotFolderSnapshot`；`wait_until().await` 等待后台发布，不主动扫描。显式 `shutdown().await` 等待运行中的文件任务及监听清理，Drop 只取消。未确认会话和组只在内存中，重启不恢复；源照片永不移动或删除，归属不确定的副本需要人工核对。

归档后通过 `capture_session/photo_validation.rs` 校验 JPEG/PNG 的哈希、长度、结束标记并完整解码；同一解码像素交给私有 `candidate.rs`，应用 EXIF 方向和 RGB ICC → sRGB，保留原尺寸，生成规范文件及最长边 512 的 PNG 缩略图。JPEG 质量 92，PNG 输出 8 位并保留透明度；源 EXIF/GPS/XMP 不复制，输出嵌入标准 sRGB ICC。无色彩声明按 sRGB；未支持的 Gray/CMYK ICC、HDR/CICP、自定义 PNG gamma/色度明确拒绝。输入 50 MiB、80 百万像素，解码分配/输出及规范像素缓冲各 256 MiB，ICC 4 MiB，单个输出文件 50 MiB；预算不是整个进程 RSS 保证。

每轮最多处理一个已归组副本，直到文件都同步且一起发布后才结束组任务。`photo_checks` 保存 `Result<PhotoCandidate, PhotoValidationError>`，快照通过 `candidates()` 只迭代当前组的成功候选，不保存像素。内容拒绝缓存；共享冲突等临时 I/O 失败间隔至少一秒重试；永久失败结束任务并保留来源。发布失败清理私有暂存，不覆盖现有目标；源和原始归档保留。

`photo_time.rs` 在规范化移除元数据前解析 EXIF 原始拍摄时间、时区与小数秒；缺失/无效时回退 `ArchivedPhoto.modified` 保存的源版本修改时间。无 EXIF 时区按 Windows 本地时区解释，歧义则回退；EXIF 时间解析限制 1 MiB。快照按时间、候选 ID 稳定倒序排列，错误在后且不能被选择。`PhotoTimeSource` 记录 EXIF/文件时间来源，不用处理完成时间排序。

`CaptureSession::select_candidate(group_id, photo_id)` 将当前未暂停组的成功候选设为手动选择，`use_latest_candidate(group_id)` 恢复自动最新；新图及迟到结果不会抢占手动选择。`snapshot.selection` 使用 `CandidateSelection::Automatic { photo_id: Option<Uuid> }` / `Manual { photo_id: Uuid }`，成功开始/放弃组会清除旧选择，命令应答前发布快照。选择只在内存，成品保存使用下述独立接口；实际屏幕仍待实现，离线 ZIP 已实现，热文件夹原图保留。

`presentation.rs` 管理窗口实例、帧修订与回执；CaptureState 仍拥有组和选图策略。`publish_snapshot` 从同一份已排序快照派生展示目标，再发布新帧/应答，避免选图已完成但呈现版本仍旧。`attach_display` 返回本次窗口的 DisplayId，`detach_display` 只接受当前 ID；无候选/暂停置 Blank，停止置 Unavailable。当前图像/身份未变时保留修订与回执，A→B→A 则生成新修订。修订不能绕回；耗尽时停止并清除呈现。

`acknowledge_presentation` 严格匹配窗口、修订、组、account_id、候选 ID 和规范图片哈希；重复当前回执幂等。同一窗口报告不匹配帧会撤销呈现资格、要求更高修订的新回执；旧窗口回执只拒绝。`prepare_confirmation` 返回未保存的 ConfirmationIntent，`confirm_photo` 重验后发布完整成品、事务提交关联，最后标记组 Confirmed；确认后不能换图或恢复自动，晚到图继续处理但不替换成品。`finish_group` 等已知任务排空后关闭组，补拍用同队新组。renderer 回调仍为可信适配器职责，相邻 UI 已接入真实 AfterRendering 回执与操作员勾选“选手已满意”；实际相机和多屏混合 DPI 仍需现场验收。

`photo.rs` 保存图片格式/尺寸及独立的 ConfirmationId、ResultId、SubmissionId；捕获与工作区复用这些领域值。`workspace/final_photos.rs` 负责源/目标路径、64 KiB 流式复制、哈希/格式/尺寸验证和无覆盖发布；`workspace/db.rs` 通过 typed Diesel 与现有 Transaction 原子校验基线并写入 photo_team_results；确认幂等、身份生成与修订递增由成品业务层负责。文件失败或事务回滚不删除旧成品；提交失败的完整无引用文件留待后续对账。

工作区目前三张业务表；新表 migration 仅在新工作区安装，打开不迁移旧开发库。重开和 confirmed_photos() 都验证当前成品，共享图片只读取一次并核对所有引用元信息。确认后的已结束组清理已实现，离线 ZIP 已实现。标准 sRGB ICC 的头日期已固定，避免图像库写入当前时间造成规范输出哈希不稳定。

## 测试文件组织

所有测试内容集中在 `tests/`：根目录保留 HTTPS 初始化、CLI 和名单的集成测试；`unit/` 覆盖工作区、资源、会话、归档、原生通知、origin、取消安全和数据库错误来源，包含仅供测试的工作区夹具；`fixtures/` 保存输入。业务模块通过 `#[cfg(test)]` 和 `#[path = "..."]` 引用单元测试，因此能访问私有实现而无需扩大生产 API。`tests/unit/` 没有独立的集成测试入口，Cargo 不会重复运行这些测试。

## 验证

Windows 测试覆盖事务回滚、资料完整性、校徽限额、拍摄屏障、原生通知、分段写入、故障恢复、HTTPS 初始化和完成重放。`cargo clippy -p natsume-photo-client --all-targets --all-features -- -D warnings` 检查代码，`cargo doc -p natsume-photo-client --no-deps` 检查文档。

Server 测试在 WSL 运行。真实联调用例由 WSL Server 启动 Windows 的 `initialization` 测试可执行文件，通过临时 CA 和真实 HTTPS 完成初始化；双方显式 ignored 的联调用例需要一起执行，常规测试不会将它算为通过。Windows 凭据库的阻塞线程读写有显式运行的专项测试，使用隔离的临时条目并删除；完整现场恢复和真实相机仍需手工验收。全仓检查统一使用 `prek run --all-files`。

工作区可写打开持有排他锁，并设置 SQLite 三秒 busy_timeout；后者只应对短暂数据库竞争，不能替代写锁。CLI 通过 `diagnose_path()` 一次完成只读打开、资源校验、授权与签名凭据检查；对已经打开的工作区调用 `diagnose()` 仍会重新检查当前资源。只读类型不能用于启动拍摄。初始化保留完成前与发布前的完整校验，发布时消费已检查的只读工作区，关闭旧 SQLite 句柄后移动目录并取得目标写锁，不在发布后再次解码校徽。

初始化请求和响应直接使用 `natsume-photo-protocol` 中的同名类型；不再把 Manifest、PairingCredentials、Team 或 HotFolder 作为它们的别名。Token 只作为不透明凭据保存和发送，内部格式由 Server 验证。HTTPS 客户端的端点、响应限额和容量重试策略均显式指定；初始化存储只管理本地状态和文件，凭据适配器使用 `SecretSlot` 和独立的 `CredentialError`。

`capture_session/ingestion.rs` 私有管理文件观测、归档和来源丢失状态。它不修改拍摄会话，`CaptureSession` 根据待处理文件事实登记／结束任务，并决定暂停或恢复。

初始化标记与授权资料先写入同目录临时文件、同步后再以不覆盖方式发布。已由 Server 验证的校徽下载缓存使用内容摘要生成文件名，允许通过同样的完整临时文件替换损坏缓存；不会沿用半写入的下载文件。工作区准备从资源检查、创建、复制到名单事务都由同一个阻塞任务持有初始化 lease，调用方取消后仍等待实际任务结束才释放该锁。

## 类型、模块和本地测试

初始化所有权只在初始化模块内调度实际文件和凭据操作，取消等待不会提前解锁；工作区的写所有权由工作区、拍摄和数据库任务持有。两者不能互换。Config 保留用户 URL 输入，运行中的 HTTPS、凭据定位和授权文件使用验证后的 ServerOrigin。签名公钥与 SHA-256 摘要使用不同类型。

拍摄公开 API、错误、状态和快照统一从 `capture_session` 导入；`hot_folder` 是内部通知和归档实现。公开库不提供测试建库函数。需要直接构造工作区的私有单元测试放在 `tests/unit/`，通过 cfg(test) path 接入；HTTPS 初始化、CLI 和名单仍使用集成测试。测试夹具调用生产准备核心，不维护另一份 schema 或建库逻辑。

`Workspace::open` / `WorkspaceReader::inspect` 检查数据完整性；`Initializer` 和 `doctor` 另行检查本地授权与当前用户凭据中的原签名密钥。打开 SQLite 本身不证明授权绑定有效。

## 已确认组清理

`capture_session/cleanup.rs` 负责已结束组的删除队列。finish_and_clean 先执行 Session.finish_group 的确认/任务屏障校验，成功后 retire_group_files 收集本组确切文件、移除已结束组的照片与候选记录，再执行 clean_retired。源观察和归属墓碑不清除，其他组及待核对照片继续保护共享归档。

finish_group 与 retry_cleanup 返回 CleanupReport，文件失败不撤销确认。快照公开 cleanup_pending / cleanup_failures；使用者必须检查 failures，不能把 Result::Ok 等同于所有文件已删。已删除或缺失的队列条目可幂等重试；只 remove_file / remove_dir，不递归清理未知目录内容。预览用 CandidateImage::open 保持只共享读取的句柄，释放后才可删除。

FinalPhotos::remove_unused 在共享 final-operation mutex 内校验引用元数据、发现无引用哈希文件并删除。该锁由 Workspace 所有并向 FinalPhotos 共享，数据库对象不持有文件锁。打开/诊断的整图校验在读取句柄保护下、互斥锁外进行；正常组清理只校验待退休副本涉及的队伍，不重读其他队伍的成品。文件操作期间不持有数据库事务；锁顺序为 final-operation 后数据库。finals 中共享/当前成品保持，删除失败的无引用对象下次重新对账，避免沿用过期删除清单。

本步没有新增表或依赖，没有在打开工作区时自动扫旧 captures。工作副本删除队列只在当前会话内重试；未确认/放弃组、崩溃遗留拍摄目录、未知文件与临时文件留待明确维护，不恢复未完成拍摄。完整的无引用哈希成品可重新发现。
## 离线成果清单（ZIP 前置契约）

`natsume-photo-protocol::results` 定义 ResultsManifest、TeamResult、SignedResults、ResultContext、成果/提交/清单身份及真实图片格式。Client 的 photo 模块复用同一个 ResultId、SubmissionId、PhotoFormat，现有数据库字符串与格式值保持原样。

`Workspace::current_result_entries()` 验证当前成品，并从已初始化名单按 account_id 读取外部 account，生成按内部身份排序的共享条目。未确认候选不进入结果；该接口只返回元数据，不持有文件读取句柄，也不是未来导出所需的固定文件快照。

签名为版本化领域前缀加 RFC 8785 JCS 编码，验签要求外部可信初始化上下文及已登记公钥。当前已接入 Vault 原密钥签名、固定文件读取快照、Manifest 重试记录、流式 ZIP 与 export CLI。格式和限额以 docs/architecture.md §23.6 为准，测试固定向量在协议 crate 的 tests/fixtures 中。
## 导出边界与文件生命周期

export::export_results 先复核授权上下文与名单/资源，调用 Workspace::export_snapshot。在 final-operation mutex 内取得并校验当前引用元数据，打开每张不同图片的 FILE_SHARE_READ 句柄后立即释放锁；整图哈希在锁外的实际写包阶段核验。快照不保持数据库事务；文件句柄跨越 ZIP 写入和校验，阻止清理或外部写入。捕获会话可确认新的结果，导出仍只读原快照。

export.rs 在授权和快照读取之前尝试取得类型化 ExportLease（.export.lock 与工作区所有权），已有导出则立即返回 Busy；受理后的后台任务保持租约直到实际操作结束，外层等待取消不会释放准入。离线授权由 initialization/authorization.rs 共享给 doctor 与 export，使用独立 AuthorizationError，不依赖 HTTP 初始化错误；exports 记录的内容指纹不含 manifest_id，已有记录严格验签并核对内容后复用。export/journal.rs 仅保存非秘密 JSON，先同步完整临时文件，再无覆盖发布，不写候选或会话表。

export/archive.rs 使用 STORE、固定 ZIP 时间与有界写入器；每张图片流式核对字节和 SHA-256。关包后重新逐项读取校验，核对包长、条目数和元数据预算，再发布目标。只有目标与完成记录都成功才返回 ExportedPackage；目标已发布但完成记录失败返回 PublishedButUnrecorded，携带已发布包信息及 JournalError。该返回值没有 Server 接收含义。可调用 export::export_results 或使用 export <workspace> <output.zip> CLI。

导出目标限制本地磁盘绝对路径，拒绝 UNC、父级穿越、ADS 和工作区内部输出；父目录需存在。现有不同内容绝不覆盖。下载授权令牌无需存在，导出只需要原签名密钥和已完成的本地资料。测试注入 Secrets 和文件写入故障，不增加生产测试字段。
初始化恢复目录在正式工作区发布成功后清理，删除前关闭日志句柄，清理期间继续持有同级永久初始化锁。失败或中断保留恢复数据；清理失败记录 tracing 警告，不将已成功发布的工作区报告为初始化失败。已完成工作区通过 `initialization::open_authorized` 离线打开，不再次执行首次初始化。
