# ADR-0031: Contest import and secret-evidence scope

> Status: `ACCEPTED`
> Scope: contest-configuration import, preview/commit concurrency, current-fact（当前事实） replacement, and password-derived evidence
> Consolidates: ADR-0005, ADR-0020, ADR-0028
> Supersedes: consolidated historical records; see [`history-map.md`](history-map.md)
> Superseded by: —

## Context

Venue 输入只有 Seat、account 和 password。支持 spreadsheet、公式、列映射或自动推断会扩大 parser、供应链、preview、错误与秘密处理面。

首次导入后永久冻结 Seat universe 无法支持同一赛事内的必要修正；增量 patch 又会引入顺序、部分应用和回滚语义。确认配置因此只表示**当前** Seat 集合、Seat→Account mapping 与当前密码材料；不保留通用 instance state、历史 Seat universe、历史 mapping 或历史秘密行。可审计性来自受限的 redacted `AuditEvent`，不是从可恢复的业务快照取得。

[ADR-0030](0030-foundation-deployment-and-delivery-baseline.md) F8/F9 说明导入不存在真实并发操作员，审计仅供受信管理员使用，因此一个全局 pending candidate 足够（preview→commit 串行化只靠该 singleton；Import 不对任何 revision 做 CAS）；无需扩展并发工作流或无消费者的秘密派生证据分类。

## Decision

**2026-08-20 修订（Import 不修改 Binding，且取消 revision CAS）**：Import Commit 零 `device_bindings` 写入、不铸造 `binding_id`。将删除且当前仍绑定的座位使 commit 返回 `IMPORT_SEATS_STILL_BOUND` 且零写入；operator 经 Binding API 解绑后再 preview。已删除 `revision_counters` 表，不存在全局 configuration 或 binding-set clock。Import Commit 不对任何 revision 做 CAS；preview→commit 串行化只靠 singleton pending candidate（存在 pending 时第二次 upload 被拒绝；single-lifetime reset 删除 candidate）。`pending_import_candidate` 不再保存 `baseline_configuration_revision` 或 `baseline_binding_revision`。preview `binding_impacts[]` 仍是 redacted 预告（commit blocker），不是解绑计划。

**2026-08-20 修订（`binding_id` occupancy UUID）**：`device_bindings` 以 `seat_id` 为 PK（FK `seats`）、`device_id` NOT NULL UNIQUE（FK `devices`），并保存 `binding_id` TEXT NOT NULL UNIQUE——每次 bind 铸造的 canonical lowercase hyphenated UUIDv7 occupancy stamp，独立于 Seat。unbind 删除行，再次 bind 得到新 UUID。无 integer bump，无 `seats.binding_generation`。`observed_device_states.installed_binding_id` 为可空 TEXT，NULL 表示从未安装 Binding。`SyncState.binding_id` 与 `SyncSecret.binding_id` 是 occupancy string UUIDv7。`BindingResult` 不携带 occupancy `binding_id`。无 `TargetAssignment` 消息。Device 在当前行缺失或 `binding_id` 不同时拒绝 Command。Import 仍零 Binding 写入。

**2026-08-20 修订（Identifier `device_id`）**：原 `device_pk` 已原位更名为 `device_id`，同一 UUIDv7 surrogate；FK / unique 跟随此名。vault 仍以 `account_id` 为 PK，无 `import_payload`、无 `revision_counters`。Import 仍零 Binding 写入。

**2026-08-20 修订（preview 不持久化密码）**：删除 encrypted whole-CSV `import_payload` vault type。`accounts` 为父表，列为 `account_id`（canonical UUIDv7 PK）、`domjudge_username`（NOT NULL UNIQUE）、`credential_revision`（INTEGER NOT NULL `>= 1`）；无 `credential_vault_record_id`。`server_vault_records` 必须在 `accounts` 之后创建，列为 `account_id`（PRIMARY KEY REFERENCES accounts(account_id) ON DELETE CASCADE）、`nonce`、`ciphertext`；无 `vault_record_id`、`record_type`、`subject_id`。同一事务先插 Account 再插 vault；删除 Account 级联删除 vault。vault 不是独立 Identifier，按 `account_id` join。保留 `accounts.credential_revision`。`pending_import_candidate` 是非秘密 singleton 草稿：`candidate_id`、`expires_at_unix_ms`、`preview_token_hash`、`nonsecret_fingerprint_version`、`nonsecret_fingerprint_sha256`、`redacted_preview_json`；无 `payload_vault_record_id`。preview 零 confirmed 写入、零 vault 写入。密码只存在于本次 HTTP 请求体的解析期内，preview 结束后从内存丢弃，commit 时再次提交。commit 请求体是同一 CSV（`Content-Type: text/csv`，同一 `CSV_IMPORT_BODY_LIMIT_BYTES`），`preview_token` 走 header `Natsume-Preview-Token`，不得放入 query string。非秘密 fingerprint 不匹配返回 `409 IMPORT_CANDIDATE_MISMATCH`，零写入，candidate 保留至 discard/expiry/成功 commit。discard/expiry 只删除 pending 草稿行。

