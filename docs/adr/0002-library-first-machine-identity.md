# ADR-0002: Library-first Machine Identity

> Status: `ACCEPTED`  
> Scope: Natsume V2  
> Note: 来源策略（增量准入 smbios/raw-cpuid/procfs/udev 等）已由 [ADR-0025](0025-deterministic-hardware-identity-recipe.md) 收窄为固定来源集合；library-first 结构与本 ADR 其余条款维持有效。

## Context

Machine Hardware ID 需要组合多个硬件来源、处理 placeholder/缺失/冲突，并在 Device Daemon 与特权采集边界之间保持可测试性。将规则直接写在 root Helper 或 daemon startup 中会把硬件访问、业务决策和持久化耦合。

## Decision

建立纯 `machine-identity` crate，负责候选类型、规范化、质量、冲突和 UUIDv5 派生。原始硬件 source collector 留在 privileged adapter；优先采用稳定 library，并按目标硬件证据增量加入 smbios/raw-cpuid/procfs/udev/libsystemd。Fleet namespace UUID 公开且不可变。

## Alternatives

- 只使用 `/etc/machine-id`：无法证明满足物理硬件和 disk-copy 语义。
- Helper 直接返回最终 UUID：会把安全策略和可测试规则锁入 root 进程。
- 安装实例 ID：不能表达物理机器身份。

## Consequences

### Positive

- 纯规则可用匿名 fixture 测试；
- root 边界更小；
- 候选质量和冲突语义稳定。

### Negative / trade-offs

- 需要明确维护 collector 与 pure crate 的转换；
- 目标硬件不足时不能宣称支持。

## Evidence and revisit trigger

若目标平台提供经过验证且跨 disk-copy 稳定的单一标准硬件身份 API，可重新评估 collector 复杂度；不得放宽 `INV-IDENTITY-01/02`。

## References

- [security-recovery.md](../security-recovery.md)
- [domain-model.md](../domain-model.md)
