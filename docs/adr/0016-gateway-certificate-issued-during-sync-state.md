# ADR-0016: Gateway certificate issued during SYNC_STATE

> Status: `SUPERSEDED`  
> Scope: Natsume V2  
> Superseded by: [ADR-0021](0021-provisioning-window-certificate-issuance.md)

## Context

Gateway certificate 的授权取决于当前 Device、binding、Target hostname 和 configuration generation。Enrollment 时这些条件可能不存在或会变化。

该前提在已记录的部署模型下不成立：hostname 是每场地一个常量，binding 与 generation 都在 provisioning 窗口内确定。见 [ADR-0021](0021-provisioning-window-certificate-issuance.md)。

## Decision

**本决策已被 ADR-0021 替代。**

原决策：Gateway key 在 Client 本地生成；CSR 只能通过 authenticated mTLS QUIC，在 active `SYNC_STATE` Command 中提交。Server 从 Target 派生 SAN/profile/validity，忽略 CSR 自报授权字段；request/SPKI 幂等并检测冲突。

仍然有效并由 ADR-0021 继承的部分：Gateway 私钥在 Client 本地生成且不离机；Server 决定 SAN/profile/validity；CSR 自报字段不授予权限。

不得从本 ADR 恢复：CSR 嵌入 active `SYNC_STATE` Command 的子协议；CSR 与 `command_id`/configuration generation/assignment revision 的绑定；SPKI 冲突检测状态机；从 Target 派生 SAN。

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

已被替代，不再作为当前决策。任何重新考虑必须先 supersede [ADR-0021](0021-provisioning-window-certificate-issuance.md)，并证明每设备 hostname 需求或物理安全窗口假设不再成立。

## References

- [ADR-0021](0021-provisioning-window-certificate-issuance.md)（替代本 ADR）
- [contracts.md](../contracts.md)
- [security-recovery.md](../security-recovery.md)