**2026-08-20 修订（持久化时刻）**：`pending_import_candidate.expires_at_unix_ms` 与 `audit_events.occurred_at_unix_ms` 均为 INTEGER UTC epoch milliseconds。HTTP preview/pending 的 `expires_at` 仍为 RFC 3339 UTC。无 RFC 3339 TEXT、无 `strftime` CHECK。`device_id`、`binding_id` UUID occupancy、vault `account_id` PK、无 `revision_counters`、无 `import_payload` 均保持。

### 输入与单一候选

- 只接受 UTF-8 CSV（可带 BOM），列严格为 `seat,account,password`。
- 不接受额外列、XLSX/ODS、公式、可配置列映射、自动推断或 password export。
- 每次 upload 都是完整 contest-configuration candidate，不是增量 patch。
- 密码只存在于 upload/commit 的 HTTP 请求体解析期内；preview 不写 vault，也不把 password-bearing 材料落入 pending 行。只有严格解析成功后才创建非秘密 candidate。invalid upload 不落库。解析完成后密码从内存丢弃。
- 全局最多一个非秘密 pending candidate。它由 `pending_import_candidate` singleton row 表示：row 存在即为 pending，且只保存 `candidate_id`、`expires_at_unix_ms`、`preview_token_hash`、`nonsecret_fingerprint_version`、`nonsecret_fingerprint_sha256` 与 `redacted_preview_json`；不使用 `state` 枚举、workflow history 或 import snapshot 表，也不保存 configuration/Binding baseline、encrypted CSV 或密码。
- pending 期间 candidate 的非秘密草稿不变。存在 pending row 时拒绝第二个 upload，操作员必须显式 discard 或完成现有 candidate。
- commit、discard 与 expiry 都在各自的原子事务中删除 `pending_import_candidate` row，并留下 redacted audit lineage。不删除 vault payload row（该 type 已不存在）。候选终止后没有可寻址的 candidate 继续保留；密码从未作为 pending 事实落库。

### Preview 与 Commit

- Server 独占 redacted diff 分类；Client 只展示 typed 结果，不自行重算。
- 普通 surface 只获得 opaque `preview_token`、`candidate_id`、redacted diff 与过期信息，不获得 password，也不返回任何 baseline configuration/binding revision；数据库只保存该 token 的 `preview_token_hash` 与非秘密 fingerprint。
- Import Commit 是高影响变更的第二次显式确认；commit 时重新检查授权，并再次提交同一 CSV。
- Commit 在同一 `BEGIN IMMEDIATE` 事务内：校验 `Natsume-Preview-Token`、重解析 CSV、按冻结算法重算非秘密 fingerprint 并与所存哈希常量时间比较。不一致 → `IMPORT_CANDIDATE_MISMATCH`，零写入，candidate 保留（operator 可用原文件重试）。将删座位仍绑定 → `IMPORT_SEATS_STILL_BOUND`。然后才应用 seats/mappings/Account vault ciphertext、删除 pending 行并审计。
- Commit 不对任何 revision 做 CAS。`seats` / `account_mappings` / Account 密码的唯一写入方是 Import Commit 本身；存在 singleton pending 时第二次 upload 被拒绝；single-lifetime reset 删除 candidate。这就是 preview→commit 锁。Import **零 `device_bindings` 写入**，不铸造 `binding_id`。expired、discarded、unauthorized、非秘密 fingerprint 不一致、仍有已绑定座位待删除、或事务失败时，不得修改 confirmed configuration、binding、Target 或相关 revision。fingerprint 不一致不要求重新 preview；其余拒绝路径操作员必须重新 preview。
- candidate 的存在性、非秘密 fingerprint、commit 时对将删座位的 Binding 再检查，与 redacted audit 是恢复和冲突边界。

