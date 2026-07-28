# Phase 2 — CSV Preparation

> 计划：W9–W12
> 入口：G1 PASS
> 退出：G2
> 权威语义：[领域模型 §4.2](../domain-model.md)、[contracts §3.4](../contracts.md)、[ADR-0020](../adr/0020-repeatable-contest-configuration-import.md)

## 1. 目标

实现一个严格、可预览、秘密不外泄的 CSV 准备流程，把完整 contest configuration candidate 转换为 Server truth，而不产生任何自动远端副作用。

失败路径仅保证 **domain transaction 失败时原子回滚**（confirmed truth 不变）。**不包含** historical configuration 产品级 rollback，也不提供“可回滚到任意历史快照”的产品能力。

## 2. 工作包

### P2.1 Upload and staging

- 单文件；
- UTF-8/BOM；
- size/row/field limits；
- encrypted staging；
- owner/expiry/cleanup；
- 上传中断恢复；
- secret-safe errors。

### P2.2 Parser and normalization

精确列：

```text
seat,account,password
```

- header/extra/missing column；
- duplicate Seat；
- **candidate 内 duplicate account → `INVALID`**（不得依赖底层 UNIQUE 才失败）；
- **空文件 / 仅 header、无有效数据行 → `INVALID`，`commit_allowed=false`**（不可经 CSV import 清空全部 Seat）；
- 空值/长度/字符规则；
- normalization；
- no XLSX/ODS/formula/mapping；
- table-driven/property tests。

### P2.3 Preview

Server 计算 redacted import diff（taxonomy 权威见 [领域模型 §4.3](../domain-model.md)）；**Server 是分类唯一权威**，Web Panel 只渲染 Server 结构化结果，不得本地重分类：

- `ADDED` / `REMOVED` / `ACCOUNT_CHANGED` / `PASSWORD_CHANGED` / `ACCOUNT_AND_PASSWORD_CHANGED` / `UNCHANGED` / `INVALID`；
- 汇总：分类计数、`is_noop`、`commit_allowed`、`blocking_reasons`；
- **`binding_impact_count`（含显式 `0`）** 与完整 binding impact 行（count > 0 时强制）：Seat code、允许展示的 Device identity、当前 `AssignmentRevision`、动作 `UNBIND_ON_COMMIT`；
- baseline `ContestConfigurationRevision`（空 confirmed configuration 时为 `0`）；
- opaque `preview_token` 与 **immutable preview evidence**（签发时刻冻结）：candidate identity、baseline revision、完整 redacted diff、精确 binding impact 集合、actor authorization context 与 expiry；`summary_json`/等价 evidence 签发后不得原地突变。

任意 `INVALID` 行阻止 commit（含 duplicate account、空/仅 header candidate）。不存在 `SEAT_SET_MISMATCH` 硬错误：集合差异通过 `ADDED`/`REMOVED` 表达。合法 **account swap** 在 preview/commit 路径上允许（DB/transaction 须采用 clear-then-apply 或等价顺序，避免非 deferrable UNIQUE 误伤合法 swap）。Preview 不修改 Server truth，也不联系 Device。普通 surface 不暴露 password 或 password-derived digest（含 fingerprint/length）。

### P2.4 Import Commit

- Import Commit 即二次确认；不新增独立 confirmation resource；
- **幂等预检是 step 0**，先于 authorization/evidence/CAS/binding freshness 与任何 mutation：已 `COMMITTED` 且同 key/同语义 body 直接返回存储结果、零副作用；同 key 不同 body 返回 conflict；只有首次非 replay 执行进入后续 live validation；
- 绑定 `import_id`、opaque `preview_token`、`baseline_revision`（CAS）、`idempotency_key`、`correlation_id`；
- actor 从 Server auth/RBAC context 获取并在 transaction 内重验；
- commit 重算 live binding impact 集合；与 preview evidence 精确集合（含 Device identity 与 `AssignmentRevision`）不等则 **binding-stale reject**，须重新 preview；**仅**可解绑 preview 已授权的 impact 集合；
- material commit：baseline CAS + binding freshness → atomic unbind-and-replace（先解绑将被删除 Seat 上的 Device 并提升 `AssignmentRevision`）→ 完整替换 confirmed contest configuration → 仅实际 password 变化提升 `CredentialRevision` → 仅内容实际变化提升 `ContestConfigurationRevision` → redacted AuditEvent / 内容变化 ChangeEvent/outbox；
- no-op（`is_noop = true`）仍需显式 commit 与 lineage/redacted audit，但不提升 contest configuration、credential 或 assignment revision，**不写内容变化 outbox**（证据 `OUTBOX_EVIDENCE=N/A`），不制造内容变更导致的 Target churn；
- stale baseline、binding stale、expired/discarded candidate、preview token mismatch、invalid blocker、transaction failure、UI disconnect 均不改变 confirmed truth；
- **transaction failure 仅原子回滚当前 domain transaction**；无 historical configuration 产品级 rollback；
- material 成功已发生 unbind/revision bump 后，同 key/同 body 重试仍在 step 0 返回原成功结果，不得因 live baseline/binding 已变化而误报 stale 或 binding-stale；
- 始终 `AUTO_COMMAND_COUNT = 0`：不创建 Operation/Command，不自动 `SYNC_STATE`/`SYNC_SECRET`，不产生 Device I/O。

