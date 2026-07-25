# Document Refactor — 2026-07-24

> 类型：文档结构变更  
> 架构语义来源：Natsume V2 v2.7 决策集合  
> Gate 影响：无；G0 保持 `OPEN`

## 1. 目的

原文档把产品范围、组件、协议、威胁、平台、测试、Gate、实施计划和 runbook 集中在一份大型设计文档中，同时在 Roadmap、Phase、Requirement、Gate、ADR 和 runbook 重复同一规则。结果是一个事实需要修改多处，并已出现 Session Agent bootstrap/descriptor 与 XDG direct model 的矛盾。

本次重构：

- 把不同变化原因拆成高内聚文档；
- 给每类事实指定唯一 Owner；
- 用稳定 `INV-*`、`REQ-*`、`G*` 和 `ADR-*` 引用；
- 将 Phase 0 状态集中到 JSON registry；
- 保留 registry 生成的人类可读视图，不保留旧兼容路径；
- 删除无效 README 入口；
- 补齐缺失 Probe/report/runbook 入口；
- 不改变任何未验证 Gate 为 PASS。

## 2. 新权威结构

```text
docs/
  README.md
  architecture.md
  domain-model.md
  contracts.md
  state-and-execution.md
  security-recovery.md
  repository-layout.md
  dependency-policy.md
  supported-platform.md
  roadmap.md
  glossary.md
  adr/
  implementation/
  verification/
  requirements/          generated view
  gates/                 generated view + decision template
  probes/
  lab/
  runbooks/
  reference/
  history/
```

## 3. 原集中式设计文档迁移

| 原主题 | 新 Owner |
|---|---|
| 产品范围、固定决策、组件 | `architecture.md` |
| Monorepo、进程与共享库 | `repository-layout.md` |
| 领域实体和关系 | `domain-model.md` |
| Enrollment、QUIC、D-Bus、ErrorCode | `contracts.md` |
| Target、Observed、Drift、Operation/Command | `state-and-execution.md` |
| Caddy、Session、Home 状态 | `state-and-execution.md` |
| 信任、秘密、PKI、vault、fail-closed | `security-recovery.md` |
| 技术依赖和 feature | `dependency-policy.md` |
| 平台/桌面/版本状态 | `supported-platform.md` |
| 容量、Phase、Gate | `roadmap.md` 与 Phase 文档 |
| 负向场景 | tests/probes |
| 操作步骤 | runbooks |
| 决策理由 | ADR |

v2.5 时期形成的设计脉络已纳入 v2.7 决策集合，并由上述权威文档和本记录继续追溯。

## 4. 不变量压缩

原本分散的约束压缩为 12 条：

- `INV-SECRET-01`
- `INV-INPUT-01`
- `INV-IDENTITY-01`
- `INV-IDENTITY-02`
- `INV-CERT-01`
- `INV-CERT-02`
- `INV-STATE-01`
- `INV-SECRET-02`
- `INV-COMMAND-01`
- `INV-PRIVILEGE-01`
- `INV-DATAPLANE-01`
- `INV-SESSION-01`

完整正文只存在于 `security-recovery.md`。Requirement、Gate、Phase、probe 和 runbook 只引用 ID。

## 5. Phase 0 状态

`docs/verification/registry.json` 是唯一机器事实源。它包含：

- 49 条 Phase 0 requirement；
- 15 项 G0；
- 7 项输入门禁；
- 6 份 Probe。

生成：

```bash
node docs/verification/render.mjs --write
```

校验：

```bash
node docs/verification/render.mjs --check
```

生成视图：

- `requirements/phase-0.md`
- `gates/g0-checklist.md`
- `verification/phase-0-status.md`

## 6. 迁移与生成视图

旧兼容入口已删除。当前阅读入口是 `docs/README.md`，权威路线图是 `roadmap.md`，Phase 工作包入口是 `implementation/README.md`。

| 路径或类别 | 处理 |
|---|---|
| `requirements/phase-0.md` | registry 生成 |
| `gates/g0-checklist.md` | registry 生成 |
| 原 ADR filenames | 原位重写、决策保持 |
| 原 Phase filenames | 原位重写 |
| 原 runbook filenames | 原位重写 |
| Session Agent reference | 保留为 non-normative scaffold |
| `validate-mermaid.mjs` | 保留并格式化 |

## 7. 已修复的问题

- 根 README 不再引用不存在的 `upstream-base.md`、`merge-report.md`、`validation-report.md` 或 `FILE_MANIFEST.*`；
- dependency policy 删除旧 bootstrap/environment descriptor 规则；
- ADR template 删除陈旧项目索引；
- Device/Gateway readiness 不再混为一个状态；
- Operation 不再强制包装所有 CRUD；
- ErrorCode 不进入 domain decision；
- platform special cases 改成 capability/evidence；
- Gate/Requirement 状态不再在多份 Markdown 手工同步；
- 补齐 Probe report 模板；
- 补齐 CSV、replacement、Home、backup/upgrade 和 rehearsal runbook；
- 增加 registry、生成文件、Markdown 结构和相对链接校验。

## 8. 有意未改变

- 单赛事实例；
- CSV-only；
- Device-only Enrollment；
- mandatory-mTLS QUIC；
- Gateway certificate in active `SYNC_STATE`；
- explicit state/secret；
- Observed source；
- application-encrypted vault；
- XDG direct Slint Agent；
- Session/Caddy decoupling；
- fixed Home backend at deployment；
- 当前 Phase 0/G0 OPEN；
- 当前平台和硬件仍未冻结。

## 9. 后续治理

- 变更事实只修改权威文档；
- 新安全约束先 ADR，再 `INV-*`；
- 新 requirement 修改 registry；
- 生成 Markdown 禁止手改；
- 机器 schema 变更与 Markdown 一起 clean diff；
- 每季度或每个 Gate 复核 broken link、orphan doc 和重复规范；
- 删除或迁移旧路径时，必须同步更新仓库内链接、测试断言和历史记录。
