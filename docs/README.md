# Natsume 文档地图

> 文档状态：`ACTIVE`
> 基线日期：2026-07-31
> 当前工程阶段：Phase 0
> 当前 Gate：G0 `OPEN`

本目录使用"**一个事实、一个权威位置**"原则。消费者文档应引用稳定 ID 或链接，不应复制规范正文。任何无法定位证据的 `PASS` 都是无效状态。

## 1. 阅读路径

**架构与开发**：[`architecture.md`](architecture.md) → [`domain-model.md`](domain-model.md) → [`contracts.md`](contracts.md) → [`state-and-execution.md`](state-and-execution.md) → [`security-recovery.md`](security-recovery.md) → [`repository-layout.md`](repository-layout.md) → [`dependency-policy.md`](dependency-policy.md)

**交付与计划**：[`roadmap.md`](roadmap.md) → [`gates/phase-0-status.md`](gates/phase-0-status.md) → [`supported-platform.md`](supported-platform.md)

**决策追溯**：[`adr/README.md`](adr/README.md) → 对应 ADR

**代码评审启发式**：[`../CONTRIBUTING.md`](../CONTRIBUTING.md)

## 2. 权威性矩阵

| 主题 | 唯一权威文档 | 其他文档允许包含 |
|---|---|---|
| 产品范围、进程、职责和依赖方向 | [`architecture.md`](architecture.md) | 摘要和链接 |
| 领域实体、聚合、事务边界 | [`domain-model.md`](domain-model.md) | ID 引用 |
| HTTP、Enrollment、QUIC、D-Bus、错误契约 | [`contracts.md`](contracts.md) | 用例级引用 |
| Target/Observed、Operation/Command、状态应用 | [`state-and-execution.md`](state-and-execution.md) | Gate/测试引用 |
| 安全、秘密、证书、fail-closed、恢复规则 | [`security-recovery.md`](security-recovery.md) | `INV-*` 引用 |
| 源码目录与模块所有权 | [`repository-layout.md`](repository-layout.md) | 路径引用 |
| 依赖准入、feature、供应链规则 | [`dependency-policy.md`](dependency-policy.md) | ADR 引用 |
| 环境、版本、桌面和硬件支持状态 | [`supported-platform.md`](supported-platform.md) | 状态引用 |
| Phase、Gate 和交付顺序 | [`roadmap.md`](roadmap.md) | — |
| Phase 0/Gate 当前状态 | [`gates/phase-0-status.md`](gates/phase-0-status.md) | 状态引用 |
| 设计理由 | [`adr/`](adr/) | 决策摘要和链接 |
| 术语 | [`glossary.md`](glossary.md) | 术语链接 |

数据库 migration、Protobuf descriptor、OpenAPI、D-Bus introspection 和生成代码分别是其机器可执行结构的权威来源。Markdown 只规定稳定语义，不复制完整生成 schema。

## 3. 文档类别

- **规范文档**：`architecture.md`、`domain-model.md`、`contracts.md`、`state-and-execution.md`、`security-recovery.md`、`repository-layout.md`、`dependency-policy.md`。使用"必须/不得/应/可以"强度，且规则应当可测试。
- **状态文档**：`roadmap.md`、`supported-platform.md`、`gates/`。可频繁变化，但不得重新定义架构规则。
- **决策文档**：`adr/` 解释"为什么选择"。ADR 与规范冲突时，当前行为以规范与已接受 ADR 的最新一致集合为准。

不可测试的评审启发式属于 [`../CONTRIBUTING.md`](../CONTRIBUTING.md)，不属于规范文档。

## 4. 稳定标识

| 前缀 | 用途 | 示例 |
|---|---|---|
| `INV-*` | 架构/安全不变量 | `INV-CERT-01` |
| `Gn` / `Gn-IN-*` | Phase Gate 与输入门禁 | `G0`、`G0-IN-004` |
| `ADR-*` | 架构决策 | `ADR-0018` |
| `ERR_*` | 稳定公开错误码 | `ERR_PROTOCOL_VERSION` |

发布后的 ID 不复用。废弃时保留 ID，并标明 `RETIRED` 或 `SUPERSEDED`。

## 5. 状态词汇

单一词汇表，适用于 Gate、输入门禁和平台条目：

| 值 | 含义 |
|---|---|
| `OPEN` | 尚未开始或尚未满足 |
| `BLOCKED-INPUT` | 被未冻结的外部输入阻塞 |
| `IN-PROGRESS` | 正在进行 |
| `PASS` | 有可定位 evidence 的通过 |
| `FAIL` | 已验证不满足 |
| `RETIRED` | 不再适用，ID 保留 |

平台条目额外使用 `ARCH-FROZEN`、`REPO-PINNED`、`ENV-UNFROZEN`、`ENV-PROPOSED`、`ENV-FROZEN`、`REJECTED`，定义见 [`supported-platform.md`](supported-platform.md)。

## 6. 变更规则

一个变更只修改其权威事实源：

- 改组件职责：修改 `architecture.md`，必要时新增 ADR；
- 改领域事务：修改 `domain-model.md`；
- 改 wire/API 语义：修改 `contracts.md` 和机器 schema；
- 改安全不变量：必须新增或更新 ADR，再修改 `security-recovery.md`；
- 改 Phase 状态：修改 [`gates/phase-0-status.md`](gates/phase-0-status.md)；
- 改环境支持：修改 `supported-platform.md`，附 evidence。

合并前至少执行：

```bash
node docs/verification/validate-links.mjs docs README.md
node docs/verification/validate-markdown.mjs docs README.md
pnpm diagrams
```

## 7. 禁止的维护模式

- 在 Gate 或状态文档中重新抄写完整安全规则；
- 用"文档已写"或"文件存在"代替实现或测试证据；
- 用 `TBD`、`PASS` 或版本号掩盖未冻结输入；
- 为尚未启动的 Phase 编写详细工作包或验收细目；
- 保留只有空模板字段的报告文件。
