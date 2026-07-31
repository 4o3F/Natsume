# Natsume V2 实施路线图

> 状态：`ACTIVE-PLAN`
> 架构基线：Natsume V2 v2.7 决策集合
> 当前阶段：Phase 0
> 计划基线：44 周 + 4–6 周外部缓冲
> 注意：日期和周数是计划，不是完成声明

本文件只定义阶段结果、依赖、Gate 和交付证据。协议字段、证书规则、平台特例和测试用例分别由规范、平台、verification 和 probe 文档拥有。

## 1. 计划原则

1. 先冻结高风险边界，再构建业务功能。
2. 每个 Phase 只在其入口条件满足后开始关键路径。
3. Gate 缺证据即未通过，不接受“先通过后补证据”。
4. 目标环境输入未冻结时，受影响条目标记 `BLOCKED-INPUT`。
5. 文档、schema、自动化、package 和 runbook 都属于交付物。
6. Phase 文档说明工作包，不重复架构规范。
7. 一次只推进一个高风险垂直切片，避免多边界同时变更。
8. 外部依赖、硬件和现场时间保留 4–6 周缓冲。

## 2. 总览

| Phase | 计划窗口 | 主要结果 | Gate |
|---|---:|---|---|
| 0. Engineering Baseline | W1–W3 | 可重现仓库、真实 CI、契约骨架、空壳 Deb、高风险探针 | G0 |
| 1. Control Domain | W4–W8 | Server 领域、Auth/RBAC、审计、Operation/Command 骨架、Web shell | G1 |
| 2. CSV Preparation | W9–W12 | 严格 CSV、加密 staging、preview/commit、Preparation Center | G2 |
| 3. Identity & Enrollment | W13–W17 | Machine ID、identity-before-vault、Device-only Enrollment、PKI | G3 |
| 4. QUIC & Command Runtime | W18–W23 | mandatory-mTLS QUIC、journal、Observed、dispatcher、Gateway request context | G4 |
| 5. State, Gateway & Data Plane | W24–W30 | `SYNC_STATE`、Gateway cert、`SYNC_SECRET`、Caddy BLOCKED/READY、LKG | G5 |
| 6. Session & Home | W31–W37 | XDG/Slint Agent、session epoch、Home transaction、双桌面验证 | G6 |
| 7. Production Release | W38–W44 | packaging、升级、备份、容量、演练、发布签收 | G7 |
| External Buffer | +4–6 weeks | DOMjudge、硬件、桌面和现场依赖 | — |

详细工作包见 [`implementation/`](implementation/)。

## 3. 关键路径

```text
G0 platform/contract evidence
  → Server domain and audit
  → CSV committed truth
  → Device identity and vault
  → authenticated QUIC and durable Command
  → explicit state/secret and Caddy
  → Session/Home
  → production rehearsal and release
```

可并行工作：

- Web shell 可与 Server 基础领域并行；
- runbook 模板可提前建立；
- target environment 准备必须从 Phase 0 持续到 Phase 7；
- threat modeling、policy scan 和 packaging smoke 持续执行；
- DOMjudge contract lab 可与 Phase 3–4 并行，但不能绕过 Gate。

## 4. Phase 0：Engineering Baseline

### Phase 0 结果

- clean checkout 运行真实 Rust、Web、契约、策略和 package checks；
- toolchain/artifact pin 可审计；
- SNAFU + stable ErrorCode boundary；
- OpenAPI/Protobuf/D-Bus/SQL contract skeleton；
- Server/Client 空壳 Deb；
- 高风险 Probe A–F 的环境、步骤和 evidence 模板；
- XDG direct launch 与 Slint boundary 已冻结；
- G0 registry、platform 和 lab 状态可追踪。

### 不包含

- 生产领域 CRUD；
- 生产 CSV；
- 真实 fleet Command executor；
- 生产 Caddy generator；
- 完整 Session/Home；
- Gate 未签收时的支持声明。

### Gate G0

15 项全部 `PASS` 且 decision 已签署，才可关闭。当前机器状态见 [`gates/phase-0-status.md`](gates/phase-0-status.md)。

## 5. Phase 1：Control Domain

### Phase 1 结果