### 当前状态、替换与修订

- `seats` 只保存当前 Seat 身份。Seat collection 不冻结；Seat code rename 表示 `REMOVED + ADDED`，不建立 rename mapping。
- `accounts` 只保存 `account_id`、`domjudge_username` 与 `credential_revision`。关联的 `server_vault_records` current row 只有 `account_id`（PK/FK，ON DELETE CASCADE）、`nonce` 与 `ciphertext`，每个 Account 至多一行当前 DOMjudge 密码；已提交的 Import Commit 无条件以新 nonce 替换当前密文、推进该 Account 的 `credential_revision` 并写 redacted audit，不做明文比较，也不保留旧密码版本。
- `account_mappings` 只表达当前 Seat→Account mapping：每个 Seat 最多一条、每个 Account 最多属于一个 Seat；没有 row 表示该 Seat 当前无 Account。它属于 confirmed contest configuration；mapping 变化由 Import Commit 写入，不是 Binding，也不经全局 configuration clock。
- `device_bindings` 同样只表达当前 Seat↔Device 关系，row 包含 `seat_id`、`device_id` 与 `binding_id`（每次 bind 铸造的 UUIDv7 occupancy stamp，供 SYNC_SECRET / Target / Observed 冻结）。**仅** bind / rebind 铸造新 `binding_id`；unbind 删除行；未变化 Binding 不重写。Import 与 Account mapping / 密码变化不得写入 Binding 或铸造 `binding_id`。
- material Import Commit 在一个 Server transaction 中替换当前配置。material / no-op 由 diff 判定：Seat 集合或 Seat→Account mapping 是否实际改变，而不是任何计数器；密码内容不参与该判定。CSV 将删除的座位若在 commit 时仍有 Binding，整单拒绝、零写入，稳定码 `IMPORT_SEATS_STILL_BOUND`；operator 必须先经 Binding API 解绑再重新 preview。discard、expiry 与失败不改变 confirmed configuration 或 `binding_id`，而**任何已提交的 import 都推进新确认配置中每个 Account 的 `credential_revision`**。
- 不设 revision CAS 不损失并发保护：`seats` / `account_mappings` / 密码的唯一写入方就是 Import Commit 本身，而全局单一 pending candidate 已经把 preview→commit 串行化——存在 pending candidate 时第二次 upload 被拒绝；single-lifetime reset 删除 candidate。
- 允许有效 account swap，拒绝 duplicate account、空候选和只有 header 的候选。清空 confirmed configuration 不是 import；必须使用独立的 destructive single-lifetime reset。
- 「no-op」自此只在非秘密维度上定义：seats/mappings 相对 confirmed 未变、不铸造 `binding_id`、不制造 Target churn，只保留 lineage 与 redacted audit。它**不**表示 credential 未变——已提交的 import 始终推进 credential revision。

### Effect、审计与秘密边界

- Import 只改变 Server truth；不得创建 Command、自动发出 `SYNC_STATE`/`SYNC_SECRET`、产生 Device I/O 或暗示 Device 已同步。
- password plaintext、private key、Device Token、原始 CSV 行不得进入普通 API、日志、指标、audit、export、Web 持久化、Target 或 Observed。
- 每个 upload、preview、commit、discard、expiry/reject 与「将删座位仍绑定」的拒绝都由同一个 guarded operation 在同一 transaction 内插入 redacted `AuditEvent` 和业务 mutation；fresh `audit_event_id` 可作为 typed operation input，但已持久化的同 ID 或预插入 audit row 不能重放为新 mutation 的依据。`audit_events` 只包含 `audit_event_id`、`occurred_at_unix_ms`、`actor`、`action_kind`、`resource_type`、可选 `resource_id`、`result`、可选 `reason_code`、`correlation_id`、可选 `group_correlation_id` 与 typed object `redacted_detail_json`；相关 revision、受影响数量和事件专用 redacted evidence 都放入该 JSON，不记录秘密、CSV 原文或未脱敏错误链。
- password-derived digest、length、fingerprint 或 raw CSV hash 不再作为单独禁止类别维护；工程默认仍是不产生、不暴露，除非存在明确消费者和重新评审。
- 删除/替换旧秘密是删除可寻址数据库事实，不承诺 SQLite page、WAL、backup 或存储介质上的取证级物理擦除；介质处置属于 reset/recovery runbook。pending 终止只删除非秘密草稿行，不涉及 vault payload。

