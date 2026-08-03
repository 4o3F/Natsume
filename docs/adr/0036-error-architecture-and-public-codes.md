# ADR-0036: Error architecture and public codes

> Status: `ACCEPTED`
> Scope: first-party Rust errors and public HTTP, Protobuf, D-Bus, and CommandStatus error contracts
> Consolidates: ADR-0004, ADR-0019
> Supersedes: consolidated historical records; see [`history-map.md`](history-map.md)
> Superseded by: —

## Context

Natsume 同时需要丰富的模块内失败上下文与跨 HTTP、Protobuf、D-Bus、CommandStatus 的稳定公开语义。单一 global domain error 会让无关模块共享变化中心；transport-specific code 或解析 Display 文本又会造成语义漂移、脆弱 Client 和秘密泄露。

模块内 SNAFU 与稳定 ErrorCode registry 必须作为一个完整架构被接受：本地 typed failure 保留 ownership，public boundary 提供穷举映射、稳定字符串和默认脱敏。

Panel-owned Command ID 还需要把输入无效与同 ID 不同请求这两类失败从通用 validation/conflict 中稳定地区分出来；否则 Client 不能安全决定是修正 ID、读取既有 Command，还是生成新 ID。

## Decision

- first-party Rust domain/application module 使用 module-local SNAFU typed error；variant、context selector 与 source relation 由 owning module 定义，不依赖 global business-error enum。
- 独立 `natsume-error-code` registry 拥有 closed `ErrorCode` 集合和稳定字符串；不得依赖 Axum、Prost、zbus、SQLx 或具体 domain module。
- 依赖方向固定为：

  ```text
  module-local typed error
    → exhaustive public-boundary adapter mapping
    → stable ErrorCode
    → HTTP / Protobuf / D-Bus / CommandStatus representation
  ```

- `ErrorCode` 不进入 domain policy。每个可跨 public boundary 的 local variant 都在 adapter 中显式、编译期穷举映射；多个 local variant 可以收敛为一个 reviewed public semantic。
- 同一公开语义在所有 transport 使用同一 `ErrorCode`。HTTP 显式映射 status 与 Problem Details/correlation ID；Protobuf/CommandStatus 使用稳定字符串；D-Bus 使用显式稳定 error name。
- stable string 不从 `Display`、`Debug`、localization、source error、Rust variant name 或 transport text 派生；published code 不得无 compatibility plan 地删除或改义。
- public Client 只能按 `ErrorCode` 和明确安全的 typed fields 分支，不得解析 title、detail、Display、source-chain 或翻译文本。
- public `detail` 默认缺失；只有经过 review/redaction 才能输出。password、private key、Device Token、certificate/CSR body、完整 Machine Hardware ID、path、arbitrary payload 与 source chain 不得进入 public、audit、metric 或 ordinary log boundary。
- internal diagnostic 可以保留受限 source/context，但 error constructor 本身不得捕获 secret/path；redaction 不是把敏感值放入 error context 的许可。
- `PUT /api/v2/commands/{command_id}` 保留以下稳定公开语义：
  - `COMMAND_ID_INVALID`：path ID 不是 canonical lowercase hyphenated UUIDv7；HTTP 为 `400`。
  - `COMMAND_REQUEST_CONFLICT`：已有 `command_id` 与当前 request 的 versioned canonical fingerprint 不同；HTTP 为 `409`。
  - 首次创建的 `201` 和 same-ID/same-fingerprint replay 的 `200` 不是错误。错误 response 不回显原始 request、fingerprint、payload 或未脱敏诊断。
- `anyhow` 与 `thiserror` 不是 first-party unified domain/application error model；例外需要独立 ADR，并必须保留 module ownership、stable mapping 与 redaction guarantees。

## Alternatives

- global Error enum：集中无关变化并破坏模块内聚。
- transport-specific code：跨 boundary 的同一语义会漂移。
- 解析 Display/title/detail：presentation 不稳定、可本地化且不安全。
- 把 command-ID-invalid 与 same-ID conflict 合并为模糊 validation error：Client 无法稳定处理 create/replay/conflict 边界。
- `anyhow` 作为公共模型：type erasure 使穷举 classification 困难。
- 暴露 raw source chain 或每个 internal variant 都新增 public code：分别泄露实现细节或制造长期 compatibility burden。

## Consequences

### Positive

- 模块保留 cohesive typed failure 与内部诊断上下文。
- Web、Device、Agent、Helper 和所有 transport 共享稳定语义。
- `COMMAND_ID_INVALID` 与 `COMMAND_REQUEST_CONFLICT` 使 Panel 的 UUIDv7 create/replay contract 可测试而不依赖显示文本。
- exhaustive mapping 让新增 public failure path 可审查、可测试。
- standalone registry 保护 dependency direction，并把 redaction 和兼容性变成明确边界。

### Negative / trade-offs

- adapter mapping、registry coverage、status mapping 和 compatibility review 需要持续维护。
- 部分内部差异会有意收敛为较小 public code set。
- redaction 降低即时公开细节，排障必须依赖 correlation ID 与受限诊断。
- 新 public code 是长期承诺，不能因实现方便随意添加。

## Acceptance basis and revisit trigger

正式接受该架构不表示所有 phase 已完成。证据必须覆盖 module-local SNAFU、所有 public adapter 的 exhaustive mapping、registry uniqueness、HTTP/Protobuf/D-Bus 显式表示、`COMMAND_ID_INVALID` 的 canonical UUIDv7 正/反例、`COMMAND_REQUEST_CONFLICT` 的 same-ID/different-fingerprint `409`、same request `200` replay、generated contract clean diff，以及 password/token/key/CSR/path/source-chain canary 的负向 redaction；Web/Device 不得解析 Display。

只有在测量证据证明 SNAFU 维护问题无法在保留 module boundary、exhaustive mapping 与 redaction 的前提下解决时重开。新增 transport 必须复用现有 ErrorCode contract，不建立平行 registry。

## Normative sources

- [Architecture](../architecture.md)
- [Contracts](../contracts.md)
- [Security and recovery](../security-recovery.md)
- [Dependency policy](../dependency-policy.md)
- [Repository layout](../repository-layout.md)
- [Phase 0 status](../gates/phase-0-status.md)
