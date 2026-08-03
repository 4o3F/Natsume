# Natsume V2 实施路线图

> 状态：`ACTIVE-PLAN`
> 架构基线：Natsume V2 当前主题决策集合（[ADR-0030](adr/0030-foundation-deployment-and-delivery-baseline.md)–[0036](adr/0036-error-architecture-and-public-codes.md)）
> 当前阶段：Phase 0（收尾）
> 计划基线：26 周 + 2–4 周外部缓冲（约 6 个月开发窗口，3 人，ADR-0030 F7）
> 注意：日期和周数是计划，不是完成声明

本文件定义阶段结果、依赖、Gate 和交付证据。协议语义、证书规则、平台状态和安全不变量分别由 [契约](contracts.md)、[安全与恢复](security-recovery.md) 和 [平台支持](supported-platform.md) 拥有。

## 1. 计划原则

1. 先冻结高风险边界，再构建业务功能。
2. Gate 缺证据即未通过；证据 = 指向 CI run / commit / artifact 的链接 + 一行结论（[ADR-0034](adr/0034-state-execution-and-data-plane-boundary.md)），不接受"先通过后补证据"。
3. 目标环境输入未冻结时，受影响条目标记 `BLOCKED-INPUT`。
4. 一次只推进一个高风险垂直切片，避免多边界同时变更。
5. 只有当前 Phase 保留详细工作包；后续 Phase 只记录结果与 Gate 主题，细目在启动时定义。
6. 外部依赖、镜像和现场时间保留 2–4 周缓冲。

## 2. 总览

| Phase | 计划窗口 | 主要结果 | Gate |
|---|---:|---|---|
| 0. Engineering Baseline（收尾） | W1–W2 | 可重现仓库、真实 CI、current-state schema 与 Command-ID 契约声明、WSS/token 骨架、空壳 Deb、输入冻结、DOMjudge lab 启动 | G0 |
| 1. Control Domain | W3–W6 | Server 领域、admin/viewer auth、审计、Command 骨架、Web shell | G1 |
| 2. CSV Preparation | W7–W9 | 严格 CSV、加密 staging、单 pending + 双 CAS import、Preparation Center | G2 |
| 3. Identity & Enrollment | W10–W12 | 固定配方 Machine ID、provisioning 窗口、Token + Gateway 联合签发、凭据文件 | G3 |
| 4. Control Channel & Command Runtime | W13–W15 | WSS + token 认证、journal、Observed、dispatcher | G4 |
| 5. State, Data Plane & Secrets | W16–W19 | `SYNC_STATE`、Caddy 渲染/reload/LKG、xheaders 自动登录、`SYNC_SECRET`、Drift 视图 | G5 |
| 6. Session & Home | W20–W23 | 当期镜像单桌面 Agent、session epoch、Home transaction | G6 |
| 7. Production Release | W24–W26 | packaging、升级、备份、演练、发布签收、赛后导出 | G7 |
| External Buffer | +2–4 weeks | DOMjudge、镜像、硬件和现场依赖 | — |

## 3. 关键路径

```text
G0 platform/contract evidence + DOMjudge lab（xheaders/brotli/upstream TLS）
  → Server domain and audit
  → CSV committed truth
  → identity recipe + window + enrollment（token/gateway）
  → WSS control and durable Command
  → explicit state/secret and Caddy autologin
  → Session/Home（当期镜像）
  → production rehearsal and release
```

可并行：Web shell 与 Server 基础领域；policy scan 和 packaging smoke 持续执行；target environment 准备从 Phase 0 持续到 Phase 7；**DOMjudge contract lab（xheaders 登录、brotli 透传、upstream TLS）自 Phase 0 收尾即启动**，与 Phase 1–4 并行，其结论必须在 Phase 5 开始前冻结。

## 4. Phase 0：Engineering Baseline（当前，收尾）

Phase 0 不实现产品功能，而是消除会在后续放大的工具链、契约、平台和安全不确定性。v2.8 裁剪后的存量工作：

### 工作包