- Server migration 和模块边界；
- Seat/account/credential metadata、Device、binding、Target/Observed metadata；
- Server vault；
- operator auth/RBAC；
- AuditEvent + ChangeEvent/outbox；
- Operation/Command/Attempt persistence skeleton；
- operator API/OpenAPI；
- Web navigation、auth shell、通用 error/empty/loading patterns；
- Device/PKI mock adapters 供领域测试。

### Gate G1

- migration 空库/升级测试；
- 领域不变量和事务原子性；
- secret redaction；
- RBAC；
- audit/outbox；
- API generated clean diff；
- Web shell e2e；
- 模块依赖扫描。

## 6. Phase 2：CSV Preparation

### Phase 2 结果

- `seat,account,password` 固定完整 candidate 输入；
- encrypted staging；
- parse/normalize 与 Server 权威 redacted diff（Web 只渲染、不重分类）；
- duplicate account → `INVALID`；合法 account swap 允许；空/仅 header candidate → `INVALID`（不可经 CSV wipe 全部 Seat）；
- 可重复 Import Commit（二次确认）：baseline CAS、immutable preview evidence、binding impact（`binding_impact_count` 含显式零）、atomic unbind-and-replace；
- binding-stale reject（live binding 集合或 `AssignmentRevision` 与 preview evidence 不等须重新 preview）；
- no-op 与 material revision 规则（含 `ContestConfigurationRevision` / `AssignmentRevision` / `CredentialRevision`）；no-op 无内容变化 outbox；
- opaque preview token；password 与 password-derived digest 不进入普通 surface；
- Preparation Center 展示 required preview evidence；显式 voluntary discard（terminal discarded、token 不可复用、confirmed truth 不变）；
- commit audit；material 时 redacted outbox；transaction failure 仅原子回滚（无 historical rollback 产品）；
- 非秘密导出；
- CSV → Server truth only；始终 zero Command（无自动远端副作用）。

### Gate G2

- malformed/duplicate Seat/duplicate account INVALID/empty-or-header-only INVALID/extra-column/BOM/size tests；
- 合法 account swap；first / no-op（`OUTBOX_EVIDENCE=N/A`）/ material import；
- stale CAS、binding-stale、expiry、voluntary discard、idempotent retry、transaction atomic rollback；
- immutable preview evidence 与 commit 相等校验；binding impact 可见（count 含 0；>0 完整行）且 atomic unbind-and-replace；`AUTO_COMMAND_COUNT = 0`；
- secret 与 password-derived digest 不进入 API/log/audit/metric/SSE/outbox/browser；
- preview/commit 并发与 transaction rollback（非 historical rollback）；
- Web accessibility/e2e；Server classification authority（无本地重分类）；
- staging cleanup/recovery；
- CSV → Server truth 的完整 trace（Target/Drift 变化不表示 Device 已同步）。

## 7. Phase 3：Identity & Enrollment

### Phase 3 结果

- pure Machine ID library；
- privileged raw collectors；
- 6 台物理 fixture；
- identity file；
- identity-before-vault；
- Client vault；
- endpoint/preseed/trust；
- server-auth HTTPS Enrollment；
- Device-only CSR/leaf/chain；
- Device lifecycle/replacement；
- control PKI provisioning runbook。

### Gate G3

- identity decision table；
- configured-disk copy；
- vault tamper/wrong-key/crash；
- Enrollment 无 Gateway material；
- IP-SAN 正反；
- certificate profile/expiry/revocation；
- package upgrade 保留 endpoint/identity；
- replacement rehearsal。

## 8. Phase 4：QUIC & Command Runtime

### Phase 4 结果

- mandatory-mTLS QUIC；
- exact wire/framing；
- anonymous rejection before decoder；
- 0-RTT disabled；
- connection registry；
- durable Command dispatcher；
- Device journal；
- receipt/status/Attempt；
- Observed snapshot；
- retry/reconnect/idempotency；
- Gateway request 上下文校验骨架。

### Gate G4

- framing/size/version/unknown enum；
- TLS identity lifecycle；
- duplicate delivery/crash recovery；
- stale/conflict；
- reconnect；
- multi-device simulator；
- Gateway request 只能绑定 active `SYNC_STATE`；
- ErrorCode across transports。

