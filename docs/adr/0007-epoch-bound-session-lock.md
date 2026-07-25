# ADR-0007: Epoch-bound Session Lock

> Status: `ACCEPTED`  
> Scope: Natsume V2

## Context

桌面 Agent、logind session 和操作员命令可能在重启、重登或网络延迟后陈旧。仅用 Seat/UID 判断会让旧 Agent 操作新会话。早期设计还可能把 lock 与 Caddy 状态耦合。

## Decision

所有 lock/unlock/terminate 动作绑定当前 SessionEpoch，并验证 logind session、UID、boot/session identity 和 Agent lease。陈旧 epoch 拒绝。Session 操作不调用或修改 Caddy。

## Alternatives

- 只按 UID/Seat：无法区分重建会话。
- 按 wall-clock timeout：不能可靠解决重放。
- lock 时切 Caddy BLOCKED：把桌面可见性与网络数据面错误耦合。

## Consequences

### Positive

- 竞态可穷举；
- 陈旧 Agent 权限被限制；
- Session/Caddy 独立。

### Negative / trade-offs

- 需要维护 epoch/lease；
- 桌面 adapter 必须可靠获取 logind identity。

## Evidence and revisit trigger

任何新的 desktop backend 都必须证明 epoch 和 Caddy 不变证据；不能用平台特例放宽。

## References

- [state-and-execution.md](../state-and-execution.md)
- [security-recovery.md](../security-recovery.md)
