# 术语表

本文件只定义术语，不定义行为。行为以对应规范文档为准。

| 术语 | 定义 |
|---|---|
| **Server truth** | Server 业务数据库中已经提交的权威领域事实。 |
| **Target** | Server 从已提交事实计算出的、面向某台 Device 的非秘密期望状态。Target 本身不产生远端副作用。 |
| **Observed snapshot** | Device 报告的实际可观察状态。它是设备应用状态的唯一业务来源。 |
| **Drift** | Target 与最新有效 Observed snapshot 的差异。 |
| **Operation** | 需要跨目标聚合、追踪或人工呈现的异步业务动作。 |
| **Operation target** | Operation 对某个具体 Device 的聚合状态。 |
| **Command** | Server 发送给单台 Device 的持久化、可重试、幂等远端指令。 |
| **Attempt** | Command 的一次投递或执行观察，不等同于新的业务动作。 |
| **Device** | 一台受管理工作站的业务实体。内部主键与硬件 ID 分离。 |
| **Device Identity certificate** | Enrollment 后获得、用于 Device 对 Server 的 mTLS 身份证书。 |
| **Gateway certificate** | 由 active `SYNC_STATE` 子协议签发、供本机 Caddy/浏览器数据面使用的证书。 |
| **Machine Hardware ID** | 从硬件来源规范化并派生的稳定机器标识；不是认证凭据。 |
| **Fleet namespace UUID** | 站点级公开且不可变的 UUID，用于确定性派生 Machine Hardware ID。 |
| **Binding** | Seat 与 Device 当前业务关联。 |
| **Assignment revision** | Binding/席位分配的单调修订，用于拒绝陈旧操作。 |
| **Confirmed contest configuration** | Server 当前权威的 Seat/account/credential-metadata 集合；不是永久 frozen Seat universe，只能通过完整 candidate 的显式 Import Commit 被替换。 |
| **Contest configuration revision** | Confirmed contest configuration 内容的单调 revision（`ContestConfigurationRevision`）；`0` 表示空 baseline；仅内容实际变化时递增；用作 import baseline CAS token。**不是** Device 侧的 Configuration generation。 |
| **Configuration generation** | 面向单台 Device 的非秘密 Target 配置单调代际。**不是** Contest configuration revision；二者不得混用。 |
| **Credential revision** | 某账号秘密发生变更后的单调修订。 |
| **Candidate import** | 单次 CSV upload 的不可变解析结果；外部以 `import_id` 标识。Server 内部 candidate digest/revision 仅存在于 encrypted staging / secret-safe persistence。 |
| **Import preview / import diff** | Server 对 candidate 与 confirmed baseline 的 redacted 结构化比较结果。**不是** Device Target 或 Observed snapshot；Server 是 classification 的唯一权威。 |
| **Import Commit** | Operator 对 candidate 的显式二次确认动作；幂等预检之后经 live 校验应用。Material 含必要的 unbind-and-replace 与内容 revision 提升；no-op 仅 lineage/redacted audit 且 revision 不变。不新增独立 confirmation resource，也不自动产生 Device Command。 |
| **Preview token** | Server 签发或持久化的 opaque 证据，绑定内部 candidate identity、baseline `ContestConfigurationRevision`、完整 redacted diff、精确 binding impact 集合（每项为 `(SeatCode, DevicePk 或允许展示的非秘密 Device identity, AssignmentRevision, UNBIND_ON_COMMIT)`）、actor authorization context 与 expiry；不得暴露 password-derived digest。 |
| **Import Discard** | Operator 按 `import_id` 显式放弃尚未提交的 candidate：转入终端 `DISCARDED`，使 preview token/evidence 对 commit 失效；不改变 confirmed configuration、binding、revision 或 Target；对已 discarded 幂等，不得撤销已 COMMITTED import。 |
| **Session epoch** | 当前受管桌面会话的身份代际；会话操作必须绑定该 epoch。 |
| **Home epoch** | 当前 Home 准备事务的身份代际；不得跨 epoch 复用未证明安全的结果。 |
| **Client vault** | Device 本地应用加密存储，保存需要离线稳态使用的秘密和证书材料。 |
| **Server vault** | Server 数据库中的应用层加密秘密存储。 |
| **LKG** | Last Known Good，本地最后一次已验证可用的配置或证书集合。 |
| **Caddy BLOCKED** | Caddy 仅提供有限本地状态页，不代理 DOMjudge 的 fail-closed 状态。 |
| **Caddy READY** | Gateway 证书、配置和目标 upstream 都通过验证后激活的数据面状态。 |
| **Enrollment** | server-auth HTTPS 上的 Device Identity 注册流程；不签发 Gateway certificate。 |
| **active `SYNC_STATE`** | 已在 authenticated mTLS QUIC 上创建且仍有效的显式状态同步 Command 上下文。 |
| **typed contract** | 输入集合封闭、字段和枚举明确、能够被穷举校验的接口契约。 |
| **evidence locator** | 能定位到可复现实验、CI、artifact、日志或签署记录的稳定引用。 |
| **fail closed** | 无法证明安全或一致时停止敏感动作，而不是猜测、降级或自动重建身份。 |
