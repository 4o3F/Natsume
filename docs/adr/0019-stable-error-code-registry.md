# ADR-0019：稳定 ErrorCode registry、SNAFU 映射与脱敏边界

## 元数据

| 字段 | 值 |
|---|---|
| ADR ID | `ADR-0019` |
| 状态 | `PROPOSED` |
| 日期 | `2026-07-23` |
| Decision owner | `ROLE_SECURITY` |
| Reviewers | `ROLE_SERVER` / `ROLE_CLIENT` / `ROLE_BUILD` |
| 关联需求 | `REQ-P0-020` / `REQ-P0-021` / `REQ-P0-022` / `REQ-P0-023` |
| 关联 Probe/Gate | Probe B/E；`G0-001` / `G0-004` / `G0-007` / `G0-008` / `G0-010` / `G0-013` |
| 替代的 ADR | 无 |

## 1. 上下文（Context）

Phase 0 已在 Protobuf 和数据库骨架中预留 stable error code 字段，但尚无单一权威 registry。若各进程直接使用 SNAFU `Display` 文案作为协议值，HTTP、QUIC、D-Bus 与 CommandStatus 会随文案修改而漂移，并可能把 path、source chain、CSR 或 secret 暴露给远端或 operator surface。

当前 Rust/CI 仍为 `ENV-PROPOSED`；本 ADR 不构成目标环境或 G0 签收。Step 3 只能建立稳定码、表面映射、typed-domain-error 接线与脱敏；不得进入 Step 4 的 OpenAPI source、descriptor、D-Bus XML 或 runtime SQL。

## 2. 决策（Decision）

> 我们决定：
>
> 1. 新增 `crates/error-code`（package `natsume-error-code`），作为 Phase 0 第四个、且本阶段唯一新增的共享生产契约。
> 2. `ErrorCode::as_str` 独占定义 SCREAMING_SNAKE 稳定字符串；不得从 `Display`、`Debug`、翻译文案或 source error 推导。
> 3. HTTP status/title/Problem Details、protocol stable string 和 `org.natsume.Error.*` D-Bus name 使用穷尽映射；registry 不依赖 Axum、Prost 或 zbus。
> 4. 领域错误仍由 owning module 定义 typed SNAFU enum，并通过 `AsErrorCode` 或显式 match 映射；不得用 `ErrorCode` 取代领域错误。
> 5. Problem Details 默认 `detail = None`。跨 operator/report 边界前使用 `Redacted<T>`、`CodedReport` 或 `redact_report`，禁止输出 secret、path 或 source chain。
> 6. Server、Daemon、Privileged Helper、Session Agent 均成为真实 package consumer；仍为 infallible blueprint 的 binary 使用公开 compile-time ownership module，不伪造 runtime failure。
> 7. Phase 0 最小集合固定为计划列出的 23 个码；新增或改变已发布语义需要更新 ADR、需求和契约测试。
>
> 我们明确不采用：
>
> - 第一方 `anyhow` / `thiserror`；
> - 通用 `common` / `utils` / `helpers` crate；
> - 解析错误文案进行业务判断；
> - 在 Step 3 实现 QUIC framing、真实 OpenAPI generation、D-Bus introspection 或 SQL execution；
> - TOFU、Identity Guard、systemd credentials 或 runtime download。

### 2.1 证书边界

| 边界 | 决策 |
|---|---|
| Device Identity certificate | Enrollment 仅使用 `ENROLLMENT_*`；错误码不得暗示 Gateway certificate 已准备 |
| Gateway certificate | 仅使用 `GATEWAY_CERT_*`；语义绑定 authenticated mTLS QUIC + active `SYNC_STATE` |
| 禁止项 | Enrollment error/schema 中出现 Gateway CSR、SPKI、leaf/chain 或通用证书签发接口 |

### 2.2 所有权

| 表面 | Owner | Phase 0 最小码 |
|---|---|---|
| install endpoint validation | Device Daemon | `INSTALL_ENDPOINT_*` |
| HTTPS Enrollment | Server | `ENROLLMENT_*` |
| mandatory-mTLS control | Server + Device Daemon | `PROTOCOL_*` |
| Gateway certificate subprotocol | Server + Device Daemon | `GATEWAY_CERT_*` |
| Session lock D-Bus | Session Agent + Device Daemon | `SESSION_CHANGED`、lock epoch/command codes |
| Home | Privileged Helper | `HOME_TRANSITION` |
| vault | Server + Device Daemon | `VAULT_CORRUPT` |
| package/invocation contract | package consumers | `PACKAGE_LAYOUT_INVALID` |

