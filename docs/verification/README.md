# Verification Registry

Phase 0 requirement、Gate、输入门禁和 Probe 状态的唯一事实源是：

- [`registry.json`](registry.json)

以下 Markdown 是生成视图，不应手工编辑：

- [`../requirements/phase-0.md`](../requirements/phase-0.md)
- [`../gates/g0-checklist.md`](../gates/g0-checklist.md)
- [`phase-0-status.md`](phase-0-status.md)

## 使用

```bash
node docs/verification/validate-registry.mjs
node docs/verification/render.mjs --check
node docs/verification/validate-markdown.mjs docs README.md
node docs/verification/render.mjs --write
node docs/verification/validate-links.mjs docs README.md
pnpm diagrams
```

Registry 使用 schema version 2。Requirement 侧的 `gates`、`probes` 和 `invariants` 是追踪关系的唯一来源；Gate 和 Probe 不再反向保存 requirement 列表。生成器按这些关系产生 Gate 视图，避免双向清单漂移。

`--check` 在生成内容与已提交文件不一致时返回失败。

## 状态规则

Requirement：

- `OPEN`：未开始或证据不足；
- `IN-PROGRESS`：正在实施，尚未满足；
- `BLOCKED-INPUT`：被未冻结环境/资产阻塞；
- `SATISFIED`：实现和证据已被接受；
- `FAILED`：已有证据证明不满足；
- `RETIRED`：需求已通过受审计决策终止，ID 不复用。

Gate：

- `OPEN`：未完成；
- `BLOCKED-INPUT`：依赖输入未冻结，不等于通过；
- `PASS`：完整 evidence 已被 reviewer 接受；
- `FAIL`：证据显示不满足。

Probe：

- `NOT-RUN`
- `RUNNING`
- `PASS`
- `FAIL`
- `BLOCKED-INPUT`

## 修改流程

1. 修改 `registry.json`；
2. 将 evidence locator 加入对应条目；
3. 运行 registry validator；
4. 运行 renderer；
5. 运行 Markdown、link 和 Mermaid validator；
6. 提交 registry 与生成 Markdown；
7. reviewer 验证 evidence，而不是只审状态词。

`SATISFIED`/`PASS` 条目必须有至少一个 evidence locator。总体 G0 只有 15 项 Gate 全部 `PASS` 且独立 `docs/gates/g0-decision.md` 已签署时才能改为 `PASS`。
