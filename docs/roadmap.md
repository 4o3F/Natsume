# Natsume V2 实施路线图

> 状态：`ACTIVE-PLAN`
> 架构基线：Natsume V2 v2.7 决策集合
> 当前阶段：Phase 0
> 计划基线：44 周 + 4–6 周外部缓冲
> 注意：日期和周数是计划，不是完成声明

本文件定义阶段结果、依赖、Gate 和交付证据。协议字段、证书规则、平台状态和安全不变量分别由 [契约](contracts.md)、[安全与恢复](security-recovery.md) 和 [平台支持](supported-platform.md) 拥有。

## 1. 计划原则

1. 先冻结高风险边界，再构建业务功能。
2. Gate 缺证据即未通过，不接受"先通过后补证据"。
3. 目标环境输入未冻结时，受影响条目标记 `BLOCKED-INPUT`。
4. 一次只推进一个高风险垂直切片，避免多边界同时变更。
5. 只有当前 Phase 保留详细工作包；后续 Phase 只记录结果与 Gate 主题，细目在启动时定义。
6. 外部依赖、硬件和现场时间保留 4–6 周缓冲。

## 2. 总览

| Phase | 计划窗口 | 主要结果 | Gate |
|---|---:|---|---|
| 0. Engineering Baseline | W1–W3 | 可重现仓库、真实 CI、契约骨架、空壳 Deb、高风险探针 | G0 |
| 1. Control Domain | W4–W8 | Server 领域、Auth/RBAC、审计、Operation/Command 骨架、Web shell | G1 |
| 2. CSV Preparation | W9–W12 | 严格 CSV、加密 staging、preview/commit、Preparation Center | G2 |
| 3. Identity & Enrollment | W13–W17 | Machine ID、identity-before-vault、Device-only Enrollment、PKI | G3 |
| 4. QUIC & Command Runtime | W18–W23 | mandatory-mTLS QUIC、journal、Observed、dispatcher | G4 |
| 5. State, Gateway & Data Plane | W24–W30 | `SYNC_STATE`、Gateway cert、`SYNC_SECRET`、Caddy BLOCKED/READY、LKG | G5 |
| 6. Session & Home | W31–W37 | XDG/Slint Agent、session epoch、Home transaction、双桌面验证 | G6 |
| 7. Production Release | W38–W44 | packaging、升级、备份、容量、演练、发布签收 | G7 |
| External Buffer | +4–6 weeks | DOMjudge、硬件、桌面和现场依赖 | — |

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

可并行：Web shell 与 Server 基础领域；threat modeling、policy scan 和 packaging smoke 持续执行；target environment 准备从 Phase 0 持续到 Phase 7；DOMjudge contract lab 可与 Phase 3–4 并行，但不能绕过 Gate。

## 4. Phase 0：Engineering Baseline（当前）

Phase 0 不实现产品功能，而是消除会在后续放大的工具链、契约、平台和安全不确定性。

### 工作包

- **P0.1 Monorepo 与工具链**：Cargo/pnpm/`just`/nFPM 所有权边界；固定 Rust、Node、pnpm、Mermaid、nFPM、Caddy、protoc；单一 lockfile；禁止占位命令和"工具缺失即跳过"。
- **P0.2 真实 CI**：PR 执行 Rust（fmt/clippy/test/doc/deny）、Web（frozen install/format/lint/typecheck/test/build）、契约 clean diff、policy scan、package smoke。Nightly 执行 install/upgrade/remove/purge/reboot 与 runtime closure。
- **P0.3 Error model**：第一方 typed SNAFU error；stable ErrorCode registry；HTTP Problem Details 与 Protobuf/D-Bus/CommandStatus 显式映射；redaction tests；禁止解析 Display 文本。
- **P0.4 Contract skeleton**：Device-only Enrollment、control envelope/framing、Observed/CommandStatus、Local D-Bus、SQL migrations、machine-generated golden、certificate ladder 负向 contract test。
- **P0.5 空壳 Deb**：`natsume-server` 与 `natsume-client` 可构建/安装；sysusers/tmpfiles/mode、systemd services、D-Bus policy、package-owned Caddy、XDG Autostart、endpoint preseed；无 Identity Guard、无 Agent user unit、无 runtime download、无 postinstall CA 生成。
- **P0.6 目标环境验证**：在目标 OS、双桌面和物理硬件上验证 IP-SAN/endpoint、证书阶梯、Caddy/DOMjudge、Machine identity、Session/Home、package/systemd。结果必须是可复现命令、日志或 artifact；截图和文档存在都不构成证据。

### Definition of Done

clean checkout 的 mandatory CI 真实运行；toolchain/artifact pin 可审计；生成契约 clean；package smoke 真实；forbidden path 由 policy/negative tests 覆盖；目标环境和物理机已冻结；G0 全部条目 PASS。

### 非目标

领域 CRUD、生产 Auth/RBAC/SSE、生产 CSV、fleet-scale command executor、生产 Caddy generator、完整 Session/Home；以 scaffold 宣称 Phase 6 完成；在 G0 未通过时发布支持矩阵。

### 主要风险

| 风险 | 控制 |
|---|---|
| 工具链只在开发机工作 | locked CI + clean checkout |
| 协议文档与 code 漂移 | generated descriptor/OpenAPI/D-Bus clean diff |
| 证书阶梯被简化 | `INV-CERT-01/02` + 目标环境负向验证 |
| 桌面环境太晚验证 | `G0-IN-004` 输入门禁 |
| Machine ID 无物理证据 | `G0-IN-005` + 6 台物理 fixture |
| package 拓扑只在文档 | install/upgrade/reboot smoke |

