# 术语表

本文件只定义术语，不定义行为。行为以对应规范文档为准。

| 术语 | 定义 |
|---|---|
| **Server truth** | Server 业务数据库中已经提交的权威领域事实。 |
| **Target** | Server 从已提交事实计算出的、面向某台 Device 的非秘密期望状态。Target 本身不产生远端副作用。 |
| **Observed snapshot** | Device 报告的实际可观察状态。它是设备应用状态的唯一业务来源。 |
| **Drift** | Target 与最新有效 Observed snapshot 的差异。 |
| **Command** | Server 发送给单台 Device 的持久化、可重试、幂等远端指令。批量操作 = 批量 Command + 查询聚合。 |
| **Device** | 一台受管理工作站的业务实体。内部主键与硬件 ID 分离。 |
| **Device Token** | Enrollment 时 Server 生成的 32 字节不透明随机凭据；Device 对 Server 的控制面认证身份。Server 只存哈希；无 TTL，失效仅经吊销、替换或 reset。 |
| **Provisioning window** | Server 持久化的受审计开关；开启期间 Enrollment 可签发 Device Token 与 Gateway certificate，关闭后不存在任何签发路径。 |
| **Gateway certificate** | provisioning 窗口内经 Enrollment 签发、供本机 Caddy loopback HTTPS 使用的证书；私钥在 Client 本地生成且不离机。 |
| **Machine Hardware ID** | 按固定多源配方（ADR-0025）规范化并派生的稳定机器标识；不是认证凭据。 |
| **Fleet namespace UUID** | 站点级公开且不可变的 UUID，用于确定性派生 Machine Hardware ID。 |
| **Binding** | Seat 与 Device 当前业务关联。 |
| **Assignment revision** | Binding/席位分配的单调修订，用于拒绝陈旧操作，并作为 Import Commit 的第二个 CAS。 |
| **Confirmed contest configuration** | Server 当前权威的 Seat/account/credential-metadata 集合；只能通过完整 candidate 的显式 Import Commit 被替换。 |
| **Contest configuration revision** | Confirmed contest configuration 内容的单调 revision（`ContestConfigurationRevision`）；`0` 表示空 baseline；仅内容实际变化时递增；用作 import baseline CAS token。 |
| **Configuration generation** | 面向单台 Device 的非秘密 Target 配置代际；由 `(ContestConfigurationRevision, 站点 policy 版本)` 确定性派生，不独立计数；Device 按其拒绝陈旧 `SYNC_STATE`。 |
| **Credential revision** | 某账号秘密发生变更后的单调修订。 |
| **Candidate import** | 单次 CSV upload 的不可变解析结果；外部以 `import_id` 标识；全局同一时刻最多一个 pending candidate。 |
| **Import preview / import diff** | Server 对 candidate 与 confirmed baseline 的 redacted 结构化比较结果。Server 是 classification 的唯一权威。 |
| **Import Commit** | Operator 对 candidate 的显式二次确认动作；以双 CAS（ContestConfigurationRevision + AssignmentRevision）校验后原子应用。Material 含必要的 unbind-and-replace 与内容 revision 提升；no-op 仅 lineage/redacted audit。不自动产生 Device Command。 |
| **Preview token** | Server 签发的 opaque 证据，绑定 candidate 身份、baseline `ContestConfigurationRevision`、完整 redacted diff 与 expiry。 |
| **Import Discard** | Operator 按 `import_id` 显式放弃尚未提交的 candidate：转入终端 `DISCARDED`；不改变 confirmed configuration、binding、revision 或 Target；幂等。 |
| **Session epoch** | 当前受管桌面会话的身份代际；会话操作必须绑定该 epoch。 |
| **Home epoch** | 当前 Home 准备事务的身份代际；不得跨 epoch 复用未证明安全的结果。 |
| **Client 凭据文件** | Device 本地 root-owned 权限文件（Device Token、Gateway key/leaf、Seat 凭据、LKG），原子写，无应用层加密（ADR-0026）。 |
| **Server vault** | Server 数据库中的应用层加密秘密存储。 |
| **LKG** | Last Known Good，本地最后一次已验证可用的配置或证书集合。 |
| **Caddy BLOCKED** | Caddy 仅提供有限本地状态页，不代理 DOMjudge 的 fail-closed 状态。 |
| **Caddy READY** | Gateway 证书、配置和目标 upstream 都通过验证后激活的数据面状态。 |
| **Enrollment** | provisioning 窗口内的 server-auth HTTPS 注册流程；同一事务签发 Device Token 与 Gateway certificate；重复注册为受审计的替换。 |
| **xheaders 自动登录** | DOMjudge 官方 X-Headers 认证：Caddy 仅在 `/login` 路由注入 `X-DOMjudge-Login` 与 base64 的 `X-DOMjudge-Pass`（ADR-0024）。 |
| **镜像升级重验清单** | 每次赛事镜像 bump 后必须重跑的桌面 capability 验证集（ADR-0027），维护于 supported-platform.md。 |
| **typed contract** | 输入集合封闭、字段和枚举明确、能够被穷举校验的接口契约。 |
| **evidence locator** | 能定位到可复现实验、CI、artifact、日志或签署记录的稳定引用。 |
| **fail closed** | 无法证明安全或一致时停止敏感动作，而不是猜测、降级或自动重建身份。 |
