# ADR-0016: Gateway certificate issued during SYNC_STATE

> Status: `ACCEPTED`  
> Scope: Natsume V2

## Context

Gateway certificate 的授权取决于当前 Device、binding、Target hostname 和 configuration generation。Enrollment 时这些条件可能不存在或会变化。

## Decision

Gateway key 在 Client 本地生成；CSR 只能通过 authenticated mTLS QUIC，在 active `SYNC_STATE` Command 中提交。Server 从 Target 派生 SAN/profile/validity，忽略 CSR 自报授权字段；request/SPKI 幂等并检测冲突。

## Alternatives

- Enrollment 同时签 Gateway：过早绑定且扩大匿名注册面。
- 通用 INSTALL_CERTIFICATE：形成任意证书分发能力。
- Client 决定 SAN：授权下放错误。

## Consequences

### Positive

- 签发绑定当前业务意图；
- 私钥不离开 Client；
- 幂等和审计清晰。

### Negative / trade-offs

- 首次 state sync 多一个子协议；
- 需要处理证书和 Caddy 原子激活。

## Evidence and revisit trigger

Probe B/C 和 G0-005–009 的负向测试必须通过。

## References

- [contracts.md](../contracts.md)
- [security-recovery.md](../security-recovery.md)