## 9. Phase 5：State, Gateway & Data Plane

### Phase 5 结果

- Target derivation/generation/hash；
- explicit `SYNC_STATE`；
- Gateway key/CSR；
- Server-derived SAN/profile；
- certificate validation；
- Caddy BLOCKED/READY；
- fixed DOMjudge upstream；
- LKG；
- explicit `SYNC_SECRET`；
- Client vault credential revision；
- Drift 和 operator views。

### Gate G5

- full certificate ladder；
- request/SPKI idempotency/conflict；
- Caddy validate/load/rollback；
- bad cert/key/SAN；
- offline LKG；
- secret stale/retry/redaction；
- Observed/Drift；
- DOMjudge contract；
- failure injection。

## 10. Phase 6：Session & Home

### Phase 6 结果

- package-owned XDG Autostart；
- resident hidden Session Agent；
- build-time Slint winit + Skia；
- local typed D-Bus；
- logind session validation；
- singleton/lease；
- lazy UI 和 focus result；
- session epoch lock/unlock/terminate；
- fixed contest user；
- selected Home backend；
- crash/reboot recovery；
- Session 与 Caddy 解耦证据。

### Gate G6

- GNOME/GDM/Wayland；
- LightDM + selected X11 desktop；
- 中文/IME、HiDPI、focus denied；
- display lost/Agent crash；
- no user unit/no descriptor；
- current-session/epoch race；
- lock/unlock Caddy call count = 0；
- Home prepare/cleanup/fault/reboot；
- package runtime closure。

## 11. Phase 7：Production Release

### Phase 7 结果

- production Deb；
- installation/preseed/reconfigure；
- upgrade/rollback；
- backup/restore；
- PKI ceremony；
- observability；
- capacity；
- security hardening；
- operator/admin runbooks；
- readiness rehearsal；
- release artifacts、checksums、SBOM/依赖记录；
- support matrix签收。

### Gate G7

- clean-site rehearsal；
- failure and recovery scenarios；
- restore to verified state；
- package lifecycle；
- offline steady state；
- capacity/SLO；
- audit export；
- security review；
- operator training；
- release decision signed。

## 12. Gate 证据标准

每条 Gate evidence 至少包含：

```text
GATE_ID
REQ_IDS
JUDGEMENT
COMMIT_SHA
ENVIRONMENT_OR_HW_ID
TEST_OR_PROBE
ARTIFACT_PATH_OR_CI_URL
OWNER
REVIEWER
DATE
LIMITATIONS
```

原则：

- 文件存在不等于通过；
- 截图不能替代可复现日志；
- VM 不能替代物理 Machine ID；
- scaffold 不能替代实现；
- repo pin 不能替代环境冻结；
- partial pass 不关闭整个 Gate；
- waiver 不能放宽安全不变量。

## 13. 风险与缓冲

| 风险 | 缓解 |
|---|---|
| 目标 OS/桌面晚冻结 | Phase 0 输入门禁；双环境提前预约 |
| 物理硬件不足 | 6 台槽位和 OEM/storage 覆盖提前到位 |
| DOMjudge contract 变化 | 固定版本、adapter contract、独立 lab |
| Caddy module/supply drift | version/source/checksum/module closure |
| PKI ceremony延迟 | owner、离线材料和 runbook 提前准备 |
| Home backend 不可用 | 两候选，部署前 ADR 选择，不 runtime fallback |
| GUI runtime closure 膨胀 | Slint feature allowlist、package scan |
| 文档/状态漂移 | registry 生成、link/diagram/contract clean diff |
| 多边界同时开发 | 以垂直切片和 Gate 控制合并 |
| 现场时间不足 | 4–6 周外部缓冲和 rehearsal |

## 14. 计划变更

以下变更需要更新本文件：

- Phase 顺序；
- Gate 退出标准；
- critical path；
- 计划窗口；
- 外部缓冲。

以下内容不在本文件维护：

- wire 字段；
- `INV-*` 正文；
- exact package version；
- 平台 PASS；
- 详细负向案例；
- runbook 命令；
- requirement 当前状态。

这些分别由契约、安全、平台、probe/runbook 和 verification registry 拥有。
