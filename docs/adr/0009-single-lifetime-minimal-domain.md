# ADR-0009: Single-lifetime minimal domain

> Status: `ACCEPTED`  
> Scope: Natsume V2

## Context

当前部署一次只服务一场竞赛。引入 Event、历史 phase 和跨赛事兼容会扩大所有表、API、权限、Target 和恢复流程。

## Decision

一个初始化后的 Server 实例只建模当前一场竞赛；没有 Event entity 或 runtime phase。重用通过明确的 single-lifetime reset 完成。

## Alternatives

- 多 Event：当前无实际并发/历史查询需求。
- 软归档旧赛事：会保留秘密和复杂生命周期。
- 每场独立数据库但同实例切换：仍引入 runtime phase。

## Consequences

### Positive

- 领域和权限更小；
- reset 边界明确；
- 减少跨赛事秘密风险。

### Negative / trade-offs

- 不能在同实例查询历史赛事；
- 重用需要破坏性重置和备份策略。

## Evidence and revisit trigger

只有明确、获批准的多赛事产品需求出现时重新建模，不能在现有表中加 nullable event_id 特例。

## References

- [architecture.md](../architecture.md)
- [domain-model.md](../domain-model.md)
