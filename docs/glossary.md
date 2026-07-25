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
| **Configuration generation** | 非秘密 Target 配置的单调代际。 |
| **Credential revision** | 某账号秘密发生变更后的单调修订。 |
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
