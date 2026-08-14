# ADR-0031: Contest import and secret-evidence scope

> Status: `ACCEPTED`
> Scope: contest-configuration import, preview/commit concurrency, current-fact（当前事实） replacement, and password-derived evidence
> Consolidates: ADR-0005, ADR-0020, ADR-0028
> Supersedes: consolidated historical records; see [`history-map.md`](history-map.md)
> Superseded by: —

## Context

Venue 输入只有 Seat、account 和 password。支持 spreadsheet、公式、列映射或自动推断会扩大 parser、供应链、preview、错误与秘密处理面。

首次导入后永久冻结 Seat universe 无法支持同一赛事内的必要修正；增量 patch 又会引入顺序、部分应用和回滚语义。确认配置因此只表示**当前** Seat 集合、Seat→Account mapping 与当前密码材料；不保留通用 instance state、历史 Seat universe、历史 mapping 或历史秘密行。可审计性来自受限的 redacted `AuditEvent`，不是从可恢复的业务快照取得。

[ADR-0030](0030-foundation-deployment-and-delivery-baseline.md) F8/F9 说明导入不存在真实并发操作员，审计仅供受信管理员使用，因此一个全局 pending candidate 加低成本双 CAS 足够；无需扩展并发工作流或无消费者的秘密派生证据分类。

## Decision

### 输入与单一候选

- 只接受 UTF-8 CSV（可带 BOM），列严格为 `seat,account,password`。
- 不接受额外列、XLSX/ODS、公式、可配置列映射、自动推断或 password export。
- 每次 upload 都是完整 contest-configuration candidate，不是增量 patch。
- password-bearing 输入只进入加密 staging 和 secret-safe commit path；只有严格解析成功后才创建 candidate。invalid upload 不落库。
- 全局最多一个加密 pending candidate。它由 `pending_import_candidate` singleton row 表示：row 存在即为 pending，且只保存 `candidate_id`、`expires_at`、`baseline_configuration_revision`、`baseline_binding_revision`、`preview_token_hash`、`payload_vault_record_id` 与 `redacted_preview_json`；不使用 `state` 枚举、workflow history 或 import snapshot 表。
- pending 期间 candidate 内容不变。存在 pending row 时拒绝第二个 upload，操作员必须显式 discard 或完成现有 candidate。
- commit、discard 与 expiry 都在各自的原子事务中删除 `pending_import_candidate` row 和其 `payload_vault_record_id` 所引用的 `server_vault_records` row，并留下 redacted audit lineage。候选终止后没有可寻址的 candidate 或 password-bearing 数据继续保留。

### Preview 与 Commit

- Server 独占 redacted diff 分类；Client 只展示 typed 结果，不自行重算。
- 普通 surface 只获得 opaque `preview_token`、`candidate_id`、baseline `configuration_revision`、baseline `binding_revision`、redacted diff 与过期信息，不获得 password；数据库只保存该 token 的 `preview_token_hash`。
- Import Commit 是高影响变更的第二次显式确认；commit 时重新检查授权。
- Commit 对 `revision_counters` singleton 的 `configuration_revision` 与 `binding_revision` 做双 CAS。前者保护 Seat 集合与 Seat→Account mapping；后者保护全局 Seat↔Device 当前 Binding 集合。密码写入不由该 CAS 保护，而由全局单一 pending candidate 串行化。stale、expired、discarded、unauthorized 或事务失败时，不得修改 confirmed configuration、binding、Target 或相关 revision；操作员必须重新 preview。
- candidate 的存在性、双 CAS 与 redacted audit 是恢复和冲突边界。

### 当前状态、替换与修订

- `seats` 只保存当前 Seat 身份。Seat collection 不冻结；Seat code rename 表示 `REMOVED + ADDED`，不建立 rename mapping。
- `accounts` 只保存 `account_id`、`domjudge_username`、`credential_vault_record_id` 与 `credential_revision`。关联的 `server_vault_records` current row 只有 `vault_record_id`、`record_type`、`subject_id`、`nonce` 与 `ciphertext`；已提交的 Import Commit 无条件以新 nonce 替换当前密文、推进该 Account 的 `credential_revision` 并写 redacted audit，不做明文比较，也不保留旧密码版本。
- `account_mappings` 只表达当前 Seat→Account mapping：每个 Seat 最多一条、每个 Account 最多属于一个 Seat；没有 row 表示该 Seat 当前无 Account。它属于 confirmed contest configuration；mapping 变化受 `revision_counters.configuration_revision` 保护，而不是 Binding。
- `device_bindings` 同样只表达当前 Seat↔Device 关系，row 包含 `seat_id`、`device_pk` 与 `binding_revision`。每次 Binding 集合实际变化（bind、unbind、rebind，或 import 删除已绑定 Seat）以 CAS 推进一次全局 `BindingRevision`（`revision_counters.binding_revision`）；受影响的新增/变更 Binding 记录该值，未变化 Binding 不重写。仅 Account mapping 或密码变化而 Binding 保持时不得推进 `BindingRevision`。
- material Import Commit 在一个 Server transaction 中替换当前配置、执行必要的 unbind-and-replace，并且每个 revision 在一个事务内最多递增一次。只有 Seat 集合或 Seat→Account mapping 实际变化才推进 `revision_counters.configuration_revision`，密码内容不参与；discard、expiry 与失败不推进任一 revision，而**任何已提交的 import 都推进新确认配置中每个 Account 的 `credential_revision`**。
- 把密码内容移出 CAS 保护范围不损失并发保护：密码的唯一写入方就是 Import Commit 本身，而全局单一 pending candidate 已经把 preview→commit 串行化——存在 pending candidate 时第二次 import 无法提交；single-lifetime reset 会删除 candidate 并清零计数器，使 CAS 无论如何都会失败。
- 允许有效 account swap，拒绝 duplicate account、空候选和只有 header 的候选。清空 confirmed configuration 不是 import；必须使用独立的 destructive single-lifetime reset。
- 「no-op」自此只在非秘密维度上定义：不推进 configuration 或 binding revision、不制造 Target churn，只保留 lineage 与 redacted audit。它**不**表示 credential 未变——已提交的 import 始终推进 credential revision。

