# ADR-0020: Repeatable contest configuration import

> Status: `ACCEPTED`
> Scope: Natsume V2 contest configuration import
> Supersedes: [ADR-0005](0005-csv-only-import.md)
> Superseded by: —

## Context

ADR-0005 将输入固定为 CSV，并在首次成功 commit 后永久冻结 Seat universe。同一 contest lifetime 内仍需要修正完整 Seat/account/password 集合（新增、删除、改账号/密码），永久 freeze 无法满足该需求。

CSV 含 password，因此 ordinary API、Browser、audit、log、metric、SSE 与 outbox 不得暴露 password-derived digest 或可作为离线猜测 oracle 的证据。Import 不得隐式驱动 Device I/O。

## Decision

1. **CSV-only 固定列（保留自 ADR-0005）**
   只接受 UTF-8（可带 BOM）文本 CSV；列必须恰好为 `seat,account,password`。不支持 XLSX/ODS、公式、列映射、自动猜测或 password export。

2. **可重复完整 candidate import（取代永久 freeze）**
   每个 CSV 都是完整 contest configuration candidate，不是增量 patch。Confirmed contest configuration 只能通过显式 Import Commit 被完整替换。

3. **统一 lifecycle**

   ```text
   upload
     -> encrypted staging
     -> strict parse
     -> immutable candidate import
     -> server-computed redacted diff
     -> explicit Import Commit
     -> baseline compare-and-swap
     -> atomic unbind-and-replace
   ```

4. **Server 权威 diff 与二次确认**
   Server 计算 redacted diff 并给出完整 taxonomy（见 domain model）。Import Commit 是二次确认动作，不新增独立 confirmation resource。确认使用 opaque Server-issued `preview_token`（普通 surface 无 password-derived 绑定），并校验签发时冻结的不可变 preview evidence：candidate identity、baseline `ContestConfigurationRevision`、完整 redacted diff、精确 binding impact 集合、actor/authz context 与 expiry。Commit 另需 `idempotency_key` 与 `correlation_id`，并重验当前 authorization。

5. **Baseline CAS、binding freshness 与 revision**
   初次 baseline 为 revision `0`/空集合。CAS 校验 `ContestConfigurationRevision`；另对 preview 授权的 binding impact 集合做精确相等校验（重算 `REMOVED` Seat 的当前绑定）。集合或 revision 任一不一致则 binding-stale/preview-mismatch，必须重新 preview。仅内容实际变化时提升 `ContestConfigurationRevision`；`is_noop`（无 ADDED/REMOVED/account/password 变更、无 INVALID、无 binding impact）仍记录 lineage/redacted audit，但不提升 contest configuration、credential 或 assignment revision，也不写内容变化 outbox。

6. **Atomic unbind-and-replace**
   删除已绑定 Seat 时，preview 列出并冻结全部 binding impacts；commit 在同一 Server transaction 中**仅**解绑该精确集合并提升 `AssignmentRevision`，再替换集合。保留 Seat code 的 binding 不变。Seat code 是身份；rename = `REMOVED + ADDED`，无 identity mapping。合法 account swap 允许；重复 account 与空/仅 header candidate 为 `INVALID` 且不可 commit。清空 confirmed configuration 不走 CSV import，仅通过独立 single-lifetime reset。

7. **Zero Device Command**
   Import 不创建 Operation/Command，不自动 `SYNC_STATE`/`SYNC_SECRET`，不产生 Device I/O。只改变 Server truth；Target/Drift 变化不代表 Device 已同步。

8. **最小持久化**
   只保留 current relational state、import lineage 与 redacted audit；preview evidence 签发后不可变；不设计完整历史 snapshot/rollback 产品。

权威语义以 [domain-model.md](../domain-model.md) 为准。

## Alternatives

- **永久 freeze（ADR-0005）**：无法在同一 lifetime 修正完整配置；已 supersede。
- **Incremental patch CSV**：破坏“完整 candidate”可审计性，增加部分应用与排序语义。
- **Reject-until-manual-unbind**：操作负担高，且无法保证 preview 与 commit 之间的原子一致性。
- **额外 confirmation resource**：扩大状态机与 API 面，无必要；Import Commit 本身即二次确认。
- **完整 snapshot history / rollback 产品**：超出 single-lifetime 最小领域（见 ADR-0009）。
- **Seat rename mapping**：引入第二身份与迁移规则，复杂且易错；rename 用 `REMOVED + ADDED` 表达。
- **自动 Device sync**：把领域 transaction 与远端可用性耦合；与 ADR-0013 冲突。

## Consequences

### Positive

- 可在同一 contest lifetime 内修正完整 Seat 集合与凭据；
- CSV 契约保持简单、可 fuzz；
- impact review + CAS + atomic unbind 降低误替换与部分失败风险；
- secret surface 保持最小；Device 同步仍为显式意图。

### Negative / trade-offs

- 大范围替换必须经过清晰 impact review；
- 删除 bound Seat 会立即改变 Server binding/Target，可能产生 Drift，仍需后续显式同步；
- 上游表格仍须先转换为固定列 CSV；
- 无完整 historical snapshot 产品回滚。

## Evidence and revisit trigger

- Domain、contracts、security、Phase 2 与 CSV runbook 必须一致描述 complete candidate、opaque preview token、不可变 preview evidence、baseline CAS、binding freshness、atomic unbind-and-replace、account 唯一性/空 candidate 策略、`is_noop` 与 zero Command。
- 测试覆盖 first/no-op/material/invalid（含重复 account 与空 candidate）/stale baseline/binding-stale/expiry/idempotency/unbind 精确集合/secret redaction。
- 仅当获批准的产品需求要求多格式输入、历史 snapshot 回滚或自动同步时重新打开；不得悄悄恢复永久 freeze。

## References

- [domain-model.md](../domain-model.md)
- [architecture.md](../architecture.md)
- [contracts.md](../contracts.md)
- [security-recovery.md](../security-recovery.md)
- [phase-2-csv-preparation.md](../implementation/phase-2-csv-preparation.md)
- [csv-import.md](../runbooks/csv-import.md)
- [ADR-0005](0005-csv-only-import.md)
- [ADR-0009](0009-single-lifetime-minimal-domain.md)
- [ADR-0013](0013-explicit-state-and-secret-commands.md)