Helper 的 `HardwareCollectionError::NotImplemented` 是 blueprint stub，不映射为稳定 wire code。

## 3. 备选方案（Alternatives）

| 方案 | 描述 | 优点 | 缺点 | 结论 |
|---|---|---|---|---|
| 独立 registry + 显式映射 | 本 ADR | 多 consumer、可测试、无 runtime framework 耦合 | 需要维护穷尽映射 | 采用 |
| 每个 crate 自建字符串 | 各自维护 code 常量 | 无新增 crate | 重复、漂移、无统一脱敏 | 拒绝 |
| 单一通用错误类型 | 所有领域返回 `ErrorCode` | 表面简单 | 丢失 typed context，演变为 common dump | 拒绝 |
| 从 Display 推导 | 将文案写入 wire/API | 快速 | 不稳定且可能泄密 | 拒绝 |

## 4. 失败与恢复（Failure / Recovery）

- 失败表现：映射不穷尽、跨表面 code 不一致、路径或凭据出现在 report。
- 检测方式：registry 单测、consumer ownership/mapping 测试、Clippy `-D warnings`、policy scan。
- Fail-closed：未知领域错误不得伪装成成功；无合适稳定码的 blueprint stub 不得随意映射。
- 恢复：修正显式映射；改变已发布稳定字符串时必须新增兼容决策，不直接重命名。
- Step 3 不改变已安装包、配置、证书或数据库 schema。

## 5. 安全影响（Security Impact）

- registry 不改变 PKI 信任边界。
- redaction 覆盖 private-key/CSR PEM、password/token/Authorization/Cookie、URL userinfo、绝对路径、长 base64/hex 和 source-chain 行。
- `Redacted<T>` 的 `Debug`/`Display` 永远只输出占位符。
- `CodedReport` 输出 stable code + sanitized detail，且不暴露原始 source chain。
- Device-only Enrollment 与 Gateway `SYNC_STATE` 命名空间保持隔离。

## 6. 测试影响（Test Impact）

| 层级 | 所需验证 |
|---|---|
| PR | registry 唯一性/映射/Problem Details/redaction；consumer ownership；Daemon endpoint mapping；export path redaction；fmt/Clippy/test/deny |
| Nightly/VM | 与 PR 相同；不构成目标环境证据 |
| Physical/Desktop Lab | Step 3 无新增要求 |
| Probe/G0 | 保持 OPEN；后续 Probe B/E 消费稳定码 |

## 7. 迁移影响（Migration Impact）

- 配置、debconf、systemd、package topology：无变更。
- OpenAPI/Protobuf/D-Bus/SQL schema：无变更；仅建立后续显式映射的 Rust source。
- workspace 新增一个共享 crate，并由四个生产 package 依赖。
- 同步更新根 README、`crates/README.md`、repository layout 与 V2 design。

## 8. 后果与残余限制

### 正向后果

- 跨表面稳定码拥有单一来源；
- 领域错误与公共 contract 分离；
- secret/path/source-chain 脱敏可由单元测试证明。

### 负向后果与成本

- 每个新增码需要同步维护显式 HTTP 和 D-Bus 映射；
- 当前 infallible binary 只有 compile-time ownership，没有 runtime error coverage。

### 已知限制 / Non-claims

- 不宣称 `REQ-P0-020`–`REQ-P0-023` 或任何 G0 项已 PASS；
- 不实现 Step 4 的 runtime/snapshot contract；
- hosted CI 和目标环境签收仍为空。

## 9. 追踪与证据

| 类型 | 定位符 |
|---|---|
| Requirements | `REQ-P0-020`–`REQ-P0-023`，状态保持 `OPEN` |
| Supported platform | Rust/CI `ENV-PROPOSED` |
| Lab assets | `NONE` |
| Probe report | `NONE` |
| Gate | `G0-001/004/007/008/010/013`，状态保持 `OPEN` |
| CI/Test evidence | 本地验证不替代 hosted/target evidence |

## 10. 审批

| 角色 | 姓名 | 日期 | 结论 |
|---|---|---|---|
| Decision owner | | | |
| Architecture reviewer | | | |
| Security/QA reviewer | | | |

## 11. 修订历史

| 日期 | 修改 | 作者角色 |
|---|---|---|
| 2026-07-23 | Step 3 初稿 | `ROLE_BUILD` |