### P2.5 Preparation Center

- upload；
- validation summary；
- 展示全部 required preview evidence（分类计数、`is_noop`、`commit_allowed`/`blocking_reasons`、**`binding_impact_count` 含显式零**、count>0 时完整 impact 行、baseline revision、expiry、opaque preview token identity 的非秘密引用）；
- 对完整显示内容的二次确认（Import Commit）；
- commit progress；
- **显式 voluntary discard**：discard `import_id` → 确认 terminal discarded → 禁止复用 preview token → confirmed truth 不变 → encrypted staging 按策略清理；
- no password echo；
- no browser persistence of secret material；
- accessible large table；
- recovery from session/network interruption。

不在本设计文档中规定具体 widget 实现。Web Panel 不得本地重算 diff 分类；只渲染 Server 权威结果。

### P2.6 Export

只允许非秘密导出，例如 Seat/account/current binding/metadata。任何密码、ciphertext、private key、password-derived digest 或 recovery material 禁止导出。

## 3. 交付物

- parser/preview/commit；
- encrypted staging；
- Preparation Center；
- secret-safe API/OpenAPI；
- property/fuzz fixtures；
- audit/export；
- G2 evidence。

## 4. Definition of Done

- malformed、duplicate Seat、**duplicate account → INVALID**、extra columns、BOM、size limits 全覆盖；
- **空/仅 header candidate → INVALID 且 `commit_allowed=false`**（不可 wipe 全部 Seat；清空走 single-lifetime reset）；
- **合法 account swap** 可成功 commit（transaction 顺序正确）；
- first import（empty baseline `ContestConfigurationRevision = 0`）；
- no-op Import Commit（lineage/audit，无 revision 提升、**无内容变化 outbox**，`OUTBOX_EVIDENCE=N/A`）；
- material add/remove/account/password 变化；
- bound Seat remove 的 binding impact 可见（count 含显式零；>0 时完整行）且 atomic unbind-and-replace；
- **immutable preview evidence** 签发后不可变；commit 与 stored evidence 相等校验；
- stale baseline CAS reject 后须重新 preview；
- **binding-stale reject**（live binding 集合或 `AssignmentRevision` 与 preview evidence 不等）后须重新 preview；
- expiry / **voluntary discard**（terminal discarded、token 不可复用、confirmed truth 不变）/ preview token mismatch；
- idempotent retry：step 0 先于 stale/CAS/binding checks；material 成功后的同 key/同 body 重试返回原结果且不重复 unbind/revision/outbox；
- **transaction failure 原子回滚**（非 historical rollback 产品）；
- secret scan（password 与 password-derived digest/fingerprint/length 不进入 API/log/audit/metric/SSE/outbox/Browser）；
- browser storage inspection；
- unchanged password 不增加 `CredentialRevision`；
- commit 不创建 Operation/Command（`AUTO_COMMAND_COUNT = 0`）；
- Target/Drift 只在领域提交后反映新 Server truth，不代表 Device 已同步；
- G2 decision 签署。

## 5. 非目标

- 自动 Device sync / 因 import 创建 Command；
- Device 在线要求；
- XLSX/ODS；
- password export；
- 可配置列映射；
- 多 Event import；
- 完整 historical configuration snapshot / **产品级 historical rollback**；
- 经 CSV import 清空 confirmed configuration（仅 single-lifetime reset）；
- 额外 confirmation resource 或 Seat identity mapping；
- Web/本地 diff 重分类权威。
