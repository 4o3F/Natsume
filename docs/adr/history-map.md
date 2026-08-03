# Historical ADR mapping

ADR-0001～ADR-0029 已于当前 consolidation 中退出 Git。下表只保留旧 ID 的语义索引，用于解释历史提交、issue、注释和外部引用；当前行为必须从“当前主题 ADR”和“权威规范”读取。

| 旧 ID | 原标题 | 原状态 / 历史关系 | 当前主题 ADR | 权威规范 |
|---|---|---|---|---|
| `ADR-0001` | Native polyglot monorepo | ACCEPTED | [ADR-0030](0030-foundation-deployment-and-delivery-baseline.md) | [repository-layout.md](../repository-layout.md)、[dependency-policy.md](../dependency-policy.md) |
| `ADR-0002` | Library-first Machine Identity | ACCEPTED；source admission 后被 ADR-0025 收窄 | [ADR-0032](0032-device-identity-and-local-credential-lifecycle.md) | [architecture.md](../architecture.md)、[security-recovery.md](../security-recovery.md) |
| `ADR-0003` | Direct nFPM packaging | ACCEPTED | [ADR-0030](0030-foundation-deployment-and-delivery-baseline.md) | [repository-layout.md](../repository-layout.md)、[dependency-policy.md](../dependency-policy.md) |
| `ADR-0004` | SNAFU unified error model | ACCEPTED | [ADR-0036](0036-error-architecture-and-public-codes.md) | [contracts.md](../contracts.md)、[dependency-policy.md](../dependency-policy.md) |
| `ADR-0005` | CSV-only import | SUPERSEDED by ADR-0020 | [ADR-0031](0031-contest-import-and-secret-evidence.md) | [domain-model.md](../domain-model.md)、[contracts.md](../contracts.md) |
| `ADR-0006` | Daemon-integrated Machine Identity startup | ACCEPTED；credential 术语后被 ADR-0026 更新 | [ADR-0032](0032-device-identity-and-local-credential-lifecycle.md) | [architecture.md](../architecture.md)、[security-recovery.md](../security-recovery.md) |
| `ADR-0007` | Epoch-bound Session Lock | ACCEPTED | [ADR-0035](0035-session-home-and-desktop-cycle.md) | [state-and-execution.md](../state-and-execution.md)、[security-recovery.md](../security-recovery.md) |
| `ADR-0008` | Visual Caddy BLOCKED page | ACCEPTED | [ADR-0034](0034-state-execution-and-data-plane-boundary.md) | [state-and-execution.md](../state-and-execution.md)、[security-recovery.md](../security-recovery.md) |
| `ADR-0009` | Single-lifetime minimal domain | ACCEPTED | [ADR-0030](0030-foundation-deployment-and-delivery-baseline.md) | [architecture.md](../architecture.md)、[domain-model.md](../domain-model.md) |
| `ADR-0010` | Immutable Machine ID and Device lifecycle | ACCEPTED | [ADR-0032](0032-device-identity-and-local-credential-lifecycle.md) | [domain-model.md](../domain-model.md)、[security-recovery.md](../security-recovery.md) |
| `ADR-0011` | Application-encrypted SQLite vault | ACCEPTED；Client vault 条款被 ADR-0026 替代 | [ADR-0032](0032-device-identity-and-local-credential-lifecycle.md) | [security-recovery.md](../security-recovery.md) |
| `ADR-0012` | Server-auth Enrollment and mTLS control | SUPERSEDED by ADR-0023 | [ADR-0033](0033-enrollment-and-device-control-boundary.md) | [contracts.md](../contracts.md)、[security-recovery.md](../security-recovery.md) |
| `ADR-0013` | Explicit state and secret commands | ACCEPTED | [ADR-0034](0034-state-execution-and-data-plane-boundary.md) | [state-and-execution.md](../state-and-execution.md) |
| `ADR-0014` | Observed snapshot is the status source | ACCEPTED | [ADR-0034](0034-state-execution-and-data-plane-boundary.md) | [domain-model.md](../domain-model.md)、[state-and-execution.md](../state-and-execution.md) |
| `ADR-0015` | Home backend and recovery | ACCEPTED | [ADR-0035](0035-session-home-and-desktop-cycle.md) | [state-and-execution.md](../state-and-execution.md)、[security-recovery.md](../security-recovery.md) |
| `ADR-0016` | Gateway certificate issued during SYNC_STATE | SUPERSEDED by ADR-0021 | [ADR-0033](0033-enrollment-and-device-control-boundary.md) | [contracts.md](../contracts.md)、[security-recovery.md](../security-recovery.md) |
| `ADR-0017` | Cross-desktop Session Agent GUI | SUPERSEDED by ADR-0018 | [ADR-0035](0035-session-home-and-desktop-cycle.md) | [architecture.md](../architecture.md)、[dependency-policy.md](../dependency-policy.md) |
| `ADR-0018` | XDG direct Slint Session Agent | ACCEPTED；持续双桌面义务被 ADR-0027 收窄 | [ADR-0035](0035-session-home-and-desktop-cycle.md) | [architecture.md](../architecture.md)、[dependency-policy.md](../dependency-policy.md) |
| `ADR-0019` | Stable ErrorCode registry | PROPOSED；由 ADR-0036 正式接受 | [ADR-0036](0036-error-architecture-and-public-codes.md) | [contracts.md](../contracts.md)、[dependency-policy.md](../dependency-policy.md) |
| `ADR-0020` | Repeatable contest configuration import | ACCEPTED；并发/证据条款被 ADR-0028 收窄 | [ADR-0031](0031-contest-import-and-secret-evidence.md) | [domain-model.md](../domain-model.md)、[contracts.md](../contracts.md)、[security-recovery.md](../security-recovery.md) |
| `ADR-0021` | Provisioning-window certificate issuance | ACCEPTED | [ADR-0033](0033-enrollment-and-device-control-boundary.md) | [contracts.md](../contracts.md)、[security-recovery.md](../security-recovery.md) |
| `ADR-0022` | Deployment facts and trust assumptions | ACCEPTED | [ADR-0030](0030-foundation-deployment-and-delivery-baseline.md) | [architecture.md](../architecture.md)、[supported-platform.md](../supported-platform.md) |
| `ADR-0023` | WSS control channel with Device Token | ACCEPTED | [ADR-0033](0033-enrollment-and-device-control-boundary.md) | [contracts.md](../contracts.md)、[security-recovery.md](../security-recovery.md) |
| `ADR-0024` | DOMjudge autologin via X-Headers | ACCEPTED | [ADR-0034](0034-state-execution-and-data-plane-boundary.md) | [contracts.md](../contracts.md)、[state-and-execution.md](../state-and-execution.md) |
| `ADR-0025` | Deterministic hardware identity recipe | ACCEPTED | [ADR-0032](0032-device-identity-and-local-credential-lifecycle.md) | [security-recovery.md](../security-recovery.md) |
| `ADR-0026` | Client secrets as permission files | ACCEPTED | [ADR-0032](0032-device-identity-and-local-credential-lifecycle.md) | [security-recovery.md](../security-recovery.md) |
| `ADR-0027` | Single-image desktop cycle | ACCEPTED | [ADR-0035](0035-session-home-and-desktop-cycle.md) | [architecture.md](../architecture.md)、[supported-platform.md](../supported-platform.md) |
| `ADR-0028` | Single-operator import and secret-evidence scope | ACCEPTED | [ADR-0031](0031-contest-import-and-secret-evidence.md) | [domain-model.md](../domain-model.md)、[contracts.md](../contracts.md)、[security-recovery.md](../security-recovery.md) |
| `ADR-0029` | Right-sizing control-plane machinery | ACCEPTED | [ADR-0034](0034-state-execution-and-data-plane-boundary.md) | [architecture.md](../architecture.md)、[domain-model.md](../domain-model.md)、[state-and-execution.md](../state-and-execution.md)、[roadmap.md](../roadmap.md) |
