# Architecture Decision Records

ADR 记录“为什么选择”，不替代当前规范。当前行为应同时满足已接受 ADR 和权威文档。

## 状态

- `PROPOSED`：待批准；
- `ACCEPTED`：当前有效；
- `SUPERSEDED`：已被后续 ADR 替代，仅保留历史；
- `REJECTED`：未采用；
- `DEPRECATED`：仍可能存在但计划移除。

## 索引

类别说明：**边界** = 安全/授权/fail-closed 不变量（已冻结）；**实现** = 工具/产品范围/wire 形式化决策，细节待对应 Phase 证据冻结；**历史** = 已 supersede。

| ADR | 标题 | 状态 | 类别 | 主属 Phase |
|---|---|---|---|---|
| [0001](0001-native-polyglot-monorepo.md) | Native polyglot monorepo | ACCEPTED | 实现 | P0 |
| [0002](0002-library-first-machine-identity.md) | Library-first Machine Identity | ACCEPTED | 边界 | P3 |
| [0003](0003-direct-nfpm-packaging.md) | Direct nFPM packaging | ACCEPTED | 实现 | P0 / P7 |
| [0004](0004-snafu-unified-error-model.md) | SNAFU unified error model | ACCEPTED | 实现 | P0 |
| [0005](0005-csv-only-import.md) | CSV-only import | SUPERSEDED | 历史 | P2 |
| [0006](0006-daemon-integrated-machine-identity-startup.md) | Daemon-integrated identity startup | ACCEPTED | 边界 | P3 |
| [0007](0007-epoch-bound-session-lock.md) | Epoch-bound session lock | ACCEPTED | 边界 | P6 |
| [0008](0008-visual-caddy-blocked-page.md) | Visual Caddy BLOCKED page | ACCEPTED | 边界 | P5 |
| [0009](0009-single-lifetime-minimal-domain.md) | Single-lifetime minimal domain | ACCEPTED | 实现 | P1 |
| [0010](0010-immutable-machine-id-and-device-lifecycle.md) | Immutable Machine ID and Device lifecycle | ACCEPTED | 边界 | P3 |
| [0011](0011-application-encrypted-sqlite-vault.md) | Application-encrypted SQLite vault | ACCEPTED | 边界 | P1 / P3 |
| [0012](0012-server-auth-enrollment-and-mtls-control.md) | Server-auth Enrollment and mTLS control | ACCEPTED | 边界 | P3 / P4 |
| [0013](0013-explicit-state-and-secret-commands.md) | Explicit state and secret commands | ACCEPTED | 边界 | P2 / P5 |
| [0014](0014-observed-snapshot-is-the-status-source.md) | Observed snapshot is status source | ACCEPTED | 实现 | P4 / P5 |
| [0015](0015-home-backend-and-recovery.md) | Home backend and recovery | ACCEPTED | 边界 | P6 |
| [0016](0016-gateway-certificate-issued-during-sync-state.md) | Gateway certificate during SYNC_STATE | ACCEPTED | 边界 | P4 / P5 |
| [0017](0017-cross-desktop-session-agent-gui.md) | Cross-desktop Session Agent GUI | SUPERSEDED | 历史 | P6 |
| [0018](0018-xdg-direct-slint-session-agent.md) | XDG direct Slint Session Agent | ACCEPTED | 实现 | P0 / P6 |
| [0019](0019-stable-error-code-registry.md) | Stable ErrorCode registry | PROPOSED | 实现 | P0 |
| [0020](0020-repeatable-contest-configuration-import.md) | Repeatable contest configuration import | ACCEPTED | 实现 | P2 |

## 维护

- 新 ADR 使用 [`0000-template.md`](0000-template.md)；
- ID 单调递增，不复用；
- 接受后同步权威规范；
- supersede 时在旧 ADR 和新 ADR 双向链接；
- template 不维护当前项目索引；
- 多数 ADR 保持简短；详细测试和操作步骤留在实现与验证产物中；
- 放宽 `INV-*` 必须在 ADR 中逐条说明安全影响。

## 已废弃的引用

ADR 正文是不可变的历史记录，不因后续文档重组而改写。已接受的 ADR 中出现的 `Probe A`–`Probe F` 与 `REQ-P0-*` / `G0-0NN` 编号来自已撤销的 registry 与 probe 报告体系；它们表达的验证意图仍然有效，当前状态以 [`../gates/phase-0-status.md`](../gates/phase-0-status.md) 为准：

| 旧编号 | 验证主题 |
|---|---|
| Probe A | IP-SAN 与 endpoint |
| Probe B | Enrollment → mTLS → Gateway CSR 证书阶梯 |
| Probe C | Caddy 与 DOMjudge 数据面 |
| Probe D | Machine identity 与物理硬件 fixture |
| Probe E | Session Agent、双桌面与 Home |
| Probe F | Package 与 systemd 生命周期 |