当前 Gate 与输入门禁状态见 [`gates/phase-0-status.md`](gates/phase-0-status.md)。

## 5. Phase 1–7：结果与 Gate 主题

后续 Phase 只记录目标结果与 Gate 覆盖主题。详细工作包、验收细目和测试场景在该 Phase 启动时定义——在此之前它们是推测，不是设计。

### Phase 1：Control Domain

Server migration 与模块边界、Seat/account/credential metadata、Device 与 binding、Server vault、operator auth/RBAC、AuditEvent + ChangeEvent/outbox、operator API/OpenAPI、Web navigation 与 auth shell。

**G1 覆盖**：migration（空库与升级）、领域不变量与事务原子性、secret redaction、RBAC、audit/outbox 原子性、API generated clean diff、模块依赖扫描。

### Phase 2：CSV Preparation

`seat,account,password` 固定完整 candidate、encrypted staging、Server 权威 redacted diff、可重复 Import Commit（baseline CAS、immutable preview evidence、atomic unbind-and-replace）、opaque preview token、Preparation Center。

**G2 覆盖**：malformed/duplicate/empty candidate 拒绝、first/no-op/material import 区分、stale CAS 与 binding-stale 拒绝、幂等重试与事务回滚、secret 与 password-derived digest 不进入任何 ordinary surface、CSV → Server truth 且零自动 Command。

### Phase 3：Identity & Enrollment

pure Machine ID library、privileged raw collectors、identity file、identity-before-vault、Client vault、server-auth HTTPS Enrollment、Device-only CSR/leaf/chain、Device lifecycle 与 replacement。

**G3 覆盖**：identity 决策表全路径、configured-disk copy、vault tamper/wrong-key/crash、Enrollment 无 Gateway material、IP-SAN 正反、certificate profile/expiry/revocation、package upgrade 保留 identity。

### Phase 4：QUIC & Command Runtime

mandatory-mTLS QUIC、exact wire/framing、anonymous rejection before decoder、0-RTT disabled、durable Command dispatcher、Device journal、receipt/status、Observed snapshot、retry/reconnect/idempotency。

**G4 覆盖**：framing/size/version/unknown enum、TLS identity lifecycle、duplicate delivery 与 crash recovery、stale/conflict 拒绝、reconnect 收敛、ErrorCode 跨 transport 一致。

### Phase 5：State, Gateway & Data Plane

Target derivation/generation、explicit `SYNC_STATE`、Gateway key/CSR 与 Server-derived SAN/profile、certificate validation、Caddy BLOCKED/READY、fixed DOMjudge upstream、LKG、explicit `SYNC_SECRET`、Drift 与 operator views。

**G5 覆盖**：完整证书阶梯、request/SPKI 幂等与冲突、Caddy validate/load/rollback、bad cert/key/SAN 拒绝、offline LKG、secret stale/retry/redaction、DOMjudge contract、故障注入。

### Phase 6：Session & Home

package-owned XDG Autostart、resident hidden Session Agent、build-time Slint winit + Skia、local typed D-Bus、logind session validation、singleton/lease、session epoch lock/unlock/terminate、固定 contest user、选定 Home backend。

**G6 覆盖**：双桌面（GNOME/GDM/Wayland 与 LightDM/X11）、中文 IME/HiDPI/focus denied、display lost 与 Agent crash、无 user unit、current-session/epoch race、lock/unlock 的 Caddy 调用数为 0、Home prepare/cleanup/fault/reboot。

### Phase 7：Production Release

production Deb、installation/preseed/reconfigure、upgrade/rollback、backup/restore、PKI ceremony、observability、capacity、security hardening、operator/admin runbooks、readiness rehearsal、release artifacts 与 SBOM。

**G7 覆盖**：clean-site rehearsal、失败与恢复场景、restore to verified state、package lifecycle、offline steady state、capacity/SLO、audit export、security review、release decision 签署。

## 6. Gate 证据标准

每条 Gate evidence 至少包含：

```text
GATE_ID
JUDGEMENT
COMMIT_SHA
ENVIRONMENT_OR_HW_ID
TEST_OR_PROBE
ARTIFACT_PATH_OR_CI_URL
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

## 7. 风险与缓冲

| 风险 | 缓解 |
|---|---|
| 目标 OS/桌面晚冻结 | Phase 0 输入门禁；双环境提前预约 |
| 物理硬件不足 | 6 台槽位和 OEM/storage 覆盖提前到位 |
| DOMjudge contract 变化 | 固定版本、adapter contract、独立 lab |
| Caddy module/supply drift | version/source/checksum/module closure |
| PKI ceremony 延迟 | owner、离线材料和 runbook 提前准备 |
| Home backend 不可用 | 两候选，部署前 ADR 选择，不 runtime fallback |
| GUI runtime closure 膨胀 | Slint feature allowlist、package scan |
| 多边界同时开发 | 以垂直切片和 Gate 控制合并 |
| 现场时间不足 | 4–6 周外部缓冲和 rehearsal |

## 8. 计划变更

需要更新本文件：Phase 顺序、Gate 退出标准、critical path、计划窗口、外部缓冲。

不在本文件维护：wire 字段、`INV-*` 正文、exact package version、平台 PASS、详细负向案例、runbook 命令。
