# ADR-0012: Server-auth Enrollment and mTLS control

> Status: `SUPERSEDED`  
> Scope: Natsume V2  
> Superseded by: [ADR-0023](0023-wss-control-channel-with-device-token.md)

## Context

首次注册时 Device 尚无认证凭据，但后续控制必须强认证。把 Enrollment、control 和所有签发混成一个接口会产生匿名授权面。

在 [ADR-0022](0022-deployment-facts-and-trust-assumptions.md) 记录的部署事实下，mTLS client certificate 与 QUIC 两项具体选择被重新评估并替代；见 [ADR-0023](0023-wss-control-channel-with-device-token.md)。

## Decision

**本决策已被 ADR-0023 替代。**

原决策：Enrollment 使用 server-auth HTTPS，只提交 Device Identity CSR 并返回 Device leaf/chain。随后 Device control 使用 mandatory-mTLS QUIC，匿名 peer 在 TLS 阶段拒绝，0-RTT 禁用。

仍然有效并由 ADR-0023 继承的部分：Enrollment 为 server-auth HTTPS，Client 验证预置 Server trust 与 IP-SAN，无 TOFU、无 dangerous verifier；Enrollment 与 control 风险分离（独立路由、授权、限流）；未认证输入不进入协议 decoder；0-RTT/early data 关闭。

不得从本 ADR 恢复：Device Identity certificate 与其 CA；mandatory-mTLS client 认证；QUIC 传输。恢复 mTLS 的条件见 ADR-0023 的 revisit trigger。

## Alternatives

- 预共享 enrollment token：增加 token 分发/泄漏/恢复。
- TOFU：不满足 IP-SAN/trust 决策。
- server-auth-only 且无应用层认证：Device 身份不足。
- 一个通用 certificate endpoint：扩大签发权限。

## Consequences

### Positive

- 认证阶梯清晰；
- 匿名输入不进入协议 decoder；
- Enrollment 和 control 风险分离。

### Negative / trade-offs

- 需要预部署 Server trust；
- PKI ceremony 和 certificate lifecycle 增加运维（该项随 ADR-0023 删除 Device CA 而缩小）。

## Evidence and revisit trigger

已被替代，不再作为当前决策。任何重新考虑必须先 supersede [ADR-0023](0023-wss-control-channel-with-device-token.md)。

## References

- [ADR-0023](0023-wss-control-channel-with-device-token.md)（替代本 ADR）
- [contracts.md](../contracts.md)
- [security-recovery.md](../security-recovery.md)
