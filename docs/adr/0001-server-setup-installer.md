# ADR 0001：Server 交互式首次安装工具

- 状态：Accepted
- 日期：2026-10-28
- 架构依据：[docs/architecture.md §5.3](../architecture.md#53-pki)。本文记录决策背景与取舍，不建立平行架构权威；当前契约以架构文档为准，交付以实现与验收证据为准。

## 背景

手工 OpenSSL 流程需要正确处理两套 CA、PEM/DER、加密备份与在线 PKCS#8、IP SAN、有效期覆盖、文件属主权限和 bootstrap 身份，容易出错。现场可能已有组织 CA 或离线私钥，不能一律生成新证书。通用 Deb 的安装脚本不能收集凭据、生成部署输入或要求 TTY。

## 决策

以独立 Rust crate `natsume-server-setup` 提供交互式工具，随 Server Deb 安装到 `/usr/bin/natsume-server-setup`。仅管理员显式运行时写入部署输入；maintainer scripts 永不调用。仅覆盖首次安装，任一七个部署输入存在即拒绝；重配置、续签、CA 替换、修复与升级仍按[运维手册](../operations-deployment.zh-CN.md#84-配置证书和版本更新)手工执行。

Control CA 支持生成、导入证书与私钥、仅证书；TLS 支持签发、导入或 CSR；Origin CA 支持生成或导入 P-256 CA 与私钥。来源组合受实际签名能力约束；CSR 首轮只保存管理员 pending，请求签回后续接。

| 决策 | 采纳结果 |
| --- | --- |
| D1 私钥保存 | 新 CA 私钥一次性加密 PEM 展示，落盘默认否；不落盘须输入 key saved，abort 中止；导入私钥原件不另存。Control 私钥不进入服务目录 |
| D2 既有设备与 CA 替换 | 随范围收窄，已部署检测后拒绝运行；本期不提供 CA 替换或设备迁移流程 |
| D3 原子性与回滚 | 单文件原子发布；健康检查前普通失败回滚本次未变动文件，不保证断电整组原子性，永不删除数据库或 server-root.key；已展示私钥须作废 |
| D4 bootstrap 边界 | 服务用户只读 probe，已初始化则跳过；否则以服务用户继承 TTY 调用 bootstrap，凭据由 Server 读取；不改变 bootstrap 命令语义 |
| D5 Control 信任锚 | 允许非自签 CA 作为直接信任锚，leaf 必须直接由它签发，浏览器补链由部署方负责 |
| D6 有效期 | leaf 覆盖 gateway_not_after 且到期不晚于 Control CA；Origin CA 也须覆盖 Gateway 期限 |
| D7 多 IP | leaf SAN 包含全部 IP；首个用于 Client 与健康检查；纯 IPv4 监听 0.0.0.0，含 IPv6 则 [::]，目标系统双栈需实测 |
| D8 校验差异 | 严格 P-256、显式 keyUsage、期限覆盖及 leaf 链/SAN 策略仅在 setup；修正文档中不存在的 probe signature 描述，本期不改变 serve |

健康检查通过为提交点。此后的 Client 公共交接导出、摘要展示和 pending 清理失败只记录并提示手工补齐，不回滚已运行服务；CSR 等待签名也是正常退出。

## 备选方案

- Bash/OpenSSL 脚本：外部命令与错误解析分散，难以复用 Server 配置/TLS 校验，也增加秘密传输和跨阶段清理负担，否决。
- 扩展 natsume-server 子命令：把 root 部署编排、交互依赖与服务用户运行边界混入现有三模式 CLI，否决；独立工具通过窄 lib 门面及受限子进程复用能力。
- 非交互应答文件：扩大凭据保存、权限与重跑契约，本期只需管理员 TTY 首次安装，否决，未提供无人值守模式。

## 后果与验收

新增 inquire 交互依赖及 PKCS#8/PBES2/SEC1 支持，OpenSSL 用于测试互通而非生产 PKI 外部命令。部署前可统一校验证书和文件计划，同时保留手工流程。

serve 当前仍接受部分未满足 setup 严格策略的材料，运行时加固留待后续任务；后续手工变更须自行完成完整 PKI 校验。非 root 临时目录与模拟 Host 测试、构建检查和 Deb 内容验证不能代替 Ubuntu 24.04 amd64 VM 验收，真实 root/TTY/UID、SQLite WAL、systemd 与 IPv4/IPv6 必须在目标环境验证。操作细节见 [setup README](../../crates/natsume-server-setup/README.md) 和[运维手册](../operations-deployment.zh-CN.md)。
