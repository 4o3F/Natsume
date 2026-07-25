# ADR-0014: Observed snapshot is the status source

> Status: `ACCEPTED`  
> Scope: Natsume V2

## Context

Command 成功、Server Target 或最近一次投递都不能证明 Device 当前实际状态。使用 Desired/CommandStatus 作为设备状态会在断线、重启和本地故障后产生误报。

## Decision

Device 的实际业务状态只来自最新有效 Observed snapshot。Target 表达期望，CommandStatus 表达某个动作，Drift 比较 Target 和 Observed；三者不得互相替代。

## Alternatives

- Target 当状态：忽略应用失败。
- Command success 当状态：后续本地变化不可见。
- 事件增量作为唯一状态：重连和丢失恢复复杂。

## Consequences

### Positive

- UI 语义准确；
- 重连可以完整收敛；
- Drift 可重算。

### Negative / trade-offs

- 需要完整 snapshot、freshness 和存储；
- Observed 可能陈旧，UI 必须表达 unknown。

## Evidence and revisit trigger

integration tests 应覆盖断线、丢失、重发、重启和本地状态变化。

## References

- [domain-model.md](../domain-model.md)
- [state-and-execution.md](../state-and-execution.md)
