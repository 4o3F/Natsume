# ADR-0006: Daemon-integrated Machine Identity startup

> Status: `ACCEPTED`  
> Scope: Natsume V2

## Context

身份检查必须先于 vault。单独 Identity Guard service 或安装实例 fallback 会增加启动竞态、状态同步和恢复特例。

## Decision

Machine identity startup 是 Device Daemon 的第一段 application flow，不创建独立 Guard service。Daemon 根据当前硬件、持久化 ID、identity-bound artifact 和 vault 结果执行封闭决策表；失败即 fail closed。

## Alternatives

- 独立 Identity Guard unit：增加服务间状态/ordering/恢复耦合。
- 安装实例 ID fallback：disk copy 后可能错误继续。
- vault 失败时自动 re-enroll：会丢失身份和证据。

## Consequences

### Positive

- 启动顺序单一；
- identity/vault 决策可原子审计；
- package 拓扑更小。

### Negative / trade-offs

- Daemon composition 必须严格保证其他 adapter 后初始化；
- 启动错误需要清晰诊断。

## Evidence and revisit trigger

若未来多个进程都需要在启动前共享同一硬件身份 gate，可考虑最小只读机制，但不能绕过 `INV-IDENTITY-02`。

## References

- [security-recovery.md](../security-recovery.md)
