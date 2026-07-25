# ADR-0001: Native polyglot monorepo

> Status: `ACCEPTED`  
> Scope: Natsume V2

## Context

Natsume 同时包含 Rust 后端/客户端、TypeScript Web、Deb packaging 和文档工具。引入 Nx、Turborepo、Bazel 等统一 orchestrator 会建立第二套依赖图，并在 Phase 0 增加缓存、插件和供应链复杂度。

## Decision

使用一个原生 polyglot monorepo：Cargo 拥有 Rust graph，pnpm 拥有 Web graph，`just` 只分发命令，nFPM 只映射已构建 artifact。根目录不增加通用 `apps/`、`packages/` 或第二套 workspace 抽象。

## Alternatives

- Nx/Turborepo：当前规模的收益不足以抵消第二套 graph 和 cache 语义。
- Bazel：可重现能力强，但迁移和规则维护成本高于当前需求。
- 多个仓库：会增加协议、版本、发布和文档的一致性成本。

## Consequences

### Positive

- 原生工具行为清晰；
- 锁文件和 ownership 单一；
- CI 与本地命令易于审计。

### Negative / trade-offs

- 跨语言增量缓存较弱；
- `justfile` 需要显式维护；
- 未来规模扩大时可能重新评估 orchestrator。

## Evidence and revisit trigger

当构建时间、仓库规模或多语言发布关系显著增长，且有实测数据证明统一 orchestrator 能降低总复杂度时重新评估。

## References

- [repository-layout.md](../repository-layout.md)
- [dependency-policy.md](../dependency-policy.md)
