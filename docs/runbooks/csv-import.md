# CSV Import

> 适用：将完整 `seat,account,password` candidate 提交为 confirmed contest configuration（首次与后续相同 lifecycle）
> 关键不变量：`INV-SECRET-01`、`INV-STATE-01`、`INV-SECRET-02`
> 权威语义：[领域模型 §4.2](../domain-model.md)、[contracts §3.4](../contracts.md)、[ADR-0020](../adr/0020-repeatable-contest-configuration-import.md)

Import 只改变 Server truth。它不创建 Operation/Command，不自动 `SYNC_STATE`/`SYNC_SECRET`，也不表示 Device 已同步。

## 1. 前提

- 操作员拥有 CSV Import Commit 权限；
- 输入来源和版本已确认；
- 文件为 UTF-8，可含 BOM；
- 列恰好为 `seat,account,password`；
- 不在聊天、工单或普通共享盘复制真实密码；
- Server vault、数据库备份和审计可用；
- 已理解本次可能的 binding impact（删除已绑定 Seat 将在 commit 时 atomic unbind）；
- 空/仅 header candidate 不能用于清空全部 Seat；破坏性清空仅走 [Single-lifetime reset](single-lifetime-reset.md)；
- 后续 bind/sync/remediation 均为独立人工动作，不在本流程自动发生。

## 2. 上传和解析

1. 在 Preparation Center 创建 staging；
2. 上传单个 CSV；
3. 确认 Server 返回 `import_id`、行数和非秘密 validation summary；
4. 检查 parse/structural 错误；存在阻断时修正源文件并创建新 candidate，不得手工改库；
5. 空文件或仅 header、无有效数据行的 candidate 为 `INVALID`，`commit_allowed=false`，不得当作“清空全部 Seat”的手段；
6. 不截图或导出 password；
7. 不记录或转发 password-derived digest。

Upload/parse 不修改 confirmed contest configuration，也不联系 Device。Server 是分类与 validation 的唯一权威；UI 只渲染 Server 返回的结构化结果，不得本地重算/改写分类。

## 3. Preview 证据清单（commit 前强制）

确认 Server 返回并可审阅的 **immutable** redacted evidence（Server 是 diff 唯一权威；UI 只渲染结构化结果，不得本地重分类）。下列证据在 commit 前**全部**为强制门禁，缺一不得 commit：

- `import_id`；
- baseline `ContestConfigurationRevision`（空集合时为 `0`）；
- opaque `preview_token` 的非秘密引用（例如 token id / 绑定摘要，不含 secret material）；
- expiry（candidate 与/或 preview token）；
- 分类计数：`ADDED` / `REMOVED` / `ACCOUNT_CHANGED` / `PASSWORD_CHANGED` / `ACCOUNT_AND_PASSWORD_CHANGED` / `UNCHANGED` / `INVALID`；
- `is_noop`；
- `commit_allowed` 与 `blocking_reasons`；
- **`binding_impact_count`（必须显式出现，含 `0`）**；
- **当 `binding_impact_count > 0` 时，完整 impact 行全部可见并可逐条审阅**（Seat code、允许展示的 Device identity、当前 `AssignmentRevision`、动作 `UNBIND_ON_COMMIT`）；count 为 `0` 时仍须在清单中确认该零值；
- 任意 `INVALID` 行均阻止 commit——必须修复文件并创建新 candidate（含：重复 account、空/仅 header candidate 等）。

不存在“exact Seat set”硬错误：集合差异通过 `ADDED`/`REMOVED` 表达。合法 account swap 允许；candidate/confirmed 映射内重复 account 为 `INVALID`。

Preview evidence 在签发时刻冻结且不可变；不得把 `summary_json`/items 当作可原地更新的可变句柄。

## 4. Binding-impact 与 binding freshness

在 §3 已确认 `binding_impact_count`（含显式零）及（若 >0）完整 impact 行的前提下：

1. 逐条核对 binding impacts：Seat code、允许展示的 Device identity、当前 `AssignmentRevision`、动作 `UNBIND_ON_COMMIT`；
2. 确认 impact count 与预期一致；
3. 理解 commit 将在同一 Server transaction 中**仅**解绑 preview evidence 已授权的精确 impact 集合，再替换 confirmed configuration；
4. 理解保留 Seat code 的 binding 不变；rename = `REMOVED + ADDED`，无 identity mapping；
5. 由第二位 reviewer 联审集合变化与 unbind 范围（当 count > 0 时）。

**Binding freshness：** commit 时 Server 对 live binding 集合与各 `AssignmentRevision` 重算；若与 preview evidence 中冻结的精确集合不相等（新增/缺失 impact、Device identity 或 `AssignmentRevision` 变更等），commit **拒绝**（binding-stale / preview-mismatch），confirmed truth 不变。Operator **必须重新 preview** 后再决定是否 commit；不得沿用旧 `preview_token`。

无 binding impact 时仍须确认清单中的 `binding_impact_count = 0`，以及 `is_noop` 或 material 分类符合预期。

## 5. Import Commit（二次确认）

1. 确认 candidate 未过期、未 discard，且 `preview_token` 仍对应当前审阅的 immutable evidence；
2. 确认 `baseline_revision` 仍为审阅时的 confirmed revision（CAS）；
3. 确认 §3 强制证据已完整审阅（含 `binding_impact_count` 显式零或完整 impact 行）；
4. 对**完整显示的** preview 与 binding-impact 内容执行显式 Import Commit（二次确认）；
5. 提交时携带 `import_id`、`preview_token`、`baseline_revision`、`idempotency_key`、`correlation_id`；
6. 等待 Server domain transaction 完成；
7. 不得在 UI 断线后盲目重复不同语义的 commit；幂等重试仅使用相同 `idempotency_key` 与相同语义输入。

