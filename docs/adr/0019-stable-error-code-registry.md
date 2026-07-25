# ADR-0019: Stable ErrorCode registry

> Status: `PROPOSED`  
> Scope: Natsume V2

## Context

HTTP、Protobuf、D-Bus 和 CommandStatus 需要让调用方可靠识别同一错误语义。仅用人类 Display 文本会导致解析耦合；一个全局 domain error 又会破坏模块内聚。

## Decision

建立独立 `natsume-error-code` registry，字符串码显式且稳定。Domain 保留各自 SNAFU error；公开 adapter 穷举映射。Error detail 默认无或脱敏，调用方只按 code/typed fields 分支。

## Alternatives

- 每个 transport 独立码：跨边界语义漂移。
- 解析 Display：不稳定且可能泄密。
- 全局 Error enum 进入 domain：高耦合。

## Consequences

### Positive

- 跨 transport 一致；
- Web/Device 逻辑稳定；
- redaction 和兼容可测试。

### Negative / trade-offs

- 需要维护 registry 和 exhaustive mappings；
- 新公开语义需兼容评审。

## Evidence and revisit trigger

在正式接受前，确认当前 crate/API/Proto/D-Bus mappings、字符串稳定性、redaction 和 clean-diff tests；实现存在不等于治理状态自动变为 ACCEPTED。

## References

- [contracts.md](../contracts.md)
- [dependency-policy.md](../dependency-policy.md)
