# Architecture Decision Records

ADR 记录“为什么选择”，不替代当前规范。当前行为应同时满足已接受 ADR 和权威文档。

## 状态

- `PROPOSED`：待批准；
- `ACCEPTED`：当前有效；
- `SUPERSEDED`：已被后续 ADR 替代，仅保留历史；
- `REJECTED`：未采用；
- `DEPRECATED`：仍可能存在但计划移除。

## 索引

| ADR | 标题 | 状态 |
|---|---|---|
| [0001](0001-native-polyglot-monorepo.md) | Native polyglot monorepo | ACCEPTED |
| [0002](0002-library-first-machine-identity.md) | Library-first Machine Identity | ACCEPTED |
| [0003](0003-direct-nfpm-packaging.md) | Direct nFPM packaging | ACCEPTED |
| [0004](0004-snafu-unified-error-model.md) | SNAFU unified error model | ACCEPTED |
| [0005](0005-csv-only-import.md) | CSV-only import | ACCEPTED |
| [0006](0006-daemon-integrated-machine-identity-startup.md) | Daemon-integrated identity startup | ACCEPTED |
| [0007](0007-epoch-bound-session-lock.md) | Epoch-bound session lock | ACCEPTED |
| [0008](0008-visual-caddy-blocked-page.md) | Visual Caddy BLOCKED page | ACCEPTED |
| [0009](0009-single-lifetime-minimal-domain.md) | Single-lifetime minimal domain | ACCEPTED |
| [0010](0010-immutable-machine-id-and-device-lifecycle.md) | Immutable Machine ID and Device lifecycle | ACCEPTED |
| [0011](0011-application-encrypted-sqlite-vault.md) | Application-encrypted SQLite vault | ACCEPTED |
| [0012](0012-server-auth-enrollment-and-mtls-control.md) | Server-auth Enrollment and mTLS control | ACCEPTED |
| [0013](0013-explicit-state-and-secret-commands.md) | Explicit state and secret commands | ACCEPTED |
| [0014](0014-observed-snapshot-is-the-status-source.md) | Observed snapshot is status source | ACCEPTED |
| [0015](0015-home-backend-and-recovery.md) | Home backend and recovery | ACCEPTED |
| [0016](0016-gateway-certificate-issued-during-sync-state.md) | Gateway certificate during SYNC_STATE | ACCEPTED |
| [0017](0017-cross-desktop-session-agent-gui.md) | Cross-desktop Session Agent GUI | SUPERSEDED |
| [0018](0018-xdg-direct-slint-session-agent.md) | XDG direct Slint Session Agent | ACCEPTED |
| [0019](0019-stable-error-code-registry.md) | Stable ErrorCode registry | PROPOSED |

## 维护

- 新 ADR 使用 [`0000-template.md`](0000-template.md)；
- ID 单调递增，不复用；
- 接受后同步权威规范；
- supersede 时在旧 ADR 和新 ADR 双向链接；
- template 不维护当前项目索引；
- 多数 ADR 保持简短；详细测试和操作步骤留在 probe/runbook；
- 放宽 `INV-*` 必须在 ADR 中逐条说明安全影响。
