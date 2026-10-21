# 文档索引

`docs/` 只保存长期维护的架构、部署运维、可选方案和 Agent 规则。架构权威仍只有 [architecture.md](architecture.md)；Client 必需的运行环境统一说明在 [Client 运行环境要求](client-environment.zh-CN.md)，镜像实现、发行版适配和目标机器验收由镜像项目负责。版本说明统一查看 [GitHub Releases](https://github.com/4o3F/Natsume/releases)，仓库不再逐版本保存 release 文档。

| 文档 | 用途 |
| --- | --- |
| [目标架构与实施计划](architecture.md) | 全系统职责、协议、数据、安全边界和工作包验收 |
| [从零部署与运维手册](operations-deployment.zh-CN.md) | 创建 CA、Ubuntu Server 安装、源码构建 Client Deb、部署输入交接、注册验收与备份恢复 |
| [Client 运行环境要求](client-environment.zh-CN.md) | 必需的系统、账号、会话、Home、部署输入与维护边界 |
| [Wayland KMS 桌面串流方案](wayland-kms-streaming.zh-CN.md) | 经授权的比赛工位可选无门户提示画面观测、SRT 传输与验收 |
| [DOMjudge 打印受理与独立交付](../packaging/print-domjudge/README.md) | 独立 wrapper 包、固定 Typst／字体、专用来源令牌、DOMjudge 配置及同步预算；不代表物理打印已实现 |
| [Agent 文档规则](agents/domain.md) | 领域术语、架构权威和文档阅读规则 |
| [Agent Issue 工作流](agents/issue-tracker.md) | GitHub Issues 和技能工作流的仓库约定 |

运行环境要求只在 `docs/` 维护，不随 Client 包安装。本仓库不交付镜像配置片段、模板、适配脚本或检查器。

阶段实施计划、带状态的验收清单、VM 测试记录、工作检查点和快照审查报告放在本地 `context/`，该目录已加入 `.gitignore`。这些记录用于开发追溯，不是仓库文档或镜像构建输入，也不能代替对应发行版本的验收。正式文档不记录本机 PID、临时路径、运行句柄或逐轮测试流水。
