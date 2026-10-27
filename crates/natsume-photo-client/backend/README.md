# Natsume Photo Client 后端

当前实现 W01.1～W01.3、W02 和 W03：名单与资源工作区、内存会话/拍摄组规则，以及自动热文件夹监控和安全复制归档。没有 UI 或网络。Windows 内置编译 SQLite，不需要安装数据库服务。

在仓库根目录执行：

```powershell
cargo run -p natsume-photo-client -- check-roster crates/natsume-photo-client/backend/tests/fixtures/roster.json
cargo test -p natsume-photo-client --locked
```

创建并检查练习工作区（父目录必须存在，目标目录必须尚不存在）：

```powershell
$photoWorkspace = Join-Path $PWD ('target\photo-workspace-' + [guid]::NewGuid().ToString('N'))
cargo run -p natsume-photo-client -- create-workspace $photoWorkspace --roster crates/natsume-photo-client/backend/tests/fixtures/roster.json
cargo run -p natsume-photo-client -- inspect-workspace $photoWorkspace
```

工作区有 `workspace.sqlite3`，配置图片时还会创建 `resources/`。仍只有 `photo_workspace` 和 `photo_roster` 两张业务表及 Diesel 自身迁移记录；校徽路径/哈希放在名单行，默认图路径/哈希放在元信息。格式版本为 1，独立的 `ready` 标记在资源复制校验后随名单和引用一起提交；它不代表正式 Server 初始化或授权。重新打开会检查版本、完成标记、名单和配置过的资源，名单按外部 `account` 排序返回。

可给 `create-workspace` 增加 `--resources <资源清单JSON路径>`，例如准备目录中有 `logo.png`、`default.jpg` 和以下 `resources.json`：

```json
{
  "school_logos": [{ "organization_id": 1, "path": "logo.png" }],
  "default_image": "default.jpg"
}
```

路径相对资源清单所在目录，禁止绝对路径、`..` 和链接越界；校徽学校必须出现在名单中且不能重复配置。图片只接受 PNG/JPEG，每张最多 16 MiB、宽高各不超过 4096 像素，解码分配上限 64 MiB。内容按 SHA-256 命名并去重，复制后不依赖源文件。

没有配置图片时返回空引用，UI 阶段使用内置图；配置后文件缺失或哈希不符会拒绝打开，不悄悄忽略。校验哈希不是来源签名。W01.2 旧工作区打开时自动增加可空资源列，原名单保留。

创建失败时保留不完整目录供检查，不自动删除或覆盖。请检查失败原因并选择新的目标路径重试。SQLite 事务不覆盖目录创建，也不承诺完整断电恢复。

输入是队伍 JSON 数组，字段见 `tests/fixtures/roster.json`：

- `account_id`：内部稳定身份；`account`：外部 Team ID，对应 Server 的 DOMjudge username。均不能为空且分别唯一。
- `organization_id`、`school_name_zh`、`team_name_zh`：学校标识及中英文资料中的必填项。
- `school_name_en`、`team_name_en`：允许空字符串，缺省时为空；不接受 null。
- 拒绝未知字段，包括密码、座位和凭据修订字段。

身份比较保留输入原值，不自动裁剪、改大小写或根据名称匹配。此 JSON 是本地开发输入，尚不是已经实现的 Server DTO；检查通过不代表登记或初始化完成。

仅输出成功队伍数或安全错误摘要，不输出名单内容。详细 Rust 学习说明位于仓库 `photodesign/explainfiles/W01.1-Rust学习说明.md`、`photodesign/explainfiles/W01.2-Rust学习说明.md` 和 `photodesign/explainfiles/W01.3-Rust学习说明.md`。

W02 的 `capture::Session` 消费已经验证的 `Roster`，选队使用内部 `account_id`；选队不等于开始组，开始后锁定队伍。已知任务以独立身份登记，未全部结束时不能关闭组。暂停保留组和任务，阻止选队、开始和关闭，但允许登记和完成已有组的工作。未确认组只能明确放弃；正常结束要求已经确认并保存成功的事实，该事实由后续 W05 实现，当前没有公开的确认快捷接口。

这些状态不写入 SQLite 或状态文件，销毁会话后全部丢弃；重新打开工作区仍须重新选队拍摄。W02 学习说明位于本地 `photodesign/explainfiles/W02-Rust学习说明.md`。

W03 通过 `hot_folder::HotFolder::prepare(&mut workspace, hot_folder)` 建立后台收图器，先建立原生监听再检查空目录。目录必须与工作区互不包含；新会话有任何文件或子目录就拒绝，绝不清空或导入残留。`select_team`、`start_group`、`abandon_group` 统一协调收图与组边界，调用者不能取得其内部可变 `Session`。当前没有新增拍摄 CLI；可用 `cargo test -p natsume-photo-client --test hot_folder --locked` 验证真实 Windows 文件事件。

通知和每 250ms 等待后的自动对账共用扫描入口，只处理顶层 `.jpg`、`.jpeg`、`.png`（大小写不敏感）。运行中出现子目录/链接会暂停；其他扩展名保留但不导入。每次扫描流式重新计算文件哈希，补偿漏通知及同名同大小变化；大量照片时扫描耗时会延长对账周期。单文件最多 50 MiB，复制缓冲区为 64 KiB；一次处理一个文件，未同时把整批原图载入内存。

Windows 用只允许共享读取的源句柄避免复制中并发写入/替换；被写入者占用时等待重试。观察版本稳定后，先写临时文件、核对版本/哈希、同步文件，再不覆盖地发布到 `captures/<session_id>/<sha256>.source`。这些仍是未解码的源副本，扩展名筛选和短暂稳定不证明图片完整，W04 才校验实际图片。相同字节可共享文件，但每次源版本观察保留独立身份与组归属。归档失败暂停切组并自动重试，不删除源文件或旧副本。

无组时、监控中断期间发现的源版本，以及同一路径跨组覆盖，默认不归队。只能用 `assign_reviewed(photo_id, group_id)` 明确核对后关联到当前组。组边界前先扫描，避免已有来图首次挂到新组；全新路径在新组开始后才首次出现的迟到照片仍无法从时间戳判定真实队伍，须由摄影师核对，不能把任务归零当成相机已无来图。

`snapshot`/`wait_until` 只读取或等待后台事实，不是手动刷新入口。销毁收图器会停止并等待线程退出，副本保留，但观察/候选归属不恢复，也不新增数据库表或状态文件。工作区/显示准入仍不能由裸 `Session::new` 或 `resume` 代替，来源登记及实际显示条件在后续阶段接入。完整代码与测试说明见本地 `photodesign/explainfiles/W03-Rust学习说明.md`。
