# Natsume 镜像改造交接包

本目录是交给 image builder 项目的完整改造说明和配置输入。接收方从本页开始即可实施，不需要 Natsume 源码、仓库外文档、旧 VM 文件或此前对话。本交接包使用 `teams` 比赛账号；记录日期为 2026-09-10。

交付对象是基于 Ubuntu、官方 GDM/GNOME/Xorg 的比赛工位镜像。目标是由 GDM 管理两个独立 X11 会话：waiting 显示纯黑等待/绑定界面，teams 运行完整比赛桌面；普通操作只切换前台，Home reset 才结束比赛会话并恢复正式模板。业务角色名仍为 `contest`，不能将协议、PAM 服务名或 CLI 参数一并改成 teams。

## 接收后按此顺序执行

1. 阅读 [构建输入与 Client 契约](inputs.md)，落实部署输入，核对完整 Client Deb 与本目录一致。Deb 是单独的版本化构建依赖；官方系统依赖由镜像项目的包来源提供。
2. 按 [镜像实施要求](integration.md)修改 image builder。它覆盖全部 IMG-01～08、已有镜像项目的修改位置、构建顺序、账号、桌面、PAM、模板、Live 分层与维护要求。
3. 按 [部署清单](manifest.tsv)应用配置。每个占位符、合并规则、权限和归属均在实施要求中说明。
4. 按 [验收与交付记录](acceptance.md)进行构建检查和新镜像实测，提交逐项结果及实际版本。静态检查或包安装成功不能代替系统验收。

## 目录内容

| 文件/目录 | 用途 |
| --- | --- |
| [inputs.md](inputs.md) | 外部构建依赖、站点输入、兼容性、包提供的运行文件及首次身份边界 |
| [integration.md](integration.md) | 完整行为、实施步骤、合并/渲染规则、模板构建和维护流程 |
| [acceptance.md](acceptance.md) | 从构建到 VM 的通过标准、26 项行为用例、重复次数和证据格式 |
| [manifest.tsv](manifest.tsv) | 30 项部署输入的源路径、系统目标、目标模式和应用方法 |
| `rootfs/` | 完整命名配置；路径相对于目标 root |
| `fragments/` | 合并进官方/站点文件的片段，不能当完整上游文件覆盖 |
| `templates/` | 需要实际 UID、模板版本或 unit 名的输入 |
| [check.py](check.py) | 无 Git/原仓库依赖的目录闭包与 Client Deb 一致性检查 |

所有系统目标 root 所有，目录 0755；waiting Home 内文件使用 waiting 的实际 UID/GID。清单 mode 指部署后的文件模式。交接包内普通源文件为 0644，PostLogin 脚本为 0755；AccountsService 输入副本为 0644，合并到系统目标时为 0600。

| manifest action | 应用方式 |
| --- | --- |
| copy | 安装完整命名文件，先处理与既有配置/覆盖层的冲突 |
| merge | 按 integration.md 的规则合并片段，保留完整上游栈和必要站点字段 |
| render | 替换目标路径或内容中的已定义占位符，再安装文件 |
| initialize-home | 初始化 waiting 的 environment.d 文件和所需父目录，保留 Home 所有权与其他数据 |

## 单独打包与检查

只复制整个 `image/` 目录即可，不选择性抽取文件。在该目录的父目录执行：

```sh
python3 image/check.py
tar -czf natsume-image-handoff.tar.gz image
sha256sum natsume-image-handoff.tar.gz
```

接收方在任意空目录解压，再执行：

```sh
tar -xzf natsume-image-handoff.tar.gz
python3 image/check.py
python3 image/check.py --deb /absolute/path/natsume-client.deb
```

`check.py` 只需要 Python 3.10+、POSIX sh；检查 Deb 还需要 `dpkg-deb`。它检查文件闭包、模式、部署映射、目录内文档链接，以及 Deb 中本目录副本的字节/归属/模式和关键 Client 入口。它不安装包、不修改目标系统，也不声称已经验收桌面。

Client Deb 同时将本目录原样放在 `/usr/share/natsume/image-integration/`。镜像构建先验证收到的目录与实际 Deb 一致，再以已安装包内副本为应用来源。不一致时取得匹配交接目录或重新发布 Client，不能混用另一版本的配置绕过检查。

## 交付范围

镜像项目负责在自己的仓库内实现所有步骤；本目录提供完整要求和输入，不替代该项目的模块编排、包管理或安装器。站点地址、正式 Deb、管理员账号和安装源等部署值必须由部署方提供，定义见 inputs.md；这些是显式输入，不依赖口头补充的设计决定。

Client 安装后持续保留，不设计卸载流程。本期按当前数据库、Home 窗口格式和 waiting/teams 账号全新部署，不提供重构前版本的迁移工具或兼容路径。新交付必须完成 acceptance.md。
