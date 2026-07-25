# ADR-0017: Cross-desktop Session Agent GUI

> Status: `SUPERSEDED`  
> Scope: Natsume V2

## Context

早期方案探索跨桌面 bootstrap、手工 GUI stack 和环境转交，以同时支持 Wayland/X11。

## Decision

该方案不再有效，已被 ADR-0018 替代。不得从本 ADR 恢复 bootstrap/run handoff、环境 descriptor、systemd user unit 或手拼 winit/renderer/text stack。

## Alternatives

- 见历史方案；当前不再作为可选项。

## Consequences

### Positive

- 保留决策历史和为何不再使用。

### Negative / trade-offs

- 无当前实现意义。

## Evidence and revisit trigger

任何重新考虑必须先 supersede ADR-0018 并证明 package、session identity、GUI closure 和双桌面 evidence。

## References

- [0018-xdg-direct-slint-session-agent.md](0018-xdg-direct-slint-session-agent.md)
- [dependency-policy.md](../dependency-policy.md)
