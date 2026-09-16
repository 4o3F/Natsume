# 文档索引

`docs/` 保存长期维护的架构、部署运维、可选方案和发布说明。架构权威仍只有 [architecture.md](architecture.md)；镜像接入、配置输入和验收说明统一维护在 [packaging/image](../packaging/image/README.md)，不再重复维护独立的功能 PRD 或镜像配置附录。

| 文档 | 用途 |
| --- | --- |
| [目标架构与实施计划](architecture.md) | 全系统职责、协议、数据、安全边界和工作包验收 |
| [从零部署与运维手册](operations-deployment.zh-CN.md) | 创建 CA、Ubuntu Server 安装、源码构建 Client Deb、部署输入交接、注册验收与备份恢复 |
| [v2.6.0 发布说明](releases/v2.6.0.md) | DOMjudge 命令行提交认证与动态 Gateway 地址 |
| [镜像接入与交付](../packaging/image/README.md) | 完整交接包、IMG-01～08、配置输入、构建顺序和维护要求 |
| [镜像验收标准](../packaging/image/acceptance.md) | AT-01～26、测量口径、证据要求和最终交付条件 |
| [Wayland KMS 桌面串流方案](wayland-kms-streaming.zh-CN.md) | 经授权的比赛工位可选无门户提示画面观测、SRT 传输与验收 |
| [Agent 文档规则](agents/domain.md) | 领域术语、架构权威和文档阅读规则 |
| [Agent Issue 工作流](agents/issue-tracker.md) | GitHub Issues 和技能工作流的仓库约定 |

交付独立 image builder 时，直接打包完整 [packaging/image](../packaging/image/README.md) 目录；其中包含输入、实施说明、验收标准和检查器。

阶段实施计划、带状态的验收清单、VM 测试记录、工作检查点和快照审查报告放在本地 `context/`，该目录已加入 `.gitignore`。这些记录用于开发追溯，不是仓库文档或镜像构建输入，也不能代替对应发行版本的验收。正式文档不记录本机 PID、临时路径、运行句柄或逐轮测试流水。
