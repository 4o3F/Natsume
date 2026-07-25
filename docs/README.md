# Natsume 文档地图

> 文档状态：`ACTIVE`  
> 基线日期：2026-07-24  
> 当前工程阶段：Phase 0  
> 当前 Gate：G0 `OPEN`

本目录使用“**一个事实、一个权威位置**”原则。消费者文档应引用稳定 ID 或链接，不应复制规范正文。任何无法定位证据的 `PASS` 或 `SATISFIED` 都是无效状态。

## 1. 建议阅读路径

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
3. [`verification/phase-0-status.md`](verification/phase-0-status.md)
4. [`supported-platform.md`](supported-platform.md)
5. [`lab/phase-0-inventory.md`](lab/phase-0-inventory.md)

### 运维与恢复

1. [`runbooks/README.md`](runbooks/README.md)
2. 对应事件的具体 runbook
3. [`security-recovery.md`](security-recovery.md) 中的恢复不变量

### 决策追溯

1. [`adr/README.md`](adr/README.md)
2. 对应 ADR
3. 相关架构或契约章节

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
| Phase 0 requirement/Gate 状态 | [`verification/registry.json`](verification/registry.json) | 生成的 Markdown |
| 运维步骤 | [`runbooks/`](runbooks/) | 架构不变量引用 |
| 设计理由 | [`adr/`](adr/) | 决策摘要和链接 |
| 术语 | [`glossary.md`](glossary.md) | 术语链接 |

数据库 migration、Protobuf descriptor、OpenAPI、D-Bus introspection 和生成代码分别是其机器可执行结构的权威来源。Markdown 只规定稳定语义，不复制完整生成 schema。

## 3. 文档类别

### 规范文档

`architecture.md`、`domain-model.md`、`contracts.md`、`state-and-execution.md`、`security-recovery.md`、`repository-layout.md`、`dependency-policy.md`。

规范使用：

- **必须**：不可省略的约束；
- **不得**：明确禁止；
- **应**：默认要求，偏离需要 ADR 或可审计理由；
- **可以**：允许但不要求。

### 状态文档

`roadmap.md`、`supported-platform.md`、`verification/`、`lab/`、`probes/`。

状态文档可以频繁变化，但不得重新定义架构规则。它们只能引用规范 ID、requirement ID、Gate ID 和 evidence locator。

### 决策文档

`adr/` 解释“为什么选择”，不替代当前规范。ADR 与规范冲突时：

1. 已接受但尚未同步到规范，视为文档缺陷；
2. 已 supersede 的 ADR 只保留历史价值；
3. 当前行为以规范与已接受 ADR 的最新一致集合为准。

### 操作文档

`runbooks/` 只描述执行、验证、回滚和升级路径。它们不得引入新的证书、状态或权限模型。

### 生成视图

下列 Markdown 由 `verification/registry.json` 生成，便于人工阅读，但不是权威源：

- [`requirements/phase-0.md`](requirements/phase-0.md)
- [`gates/g0-checklist.md`](gates/g0-checklist.md)

## 4. 稳定标识

| 前缀 | 用途 | 示例 |
|---|---|---|
| `INV-*` | 架构/安全不变量 | `INV-CERT-01` |
| `REQ-Pn-*` | Phase requirement | `REQ-P0-031` |
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
- 改 Phase 状态：修改 `verification/registry.json`，运行 renderer；
- 改环境支持：修改 `supported-platform.md`，附 evidence；
- 改操作步骤：修改对应 runbook，不复制架构背景。

合并前至少执行：

```bash
node docs/verification/validate-registry.mjs
node docs/verification/render.mjs --check
node docs/verification/validate-markdown.mjs docs README.md
node docs/verification/validate-links.mjs docs README.md
pnpm diagrams
```

当机器 schema 发生变化时，还必须执行仓库的契约 clean-diff 检查。

## 6. 状态词汇

### Requirement

`OPEN`、`IN-PROGRESS`、`BLOCKED-INPUT`、`SATISFIED`、`FAILED`、`RETIRED`。

### Gate

`OPEN`、`BLOCKED-INPUT`、`PASS`、`FAIL`。

### 平台

`ARCH-FROZEN`、`ENV-PROPOSED`、`ENV-UNFROZEN`、`ENV-FROZEN`、`REJECTED`。

定义见 [`supported-platform.md`](supported-platform.md) 和 [`verification/README.md`](verification/README.md)。

## 7. 禁止的维护模式

- 在 Phase、Gate、runbook 中重新抄写完整安全规则；
- 在 README 中维护容易过期的 ADR 索引；
- 同时手工编辑 registry 与生成 Markdown；
- 用“文档已写”“文件存在”代替实现或测试证据；
- 用示例代码替代 production contract；
- 用单个全局状态词描述多个独立就绪维度；
- 用 `TBD`、`PASS` 或版本号掩盖未冻结输入；
- 创建 `common.md`、`misc.md` 等无明确变化原因的文档垃圾桶。