- **P0.1 Monorepo 与工具链**：Cargo/pnpm/`just`/nFPM 所有权边界；固定 Rust、Node、pnpm、Mermaid、nFPM、Caddy、protoc；单一 lockfile；禁止占位命令和"工具缺失即跳过"。
- **P0.2 真实 CI**：PR 执行 Rust（fmt/clippy/test/doc/deny）、Web（frozen install/format/lint/typecheck/test/build）、契约 clean diff、policy scan、package smoke。完整 install/upgrade/remove/purge/reboot 生命周期改为每周与发版前执行。
- **P0.3 Error model**：第一方 typed SNAFU error；stable ErrorCode registry；HTTP Problem Details 与 Protobuf/D-Bus/CommandStatus 显式映射；redaction tests；禁止解析 Display 文本。
- **P0.4 Contract skeleton（v2.8 重定向）**：current-only SQL 基线（无 Seat-universe freeze、generic instance state 或未消费的 workflow history）；窗口门禁 Enrollment（token + gateway）；Panel-owned canonical UUIDv7 `command_id` 与声明式 `PUT /api/v2/commands/{command_id}`（`201/200/400/409`、same-ID fingerprint conflict、`request_fingerprint_*` 与 `frozen_payload_json`）；WSS envelope（一帧一消息）、Observed/CommandStatus、Local D-Bus、machine-generated golden、`INV-CERT-01` 两段阶梯负向 contract test。该工作包只冻结 migration/schema/contract，不宣称 HTTP listener、Command repository/dispatcher、Device journal 或 Panel mutation 已实现。**删除**：QUIC/framing 骨架、Device Identity CSR 契约、mTLS client verifier 骨架。
- **P0.5 空壳 Deb**：`natsume-server` 与 `natsume-client` 可构建/安装；sysusers/tmpfiles/mode、systemd services、D-Bus policy、package-owned Caddy、XDG Autostart、endpoint/hostname preseed；无 Identity Guard、无 Agent user unit、无 runtime download、无 postinstall CA 生成。
- **P0.6 目标环境验证（v2.8 收缩）**：在目标 OS（ICPC 派生镜像）与 Server 网络上验证：IP-SAN/endpoint 与单 TCP 端口、窗口签发阶梯 schema 负向断言、**DOMjudge lab：xheaders 登录、brotli 透传、upstream TLS**、identity fixture（v1 事故 + 代表性异构）、当期桌面 capability 清单、package/systemd。结果必须是可复现命令、日志或 artifact。

### Definition of Done（G0）

clean checkout 的 mandatory CI 真实运行；toolchain/artifact pin 可审计；生成契约 clean；package smoke 真实；forbidden path 由 policy/negative tests 覆盖；`G0-IN-001`–`G0-IN-007` 全部冻结；DOMjudge lab 三项结论可定位。

### 非目标

领域 CRUD、生产 Auth、生产 CSV、生产 Caddy generator、完整 Session/Home；以 scaffold 宣称后续 Phase 完成；在 G0 未通过时发布支持矩阵。

### 主要风险

| 风险 | 控制 |
|---|---|
| 工具链只在开发机工作 | locked CI + clean checkout |
| 协议文档与 code 漂移 | generated descriptor/OpenAPI/D-Bus clean diff |
| 签发阶梯被简化 | `INV-CERT-01/02` + 窗口负向验证 |
| DOMjudge xheaders 契约不符 | lab 提前到 P0 收尾，结论冻结后才建 Phase 5 |
| 镜像晚到/大版本变化 | `G0-IN-001/004` 输入门禁 + 重验清单（ADR-0035） |
| package 拓扑只在文档 | install/upgrade/reboot smoke |

当前 Gate 与输入门禁状态见 [`gates/phase-0-status.md`](gates/phase-0-status.md)。

## 5. Phase 1–7：结果与 Gate 主题

后续 Phase 只记录目标结果与 DoD 覆盖主题。详细工作包、验收细目和测试场景在该 Phase 启动时定义——在此之前它们是推测，不是设计。

### Phase 1：Control Domain

Server current-state migration 与模块边界、Seat/account/current-credential metadata、`account_mappings`、Device 与当前 Binding、Server vault、operator auth（admin/viewer）、只含 correlation/redaction envelope 的 AuditEvent（event-specific revision/count 在 `redacted_detail_json`，由 guarded operation 与领域 mutation 同事务写入）、operator API/OpenAPI、Web navigation 与 auth shell（轮询）。

**G1 覆盖**：migration（空库与升级）、领域不变量与事务原子性、secret redaction、两角色授权、audit 原子性、API generated clean diff、模块依赖扫描。

### Phase 2：CSV Preparation

`seat,account,password` 固定完整 candidate、encrypted staging、严格解析后唯一 `pending_import_candidate`、Server 权威 redacted diff、双 CAS Import Commit、atomic unbind-and-replace、commit/discard/expiry 的 candidate+payload 终态删除、opaque preview token、Preparation Center。Seat→Account mapping 只推进 `revision_counters.configuration_revision`；Seat↔Device Binding-set mutation 才推进全局 `BindingRevision`。

**G2 覆盖**：malformed/duplicate/empty candidate 拒绝、单 pending mutual exclusion、first/no-op/material 区分、双 CAS 拒绝与重复提交安全失败、candidate/payload 终态删除、current-only credential/mapping、事务回滚、password 明文不进任何普通 surface、CSV → Server truth 且零自动 Command。

### Phase 3：Identity & Enrollment

