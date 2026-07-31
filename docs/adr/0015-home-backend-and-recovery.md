# ADR-0015: Home backend and recovery

> Status: `ACCEPTED`  
> Scope: Natsume V2

## Context

竞赛工作站需要可重置 Home。OverlayFS 性能好但依赖目标 kernel/filesystem；staged-copy 可移植但成本更高。运行时自动 fallback 会让失败语义和数据残留不可预测。

## Decision

使用固定 contest user 和 versioned template。部署时在 OverlayFS 与 staged-copy 中选择一个 backend，并记录 ADR/evidence；运行时不 silent fallback。所有 Home 操作使用 durable HomeEpoch transaction，无法证明安全时不启动 session。

## Alternatives

- 每次创建 Linux user：扩大系统生命周期。
- 运行时自动 fallback：难以证明清理和 ownership。
- 直接复用持久 Home：不能保证竞赛隔离和重置。

## Consequences

### Positive

- backend 差异封装；
- 故障恢复可建模；
- session start 有安全前置。

### Negative / trade-offs

- 需要两套 probe/实现候选；
- 部署选择后灵活性降低。

## Evidence and revisit trigger

Probe E 必须覆盖 prepare/cleanup、crash、reboot、disk full、ownership 和性能。

## References

- [state-and-execution.md](../state-and-execution.md)
