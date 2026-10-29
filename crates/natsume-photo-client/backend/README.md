# Natsume Photo Client 后端

Windows 专用 Rust 后端，当前没有 UI。首次通过 HTTPS 授权并下载名单和学校校徽，之后本地操作离线。SQLite 随 Windows 构建，不需要安装数据库服务。启动、恢复及产品边界见[上级 README](../README.md)。

```powershell
cargo run -p natsume-photo-client -- initialize C:\PhotoData\Contest --server https://natsume.example:8443
cargo run -p natsume-photo-client -- doctor C:\PhotoData\Contest
cargo test -p natsume-photo-client --locked
```

初始化输入隐藏的一次性配对码。签名密钥和临时 token 保存在 Windows 凭据库；工作区及恢复日志只存非秘密资料。`doctor` 不联网、不写文件、不重新配对，验证名单、校徽、下载快照与授权上下文，并核对原签名密钥。JSON 读取只供内部准备和测试夹具使用，正式 CLI 没有本地 JSON 初始化命令。

## 持久化与调用关系

`Initializer::run().await` 完成配对、确认、下载、暂存、完成回执与工作区发布。`Workspace::create_with_logos().await` 从类型化下载资料验证校徽（内部准备不经过 resources.json），调用 `storage::create().await` 创建目录和安装 schema，再将名单、校徽引用和 ready 标记放在同一事务提交。`Workspace::open().await` 只打开现有文件，检查资料就绪、名单和校徽；不迁移、不检查旧格式版本。

| 模块 | 职责 |
| --- | --- |
| `initialization/` | HTTPS、协议验证、Windows 凭据、恢复日志、只读诊断 |
| `workspace/mod.rs` | 资料就绪、名单和校徽的业务校验 |
| `workspace/storage.rs` | 在 Tokio 阻塞任务中装配目录、连接和 schema |
| `workspace/db.rs` | typed Diesel 查询、事务和持久化错误 |
| `workspace/resources.rs` | 校徽输入、路径、限额、解码和哈希校验 |
| `capture.rs` | 内存会话、队伍锁定、拍摄组与任务屏障 |
| `capture_session.rs` | 拍摄会话所有者与有序命令/快照协调 |
| `hot_folder.rs` | 不拥有 Session 的增量收图状态 |
| `hot_folder/native.rs` | 安全 WinRT 文件通知，不使用系统索引服务 |
| `hot_folder/archive.rs` | 有界复制、源版本复核及不覆盖发布 |

数据库只有 `photo_workspace` 与 `photo_roster` 两张业务表，另有 Diesel 迁移记录。`photo_workspace` 保存 singleton、保留的 format_version 和资料 ready 标记。`photo_roster` 保存七字段队伍资料与可空的校徽相对路径和哈希。没有默认图片字段、接口或输入；没有照片时留空。工作区直接重建，不实现旧格式兼容或拒绝逻辑。

数据库连接、迁移和事务实际运行在 `spawn_blocking` 中；异步调用者不持有 SQLite 锁。资源解码和归档复制同样在阻塞任务中完成。业务条件在 Rust 中检查，数据库保留主键及唯一性约束。`ready` 只表示本地资料提交完成，不能单独证明来源授权完成。

## 收图状态与生命周期

依次 await `HotFolder::prepare`、`select_team`、`start_group`。准备时先注册通知，再验证热文件夹为空。目录与工作区不得互相包含。消息通道处理操作，Tokio 定时器和原生通知触发同一对账入口；WinRT 对象在一个 Tokio 管理的线程中创建、使用和释放，不添加 unsafe 或手写 Send。

`IngestionState` 区分 Running、RecoveringMonitor、PausedDirectory、PausedArchive、Stopped。`Error` 表示一次调用失败，状态表示持续运行情况。文件句柄共享限制防止读取正在写入的照片；通知可能晚于写入，周期扫描仍会发现待处理任务。复制、目录或监控故障阻止切组并重试，身份和任务不丢失。完整目录枚举成功后发现 pending 源消失，则解除对应任务，在 `missing_sources: Vec<MissingSource>` 记录源路径与组身份；不创建虚假 ArchivedPhoto。同轮出现的新路径先留待核对，目录失效仍暂停。

`snapshot()` 返回 `HotFolderSnapshot`；`wait_until().await` 等待后台发布，不主动扫描。显式 `shutdown().await` 等待运行中的文件任务及监听清理，Drop 只取消。未确认会话和组只在内存中，重启不恢复；源照片永不移动或删除，归属不确定的副本需要人工核对。

当前没有候选图解码、选图确认、成品持久化或成果 ZIP。后续确认成功后才清理本组未采用的工作区副本，热文件夹原图保留。

## 测试文件组织

所有测试内容集中在 `tests/`：根目录的 Rust 文件是集成测试，`unit/` 中的七个文件分别测试 capture、hot_folder、native、workspace_db、workspace_resources、初始化日志锁的取消安全，以及显式运行的 Windows 凭据库读写，`fixtures/` 保存测试输入。业务模块通过 `#[cfg(test)]` 和 `#[path = "..."]` 引用对应单元测试，因此仍能访问父模块的私有实现，不需要扩大业务 API 的可见性。`tests/unit/` 没有独立的集成测试入口，Cargo 不会重复运行这些测试。

## 验证

Windows 测试覆盖事务回滚、资料完整性、校徽限额、拍摄屏障、原生通知、分段写入、故障恢复、HTTPS 初始化和完成重放。`cargo clippy -p natsume-photo-client --all-targets --all-features -- -D warnings` 检查代码，`cargo doc -p natsume-photo-client --no-deps` 检查文档。

Server 测试在 WSL 运行。真实联调用例由 WSL Server 启动 Windows 的 `initialization` 测试可执行文件，通过临时 CA 和真实 HTTPS 完成初始化；双方显式 ignored 的联调用例需要一起执行，常规测试不会将它算为通过。Windows 凭据库的阻塞线程读写有显式运行的专项测试，使用隔离的临时条目并删除；完整现场恢复和真实相机仍需手工验收。全仓检查统一使用 `prek run --all-files`。

工作区可写打开持有排他锁，并设置 SQLite 三秒 busy_timeout；后者只应对短暂数据库竞争，不能替代写锁。`Workspace::open_readonly()` 只打开就绪名单元信息，CLI 随后调用 `diagnose()` 完整检查资源、授权与签名凭据。初始化的 verify 与 doctor 使用同一校验入口；同一轮校徽按内容引用去重读取/解码，工作区发布前保留必要的重新校验。