### Effect、审计与秘密边界

- Import 只改变 Server truth；不得创建 Command、自动发出 `SYNC_STATE`/`SYNC_SECRET`、产生 Device I/O 或暗示 Device 已同步。
- password plaintext、private key、Device Token、原始 CSV 行不得进入普通 API、日志、指标、audit、export、Web 持久化、Target 或 Observed。
- 每个 upload、preview、commit、discard、expiry/reject 与 material binding impact 都由同一个 guarded operation 在同一 transaction 内插入 redacted `AuditEvent` 和业务 mutation；fresh `audit_event_id` 可作为 typed operation input，但已持久化的同 ID 或预插入 audit row 不能重放为新 mutation 的依据。`audit_events` 只包含 `audit_event_id`、`occurred_at`、`actor`、`action_kind`、`resource_type`、可选 `resource_id`、`result`、可选 `reason_code`、`correlation_id`、可选 `group_correlation_id` 与 typed object `redacted_detail_json`；相关 revision、受影响数量和事件专用 redacted evidence 都放入该 JSON，不记录秘密、CSV 原文或未脱敏错误链。
- password-derived digest、length、fingerprint 或 raw CSV hash 不再作为单独禁止类别维护；工程默认仍是不产生、不暴露，除非存在明确消费者和重新评审。
- 删除/替换旧秘密或 pending payload 是删除可寻址数据库事实，不承诺 SQLite page、WAL、backup 或存储介质上的取证级物理擦除；介质处置属于 reset/recovery runbook。

## Alternatives

- XLSX/ODS 或可配置列映射：引入公式、格式和额外 UI/validation 分支。
- 首次导入后永久 freeze：阻止必要修正，并迫使存在无消费者的 Seat-universe 状态。
- incremental patch、Seat rename mapping 或 full snapshot rollback：扩大身份与历史模型。
- 以 candidate state/history、历史 credential/mapping 行或 vault supersession 表保存可恢复秘密：扩大秘密保留面，且没有产品消费者。
- 无 preview 的直接导入：缺少明确审查与确认。
- 先手工 unbind 再导入：无法提供 preview-to-commit 的原子性。
- automatic Device sync：把 Server transaction 与远端可用性和秘密授权耦合。
- 通用多操作员冲突模型、额外 browser replay 基础设施或事件流：当前部署没有相应消费者，却增加持久状态与失败语义。

## Consequences

### Positive

- 输入面小、确定、适合严格验证和 fuzzing。
- 完整替换、redacted preview、双 CAS 与原子 commit 防止部分或静默 stale 变更。
- 当前事实和必要安全证据分离；终止候选及旧秘密不会成为可查询历史。
- 导入与 Device 可用性、Command 和秘密同步保持解耦。

### Negative / trade-offs

- 上游 spreadsheet 必须转换为固定 CSV。
- 删除已绑定 Seat 会产生需要操作员理解的影响和后续 Drift。
- 同一时间只能审查一个 candidate；冲突、过期或未知结果后必须重新 preview。
- 没有产品级历史 snapshot rollback，也不提供取证级 SQLite 物理擦除承诺。
- preview 不再能揭示密码内容是否真的变化，操作员无法在 commit 前发现上游误重新生成了整列密码。接受该代价的前提是运行模型本就要求每次 import 之后对整个 fleet 重新同步秘密。
- 每次已提交的 import 都产生 fleet 范围的 credential-stale Drift；这同时也是操作员执行批量 `SYNC_SECRET` 的提示信号。

## Acceptance basis and revisit trigger

实现证据必须覆盖 pending mutual exclusion、invalid upload 零落库、candidate/payload 的 commit/discard/expiry 终态删除、所有拒绝路径零 confirmed-state 变更、双 CAS、material/no-op transaction、全局 Binding-set `binding_revision` 语义、plaintext redaction、审计 envelope 原子性和零自动 Device effect。

出现真实并发导入、外部审计消费者、额外输入格式、snapshot rollback、历史秘密保留或自动同步需求时重开；不得通过给现有流程添加零散例外来恢复已移除的复杂度。

## Normative sources

- [Architecture](../architecture.md)
- [Domain model](../domain-model.md)
- [Contracts](../contracts.md)
- [State and execution](../state-and-execution.md)
- [Security and recovery](../security-recovery.md)
- [ADR-0030](0030-foundation-deployment-and-delivery-baseline.md)
