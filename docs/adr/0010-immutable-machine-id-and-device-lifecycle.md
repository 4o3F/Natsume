# ADR-0010: Immutable Machine ID and Device lifecycle

> Status: `ACCEPTED`  
> Scope: Natsume V2

## Context

硬件更换、disk copy 和 identity conflict 需要清晰的业务生命周期。允许编辑 Machine ID 或 merge/split Device 会破坏证书、审计和 vault 绑定。

## Decision

Machine Hardware ID 对一个 Device lifecycle 不可变；Server 内部使用独立 UUIDv7 DevicePk。设备替换通过 retire/delete + 新 Enrollment + 新 binding 完成；不提供 merge/split 或 identity edit。

## Alternatives

- 可编辑 Machine ID：历史和证书绑定不可追溯。
- 自动 merge：冲突时可能接管错误设备。
- 复用 DevicePk 给新硬件：掩盖 lifecycle boundary。

## Consequences

### Positive

- 证书和审计链清晰；
- replacement 可演练；
- identity conflict fail closed。

### Negative / trade-offs

- 硬件更换需要显式操作；
- 某些维修场景步骤更多。

## Evidence and revisit trigger

若将来有可验证的主板维修/组件更换语义，应新增专门 lifecycle ADR，而不是开放任意编辑。

## References

- [domain-model.md](../domain-model.md)
