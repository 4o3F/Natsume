# ADR-0004: SNAFU unified error model

> Status: `ACCEPTED`  
> Scope: Natsume V2

## Context

多个 Rust 进程和共享 crate 需要保留 typed domain error、source chain 和可审计 context，同时在 HTTP/Protobuf/D-Bus 上提供稳定公开码。

## Decision

第一方 Rust domain/application 使用 SNAFU。每个模块保留自己的 error type；adapter 穷举映射到 stable ErrorCode。Display 文本不参与业务协议。

## Alternatives

- `anyhow` 作为统一模型：适合应用边缘，但会抹平稳定 domain category。
- `thiserror`：可表达 typed error，但当前基线选择 SNAFU 的 context/selectors。
- 一个全局 Error enum：会把所有模块耦合到共同变更中心。

## Consequences

### Positive

- 模块错误保持内聚；
- source/context 清晰；
- 公开映射可测试。

### Negative / trade-offs

- 映射代码较多；
- 开发者需要遵循 context/redaction 规则。

## Evidence and revisit trigger

只在有实测维护问题且替代方案能保留模块边界、redaction 和 exhaustive mapping 时重新评估。

## References

- [contracts.md](../contracts.md)
- [dependency-policy.md](../dependency-policy.md)