固定配方 `machine-identity`（ADR-0032）、privileged raw collectors、identity file、identity-before-credentials、凭据文件（ADR-0032）、一个只含 `state`/`revision`/`last_audit_event_id` 的 current provisioning-window singleton、正常 open/close 审计 CAS 与 restart/restore close-once recovery、窗口内 Token + Gateway 联合签发（ADR-0033）、替换语义与审计。

**G3 覆盖**：identity 决策表全路径、configured-disk copy fixture、窗口开/关负向、正常 open/close audit+CAS、open-window restart/restore close-once 与 closed-window 零写入、联合签发事务原子性、CSR SAN ignore、替换语义与旧连接异常审计、package upgrade 保留 identity/凭据。

### Phase 4：Control Channel & Command Runtime

WSS + Bearer token 认证、subprotocol 版本协商、401-before-decode、durable direct-Command dispatcher、Device journal、receipt/status、Observed snapshot（变化 + 低频兜底）、same-ID replay/conflict、retry/reconnect 收敛。Panel-generated UUIDv7 与 `PUT` create/replay contract 已在 Phase 0 冻结；本阶段实现其 WSS/journal execution，并以 `frozen_payload_json` 表达每 Command 的 frozen typed input。

**G4 覆盖**：canonical UUIDv7/`201/200/400/409` contract、HTTP/WSS/journal/status/audit 同 ID、frame size/version/unknown enum、token 吊销即断、duplicate delivery 与 crash recovery、same-ID/different-request conflict、stale/conflict 拒绝、断线重连收敛、ErrorCode 跨 transport 一致。

### Phase 5：State, Data Plane & Secrets

Target derivation（派生代际）、explicit `SYNC_STATE`、Caddy 配置渲染/validate/reload/LKG 回滚、BLOCKED/READY、fixed TLS upstream、xheaders `/login` 注入（ADR-0034）、explicit `SYNC_SECRET` 与凭据文件/配置重渲染、Drift 与 operator views。

**G5 覆盖**：两段签发阶梯负向、Caddy validate/reload/rollback、bad cert/key/SAN 拒绝、offline LKG、upstream 非 TLS 拒绝激活、`/login` 之外无注入头、secret stale/retry/redaction、DOMjudge 契约回归、故障注入。

### Phase 6：Session & Home

package-owned XDG Autostart、resident hidden Session Agent、build-time Slint、local typed D-Bus、logind session validation、singleton/lease、session epoch lock/unlock/terminate、固定 contest user、选定 Home backend（限时定案）、多次重置流程。

**G6 覆盖**：当期镜像 capability 清单全项（ADR-0035）、中文 IME/HiDPI/focus denied、display lost 与 Agent crash、无 user unit、epoch race、lock/unlock 的 Caddy 调用数为 0、Home prepare/cleanup/fault/reboot 与连续多次重置。

### Phase 7：Production Release

production Deb、installation/preseed/reconfigure、upgrade/rollback、backup/restore、赛后审计导出、PKI 材料 runbook（control CA / origin CA）、observability、容量验证（500 台 WSS 并发）、operator/admin runbooks、readiness rehearsal、release artifacts 与 SBOM。

**G7 覆盖**：clean-site rehearsal、失败与恢复场景、restore to verified state、package lifecycle、offline steady state、500 台并发容量、audit export、Gateway validity 赛前校验、release decision 签署。

## 6. Gate 证据标准

每条 Gate 证据 = **指向 CI run / commit / artifact / 日志的链接 + 一行结论与日期**。

原则（保留）：

- 文件存在不等于通过；
- 截图不能替代可复现日志；
- VM 不能替代物理硬件 fixture；
- scaffold 不能替代实现；
- repo pin 不能替代环境冻结；
- partial pass 不关闭整个 Gate；
- waiver 不能放宽安全不变量。

## 7. 风险与缓冲

| 风险 | 缓解 |
|---|---|
| ICPC 镜像晚到或大版本变化 | 输入门禁 + 镜像升级重验清单（ADR-0035） |
| DOMjudge xheaders/brotli/TLS 契约不符 | lab 提前至 P0 收尾；结论冻结后才建 Phase 5 |
| 异构硬件 placeholder 超预期 | fixture 先行 + 首次 provisioning 实地全量验证（ADR-0032） |
| Caddy module/supply drift | version/source/checksum/module closure |
| PKI 材料延迟 | control CA / origin CA runbook 与 owner 提前准备 |
| Home backend 不可用 | 两候选限时定案，不 runtime fallback |
| GUI runtime closure 膨胀 | Slint feature allowlist、package scan |
| 多边界同时开发 | 以垂直切片和 Gate 控制合并 |
| 现场时间不足 | 2–4 周外部缓冲和 rehearsal |

## 8. 计划变更

需要更新本文件：Phase 顺序、Gate 退出标准、critical path、计划窗口、外部缓冲。

不在本文件维护：wire 字段、`INV-*` 正文、exact package version、平台 PASS、详细负向案例、runbook 命令。
