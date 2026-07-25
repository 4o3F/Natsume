# ADR-0012: Server-auth Enrollment and mTLS control

> Status: `ACCEPTED`  
> Scope: Natsume V2

## Context

首次注册时 Device 尚无 client certificate，但后续控制必须强认证。把 Enrollment、control 和所有证书签发混成一个接口会产生匿名授权面。

## Decision

Enrollment 使用 server-auth HTTPS，只提交 Device Identity CSR并返回 Device leaf/chain。随后 Device control 使用 mandatory-mTLS QUIC，匿名 peer 在 TLS 阶段拒绝，0-RTT 禁用。Server control certificate 由离线 control root 流程提供；local origin PKI 分离。

## Alternatives

- 预共享 enrollment token：增加 token 分发/泄漏/恢复。
- TOFU：不满足 IP-SAN/trust 决策。
- server-auth-only QUIC：Device 身份不足。
- 一个通用 certificate endpoint：扩大签发权限。

## Consequences

### Positive

- 认证阶梯清晰；
- 匿名输入不进入协议 decoder；
- Enrollment 和 control 风险分离。

### Negative / trade-offs

- 需要预部署 Server trust；
- PKI ceremony 和 certificate lifecycle 增加运维。

## Evidence and revisit trigger

Probe A/B、schema/DB tests 和 package provisioning 必须证明完整阶梯。

## References

- [contracts.md](../contracts.md)
- [security-recovery.md](../security-recovery.md)
