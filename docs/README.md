# 文档索引

`docs/` 保存长期维护的架构、部署运维、可选方案和 Agent 规则。[architecture.md](architecture.md) 是唯一架构权威；Client 必需的运行环境统一说明在 [Client 运行环境要求](client-environment.zh-CN.md)，镜像实现、发行版适配和目标机器验收由镜像项目负责。版本说明统一查看 [GitHub Releases](https://github.com/4o3F/Natsume/releases)。

| 文档 | 用途 |
| --- | --- |
| [目标架构与实施计划](architecture.md) | 全系统职责、协议、数据、安全边界和工作包验收 |
| [从零部署与运维手册](operations-deployment.zh-CN.md) | 使用 natsume-server-setup 首次安装、证书来源与离线 CSR、手工部署附录、Client Deb 构建、交接验收与备份恢复 |
| [Client 运行环境要求](client-environment.zh-CN.md) | 必需的系统、账号、会话、Home、部署输入与维护边界 |
| [Wayland KMS 桌面串流方案](wayland-kms-streaming.zh-CN.md) | 经授权的比赛工位可选无门户提示画面观测、SRT 传输与验收 |
| [DOMjudge 打印受理与独立交付](../packaging/print-domjudge/README.md) | 独立 wrapper 包、固定 Typst／字体、专用来源令牌、DOMjudge 配置及同步受理预算 |
| [架构决策记录（ADR）](adr/) | 已接受决策的背景、取舍与后果；架构契约仍以 architecture.md 为准 |
| [Server 首次安装工具决策](adr/0001-server-setup-installer.md) | 首次安装边界、证书来源、私钥交接与运行时校验差异 |
| [Agent 文档规则](agents/domain.md) | 领域术语、架构权威和文档阅读规则 |
| [Agent Issue 工作流](agents/issue-tracker.md) | GitHub Issues 和技能工作流的仓库约定 |

阶段计划、验收清单、VM 测试、检查点和快照审查记录放在已忽略的本地 `context/`，仅用于开发追溯。