## Alternatives

- XLSX/ODS 或可配置列映射：引入公式、格式和额外 UI/validation 分支。
- 首次导入后永久 freeze：阻止必要修正，并迫使存在无消费者的 Seat-universe 状态。
- incremental patch、Seat rename mapping 或 full snapshot rollback：扩大身份与历史模型。
- 以 candidate state/history、历史 credential/mapping 行或 vault supersession 表保存可恢复秘密：扩大秘密保留面，且没有产品消费者。
- 把整份 CSV 加密为 `import_payload` vault type、preview 阶段持久化密码：扩大秘密保留面，而 redacted preview 并不需要密码。**2026-08-20 起删除该 type**；preview 不写 vault，commit 再次提交同一 CSV。
- 无 preview 的直接导入：缺少明确审查与确认。
- Import 内 atomic unbind-and-replace：用全局 `binding_revision` 双 CAS 覆盖操作员空闲期，粒度过粗（无关 bind 会使密码-only commit stale），且让 CSV 隐式踢掉已绑定 Device。**2026-08-20 起改为拒绝直至手工 unbind**；commit 在 `BEGIN IMMEDIATE` 内再检查将删座位当前无 Binding，故空闲期内被重新绑定的座位仍会失败而不是解绑错机器。
- automatic Device sync：把 Server transaction 与远端可用性和秘密授权耦合。
- 通用多操作员冲突模型、额外 browser replay 基础设施或事件流：当前部署没有相应消费者，却增加持久状态与失败语义。

## Consequences

### Positive

- 输入面小、确定、适合严格验证和 fuzzing。
- 完整替换、redacted preview、singleton pending 串行化与原子 commit 防止部分或未确认变更；Binding 只经显式 bind API 变化。
- 当前事实和必要安全证据分离；终止候选及旧秘密不会成为可查询历史。
- 导入与 Device 可用性、Command 和秘密同步保持解耦。

### Negative / trade-offs

- 上游 spreadsheet 必须转换为固定 CSV。
- 删除仍绑定的 Seat 必须先解绑；commit 拒绝而不是静默踢设备，操作员要多一步 Binding API。
- 同一时间只能审查一个 candidate；冲突、过期或未知结果后必须重新 preview。
- 没有产品级历史 snapshot rollback，也不提供取证级 SQLite 物理擦除承诺。
- preview 不再能揭示密码内容是否真的变化，操作员无法在 commit 前发现上游误重新生成了整列密码。接受该代价的前提是运行模型本就要求每次 import 之后对整个 fleet 重新同步秘密。
- 每次已提交的 import 都产生 fleet 范围的 credential-stale Drift；这同时也是操作员执行批量 `SYNC_SECRET` 的提示信号。

## Acceptance basis and revisit trigger

实现证据必须覆盖 pending mutual exclusion、invalid upload 零落库、preview 零 vault 写入、candidate 的 commit/discard/expiry 终态删除（无 payload vault row）、fingerprint mismatch 零写入且 candidate 保留、所有拒绝路径零 confirmed-state 变更、将删座位仍绑定的零写入拒绝、material/no-op transaction、bind API 的 `binding_id` occupancy 语义、plaintext redaction、审计 envelope 原子性和零自动 Device effect。 Import Commit 不得写入 `device_bindings`，也不得对任何 revision 做 CAS。

出现真实并发导入、外部审计消费者、额外输入格式、snapshot rollback、历史秘密保留或自动同步需求时重开；不得通过给现有流程添加零散例外来恢复已移除的复杂度。

## Normative sources

- [Architecture](../architecture.md)
- [Domain model](../domain-model.md)
- [Contracts](../contracts.md)
- [State and execution](../state-and-execution.md)
- [Security and recovery](../security-recovery.md)
- [ADR-0030](0030-foundation-deployment-and-delivery-baseline.md)
