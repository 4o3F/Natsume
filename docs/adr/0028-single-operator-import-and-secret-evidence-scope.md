# ADR-0028: Single-operator import model and secret-evidence scope

> Status: `ACCEPTED`  
> Scope: Natsume V2 contest configuration import 与 password-derived 证据脱敏范围  
> Supersedes: —（收窄 [ADR-0020](0020-repeatable-contest-configuration-import.md) 的并发防护与绑定校验条款；其余条款维持）  
> Superseded by: —

## Context

[ADR-0020](0020-repeatable-contest-configuration-import.md) 的并发防护（不可变 preview evidence 五元组、binding impact 精确集合相等重验、`idempotency_key`）假设多操作员并发提交冲突 CSV。按 [ADR-0022](0022-deployment-facts-and-trust-assumptions.md) F8：操作员 1–3 人、互相信任、**不存在并发导入场景**——单一互斥即可给出同等安全，精确集合重验保护的竞态被锁消除。

同理，"password length / fingerprint / raw CSV hash 一律不得出现"的禁令防的是离线猜测 oracle，其唯一受众（F8/F9）是本来就持有整份 CSV 的操作员与仅限内部的审计读者。该禁令产生大量规范文本与负向测试面，但没有对应的模型内攻击者。按规则，放宽 `INV-*` 必须有 ADR——即本 ADR。

## Decision

### Import 并发模型

1. **全局单 pending candidate**：同一时刻最多一个未提交 candidate；新 upload 需先显式 discard 旧 candidate。preview → commit 期间不需要防第二操作员。
2. **提交校验收敛为双 CAS**：baseline `ContestConfigurationRevision` CAS（同 ADR-0020）+ `AssignmentRevision` CAS（preview 签发后任何绑定变化 → 提交拒绝、重新 preview）。**取消 binding impact 精确集合相等重验**——单调 revision 比对给出相同结论。
3. **取消 commit `idempotency_key`**：双 CAS 已保证重复提交安全失败（第二次提交因 revision 前移而拒绝）；超时后未知结果的恢复路径统一为"重新 preview"。
4. 保留不变：完整 candidate 替换语义、opaque `preview_token`（绑定 candidate 身份、baseline revision、redacted diff、过期时间）、Server 权威 diff、`is_noop` lineage、atomic unbind-and-replace、zero Device Command、清空仅经 single-lifetime reset。

### 秘密证据范围（放宽 `INV-SECRET-01` 的 CSV 派生条款）

5. **保留的红线**：password 明文、private key、Device Token 值不进入任何普通 surface（API、日志、审计、指标、导出、Web 持久化）。
6. **取消的禁令**：password length、password fingerprint、raw CSV hash 及其他 password-derived digest 不再作为独立禁止类别维护，相应负向测试面取消。工程默认仍不主动输出这些值（无输出理由），但不再为其建规范条款与测试义务。
7. 若审计消费者范围扩大到外部方（F9 失效），本条必须重新评审。

## Alternatives

- **维持全量并发防护**：状态机与测试面大一个量级，防护的竞态被单锁消除。
- **连 CAS 一起取消**：单操作员也可能开两个标签页/中途 binding 变更；两个整数比对的成本换取误替换保护，保留合算。
- **维持 digest 禁令**：无模型内攻击者；成本全部落在规范与测试维护。

## Consequences

### Positive

- Import 状态机、preview evidence 结构与负向测试面大幅缩小；
- `INV-SECRET-01` 缩短为可背诵的红线；
- 提交冲突的用户路径统一为"重新 preview"，无特例分支。

### Negative / trade-offs

- 多操作员并发导入若将来出现（F8 失效），需恢复更细的冲突模型；
- 审计导出交给外部方前必须先重审第 6 条。

## Evidence and revisit trigger

- 接受前需要：pending 互斥、双 CAS 拒绝路径、重复提交安全失败、明文红线 secret scan 的测试证据。
- 重开条件：F8（单操作员）或 F9（审计仅内部）失效。

## References

- [ADR-0020](0020-repeatable-contest-configuration-import.md)
- [ADR-0022](0022-deployment-facts-and-trust-assumptions.md)
- [domain-model.md](../domain-model.md)
- [security-recovery.md](../security-recovery.md)