No-op（`is_noop = true`）仍需显式 commit 与 audit/lineage，但不提升 contest configuration、credential 或 assignment revision，**不写内容变化 ChangeEvent/outbox**（证据模板中 `OUTBOX_EVIDENCE=N/A`）。

始终 `AUTO_COMMAND_COUNT = 0`：不创建 Operation/Command，不自动 `SYNC_STATE`/`SYNC_SECRET`。

## 6. 自愿中止 / Discard

Operator 在 preview 之后、commit 之前决定放弃（含已接触 password-bearing candidate 的场景）时，**必须**走显式 discard，不得静默离开并假定安全：

1. 对当前 `import_id` 执行 discard（Preparation Center 或等价 operator API）；
2. 确认 candidate 进入 **terminal discarded** 状态，且不可再 commit；
3. **禁止**复用该 candidate 的 `preview_token`（含任何缓存的 token 引用）；若需继续，必须重新 upload 并重新 preview，取得新 `import_id` / 新 token；
4. 确认 **confirmed contest configuration、binding 与相关 revision 均未改变**；
5. encrypted staging 的清理按 Server 策略（expiry/cleanup job 或 discard 触发的清理）执行；不得手工改库或拷贝 staging ciphertext。

Discard / 自愿中止本身不产生 Device I/O，也不写内容变化 outbox。

## 7. 结果验证

成功后核对：

- confirmed `ContestConfigurationRevision`：material 变化应提升；no-op 不提升；
- 受 unbind 影响的 `AssignmentRevision` 已提升；未受影响 binding 不变；
- 仅实际 password 变化提升 `CredentialRevision`；
- redacted AuditEvent 可追踪；
- **material** 时 ChangeEvent/outbox 可追踪；**no-op** 时无内容变化 outbox（`OUTBOX_EVIDENCE=N/A`）；
- `AUTO_COMMAND_COUNT = 0`（无自动 `SYNC_STATE`/`SYNC_SECRET`/Operation/Command）；
- Target/Drift 至多反映新 Server truth，**不代表 Device 已更新**；
- 密码与 password-derived digest 未出现在 API response、browser storage、日志、审计、metric、SSE、outbox 或导出。

后续 bind、显式 state/secret sync 或 remediation 按独立 runbook 人工执行，见 [Explicit State and Secret Sync](explicit-state-and-secret-sync.md)。

## 8. 失败矩阵

| 情况 | 行为 |
|---|---|
| parse / invalid candidate（含任意 `INVALID` 行、重复 account、空/仅 header candidate） | 修正源文件，创建新 candidate；confirmed truth 不变；空 candidate 不得用于清空全部 Seat |
| stale baseline | 重新读取当前 confirmed revision，重新 preview，再决定是否 commit |
| binding stale（live binding 集合或 `AssignmentRevision` 与 preview evidence 不等） | commit 拒绝；重新 preview；不得沿用旧 token |
| expired candidate / expired preview token | 重新 upload/preview；不得沿用过期 token |
| discarded candidate | 创建新 candidate；旧 import 不可 commit；旧 preview token 禁止复用 |
| preview token mismatch | 停止；重新 preview 获取与当前 actor/baseline/diff/binding 绑定的 token |
| idempotency conflict | 相同 key 绑定了不同语义输入——停止并调查；不得强行覆盖 |
| authorization failure | 停止；确认 RBAC/session 后重新 preview |
| transaction failure | 同一 domain transaction **原子回滚**，旧 confirmed truth 完整保留；保存 correlation；按需重试。**不提供** historical configuration 产品级 rollback |
| UI disconnect | 按 `import_id` / correlation 查询结果，不盲目重复不同语义 commit |
| secret leakage 怀疑 | 停止、隔离 artifact、按安全事件处理 |

不得直接编辑数据库、credential revision、assignment revision 或 staging ciphertext。

清空全部 confirmed Seat / 破坏性 empty state **不在本 runbook**；见 [Single-lifetime reset](single-lifetime-reset.md)。

## 9. 成功判定

- import lineage（`import_id`、baseline/new revision、correlation）可追踪；
- confirmed contest configuration 与 preview 一致（no-op 时内容不变且 revision 规则正确）；
- binding impact 已按 commit 结果落实，或清单中确认 `binding_impact_count = 0`；
- 秘密未进入普通 surface；
- 无自动远端副作用（`AUTO_COMMAND_COUNT = 0`）；
- operator 理解后续 Device 同步为独立显式动作。

## 10. Evidence

```text
IMPORT_ID=
BASELINE_REVISION=
NEW_REVISION=
PREVIEW_TOKEN_REF=
IS_NOOP=YES|NO
CATEGORY_COUNTS=
BINDING_IMPACT_COUNT=0|#   # 必须显式填写，含 0
UNBIND_DEVICE_IDS=          # count=0 时可空
COMMIT_CORRELATION=
AUDIT_EVENT=
OUTBOX_EVIDENCE=            # material: 可追踪引用；no-op: N/A
AUTO_COMMAND_COUNT=0
TARGET_DRIFT_NOTE=SERVER_TRUTH_ONLY
DISCARD_PATH_USED=YES|NO|N/A
OWNER=
REVIEWER=
```
