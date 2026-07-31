# ADR-0005: CSV-only import

> Status: `SUPERSEDED`
> Scope: Natsume V2
> Supersedes: —
> Superseded by: [ADR-0020](0020-repeatable-contest-configuration-import.md)

## Context

现场输入只需要 Seat、account 和 password。支持 XLSX/ODS、sheet、公式和列映射会显著增加 parser/supply-chain/preview 特例，并扩大秘密处理面。

## Decision

> 历史决策正文（已不再是当前规范）。当前 import 语义以 [ADR-0020](0020-repeatable-contest-configuration-import.md) 与 [domain-model.md](../domain-model.md) 为准。ADR-0020 保留本 ADR 的 CSV-only 固定列决策，并移除首次 commit 后永久冻结 Seat universe 的规则。

只接受一个 UTF-8/BOM CSV，列必须恰好为 `seat,account,password`。上传进入加密 staging；preview 后显式 commit；首次 commit 冻结 Seat universe。无 XLSX/ODS、公式、列映射或密码导出。

## Alternatives

- XLSX/ODS：复杂解析和公式/格式风险。
- 可配置列映射：增加 UI、错误和审计分支。
- 直接导入无 preview：不满足现场可审查和原子提交。

## Consequences

### Positive

- 契约简单且可 fuzz；
- 秘密路径小；
- preview/commit 语义确定。

### Negative / trade-offs

- 上游表格必须先转换 CSV；
- 不能容忍自定义列名；
- 永久 freeze 无法满足同一 lifetime 内修正完整配置（由 ADR-0020 解决）。

## Evidence and revisit trigger

只有明确产品需求证明多个输入格式的收益高于安全和测试成本时才重新评估。Seat 集合可重复替换见 ADR-0020。

## References

- [ADR-0020](0020-repeatable-contest-configuration-import.md)
- [domain-model.md](../domain-model.md)
