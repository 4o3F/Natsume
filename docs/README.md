# 文档索引

`docs/` 保存长期维护的架构、产品要求、部署和维护说明。架构权威仍只有 [architecture.md](architecture.md)；其他文档落实产品行为或部署要求，不另建架构规则。

| 文档 | 用途 |
| --- | --- |
| [目标架构与实施计划](architecture.md) | 全系统职责、协议、数据、安全边界和工作包验收 |
| [GNOME 双会话 PRD](prd-gnome-dual-session.zh-CN.md) | 产品范围、会话/Home 行为、AT-01～26 和测量口径 |
| [镜像交付要求](gnome-session-image-requirements.zh-CN.md) | IMG-01～08、镜像构建顺序和最终交付条件 |
| [镜像配置附录](gnome-session-image-configuration.zh-CN.md) | 正式输入的目标路径、权限、归属和接入位置 |

交付独立 image builder 时，直接打包完整 [packaging/image](../packaging/image/README.md) 目录；其中包含输入、实施说明、验收标准和检查器。

阶段实施计划、带状态的验收清单、VM 测试记录、工作检查点和快照审查报告放在本地 `context/`，该目录已加入 `.gitignore`。这些记录用于开发追溯，不是仓库文档或镜像构建输入，也不能代替对应发行版本的验收。正式文档不记录本机 PID、临时路径、运行句柄或逐轮测试流水。
