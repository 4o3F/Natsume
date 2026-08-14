# Architecture Decision Records

ADR 记录“为什么选择”，不替代当前规范。当前行为由权威规范与机器 schema 定义；ADR 只保留跨边界取舍、被拒绝方案、代价和重开条件。

## 当前决策集

| ADR | 主题 | 状态 | 主属范围 |
|---|---|---|---|
| [0030](0030-foundation-deployment-and-delivery-baseline.md) | Foundation, deployment, and delivery baseline | ACCEPTED | 全局 / P0 / P1 / P7 |
| [0031](0031-contest-import-and-secret-evidence.md) | Contest import and secret-evidence scope | ACCEPTED | P2 |
| [0032](0032-device-identity-and-local-credential-lifecycle.md) | Device identity and local credential lifecycle | ACCEPTED | P3 |
| [0033](0033-enrollment-and-device-control-boundary.md) | Enrollment and Device control boundary | ACCEPTED | P3 / P4 |
| [0034](0034-state-execution-and-data-plane-boundary.md) | State, execution, and data-plane boundary | ACCEPTED | P2 / P4 / P5 |
| [0035](0035-session-home-and-desktop-cycle.md) | Session, Home, and desktop cycle | ACCEPTED | P0 / P6 |
| [0036](0036-error-architecture-and-public-codes.md) | Error architecture and public codes | ACCEPTED | 全部 |
| [0037](0037-operator-identity-and-server-runtime-secrets.md) | Operator identity and Server runtime secret material | ACCEPTED | P1 |

ADR-0001～ADR-0029 已在一次性 consolidation 中退出 Git。旧 ID、原状态、替代关系和当前去向见 [`history-map.md`](history-map.md)。完整旧正文仅存在于本地 ignored archive，不是 clean clone 或 CI 的依赖。

## 状态

- `PROPOSED`：待批准；
- `ACCEPTED`：当前治理决策，不表示功能已经实现；
- `SUPERSEDED`：被后续 ADR 整体替代；
- `REJECTED`：未采用；
- `DEPRECATED`：仍可能存在，但计划移除。

工程完成度只由 [`../roadmap.md`](../roadmap.md)、Gate 与 [`../supported-platform.md`](../supported-platform.md) 的 evidence 状态决定。

## 阅读规则

1. 先从 [`../README.md`](../README.md) 找到主题的唯一权威规范；
2. 需要理解取舍时再阅读对应主题 ADR；
3. 遇到旧提交、issue 或注释中的 `ADR-00xx` 时，通过 [`history-map.md`](history-map.md) 定位当前主题；
4. 不从历史 ADR 或 ADR 摘要反推 wire、数据库、状态机或安全不变量的当前细节。

## 维护规则

- 新 ADR 使用 [`0000-template.md`](0000-template.md)；
- ID 单调递增，不复用；
- 同一主题内、不改变稳定边界的澄清直接更新现有主题 ADR；
- 只有新增独立 trust boundary、真实反转、兼容性承诺或持久化身份语义时才新增 ADR；
- 接受或修改 ADR 后，同步更新唯一权威规范和机器 schema；
- `supersede` 必须在新旧 ADR 双向标记；条款级变化优先更新主题 ADR，避免继续拆分微型记录；
- 详细协议字段、测试步骤、Gate 清单、probe/runbook 和预实现工作包不得写入 ADR；
- 放宽任何 `INV-*` 必须在 ADR 中逐条说明安全影响和恢复边界。
