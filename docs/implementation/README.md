# Implementation Phase Plans

本目录把路线图拆成可执行工作包。权威范围、契约、安全和状态规则位于：

- [架构](../architecture.md)
- [领域模型](../domain-model.md)
- [契约](../contracts.md)
- [状态与执行](../state-and-execution.md)
- [安全与恢复](../security-recovery.md)
- [路线图](../roadmap.md)

Phase 文档只说明：

- 入口条件；
- 工作包；
- 交付物；
- Definition of Done；
- Gate evidence；
- 明确非目标。

它们不重新定义 `INV-*`、wire 字段、平台状态或 requirement 当前状态。

| Phase | 文档 |
|---|---|
| 0 | [Engineering Baseline](phase-0-engineering-baseline.md) |
| 1 | [Control Domain](phase-1-control-domain.md) |
| 2 | [CSV Preparation](phase-2-csv-preparation.md) |
| 3 | [Identity & Enrollment](phase-3-identity-enrollment.md) |
| 4 | [QUIC & Command Runtime](phase-4-quic-command.md) |
| 5 | [State, Gateway & Data Plane](phase-5-state-gateway-data-plane.md) |
| 6 | [Session & Home](phase-6-session-home.md) |
| 7 | [Production Release](phase-7-production-release.md) |

状态：

- 当前 Phase：0；
- G0：`OPEN`；
- 当前机器状态：[Phase 0 status](../verification/phase-0-status.md)。
