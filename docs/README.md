# Natsume 文档地图

> 文档状态：`ACTIVE`  
> 基线日期：2026-07-24  
> 当前工程阶段：Phase 0  
> 当前 Gate：G0 `OPEN`

本目录使用“**一个事实、一个权威位置**”原则。消费者文档应引用稳定 ID 或链接，不应复制规范正文。任何无法定位证据的 `PASS` 或 `SATISFIED` 都是无效状态。

## 1. 阅读路径

### 架构与开发

1. [`architecture.md`](architecture.md)
2. [`domain-model.md`](domain-model.md)
3. [`contracts.md`](contracts.md)
4. [`state-and-execution.md`](state-and-execution.md)
5. [`security-recovery.md`](security-recovery.md)
6. [`repository-layout.md`](repository-layout.md)
7. [`dependency-policy.md`](dependency-policy.md)

### 交付与计划

1. [`roadmap.md`](roadmap.md)
2. [`implementation/README.md`](implementation/README.md)
3. [`gates/phase-0-status.md`](gates/phase-0-status.md)
4. [`supported-platform.md`](supported-platform.md)
5. [`lab/phase-0-inventory.md`](lab/phase-0-inventory.md)

### 运维与恢复

1. [`runbooks/README.md`](runbooks/README.md)
2. [`security-recovery.md`](security-recovery.md) 中的恢复不变量

### 决策追溯

1. [`adr/README.md`](adr/README.md)
2. 对应 ADR

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
| Phase、Gate 和交付顺序 | [`roadmap.md`](roadmap.md) | Phase 文档的工作包 |
| Phase 0/Gate 状态 | [`gates/phase-0-status.md`](gates/phase-0-status.md) | 状态引用 |
| 运维步骤 | [`runbooks/`](runbooks/) | 架构不变量引用 |
| 设计理由 | [`adr/`](adr/) | 决策摘要和链接 |
| 术语 | [`glossary.md`](glossary.md) | 术语链接 |

数据库 migration、Protobuf descriptor、OpenAPI、D-Bus introspection 和生成代码分别是其机器可执行结构的权威来源。Markdown 只规定稳定语义，不复制完整生成 schema。

## 3. 文档类别

- **规范文档**：`architecture.md`、`domain-model.md`、`contracts.md`、`state-and-execution.md`、`security-recovery.md`、`repository-layout.md`、`dependency-policy.md`。使用“必须/不得/应/可以”强度。
- **状态文档**：`roadmap.md`、`supported-platform.md`、`gates/`、`lab/`、`probes/`。可频繁变化，但不得重新定义架构规则。
- **决策文档**：`adr/` 解释“为什么选择”。ADR 与规范冲突时，当前行为以规范与已接受 ADR 的最新一致集合为准。
- **操作文档**：`runbooks/` 只描述执行、验证、回滚和升级路径。

## 4. 稳定标识

| 前缀 | 用途 | 示例 |
|---|---|---|
| `INV-*` | 架构/安全不变量 | `INV-CERT-01` |
| `Gn-*` | Phase Gate | `G0-005` |
| `Gn-IN-*` | Gate 输入门禁 | `G0-IN-004` |
| `ADR-*` | 架构决策 | `ADR-0018` |
| `PROBE-*` | 探针 | `PROBE-B` |
| `ERR_*` | 稳定公开错误码 | `ERR_PROTOCOL_VERSION` |

发布后的 ID 不复用。废弃时保留 ID，并标明 `RETIRED` 或 `SUPERSEDED`。

## 5. 变更规则

一个变更只修改其权威事实源：

- 改组件职责：修改 `architecture.md`，必要时新增 ADR；
- 改领域事务：修改 `domain-model.md`；
- 改 wire/API 语义：修改 `contracts.md` 和机器 schema；
- 改安全不变量：必须新增或更新 ADR，再修改 `security-recovery.md`；
- 改 Phase 状态：修改 [`gates/phase-0-status.md`](gates/phase-0-status.md)；
- 改环境支持：修改 `supported-platform.md`，附 evidence；
- 改操作步骤：修改对应 runbook，不复制架构背景。

合并前至少执行：

```bash
node docs/verification/validate-links.mjs docs README.md
node docs/verification/validate-markdown.mjs docs README.md
pnpm diagrams
```

## 6. 状态词汇

- Requirement：`OPEN`、`IN-PROGRESS`、`BLOCKED-INPUT`、`SATISFIED`、`FAILED`、`RETIRED`。
- Gate：`OPEN`、`BLOCKED-INPUT`、`PASS`、`FAIL`。
- 平台：`ARCH-FROZEN`、`ENV-PROPOSED`、`ENV-UNFROZEN`、`ENV-FROZEN`、`REJECTED`。

定义见 [`supported-platform.md`](supported-platform.md)。

## 7. 禁止的维护模式

- 在 Phase、Gate、runbook 中重新抄写完整安全规则；
- 用“文档已写”“文件存在”代替实现或测试证据；
- 用 `TBD`、`PASS` 或版本号掩盖未冻结输入。
