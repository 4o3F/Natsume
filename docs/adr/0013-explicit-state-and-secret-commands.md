# ADR-0013: Explicit state and secret commands

> Status: `ACCEPTED`  
> Scope: Natsume V2

## Context

CSV、binding 或配置变化若自动推送，会把领域 transaction、远端可用性和秘密分发耦合，并难以审计现场意图。

## Decision

Target 非秘密且惰性。只有操作员显式发起 `SYNC_STATE` 才应用非秘密状态；只有显式 `SYNC_SECRET` 才分发密码。两者为独立 durable Command，secret sync 绑定当前 assignment/credential revision。

## Alternatives

- 自动 reconcile：减少点击但增加隐式副作用和现场不可控性。
- 把密码放入 SYNC_STATE：扩大秘密面。
- 保存配置即同步：Web request 与远端执行耦合。

## Consequences

### Positive

- 人工意图和审计明确；
- 秘密路径最小；
- 离线和重试语义清晰。

### Negative / trade-offs

- 操作员需要管理 Drift 和同步步骤；
- 可能存在一段有意的 Target/Observed 差异。

## Evidence and revisit trigger

UI、authorization、domain tests 和 secret negative tests 必须证明不存在自动路径。

## References

- [state-and-execution.md](../state-and-execution.md)
- [security-recovery.md](../security-recovery.md)
