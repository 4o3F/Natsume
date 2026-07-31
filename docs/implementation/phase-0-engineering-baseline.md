# Phase 0 — Engineering Baseline

> 计划：W1–W3  
> 当前状态：`IN-PROGRESS / G0 OPEN`  
> Requirement/Gate 状态：[phase-0-status](../gates/phase-0-status.md)

## 1. 目标

建立能够真实构建、测试、打包和验证关键架构边界的工程基线。Phase 0 不实现完整产品，而是消除会在后续放大的工具链、契约、平台和安全不确定性。

## 2. 入口条件

- V2 架构和 ADR 集合可审查；
- 仓库可以 clean checkout；
- Build、Architecture、Security、Packaging、Desktop、PKI 和 Lab owner 角色已分配；
- 目标环境输入可以在 Phase 0 窗口内获取，或明确标记 `BLOCKED-INPUT`。

## 3. 工作包

### P0.1 Monorepo 与工具链

- 冻结 Cargo workspace、pnpm workspace、`just` 和 nFPM 所有权；
- 固定 Rust、Node、pnpm、Mermaid、nFPM、Caddy、protoc；
- 单一 lockfile；
- 禁止占位命令和“工具缺失即跳过”；
- 建立 repository/module/dependency policy；
- 文档 link、diagram 和 verification registry 校验。

交付：

- root workspace/lockfiles；
- `justfile`；
- dependency/policy scan；
- 可重现 toolchain check。

### P0.2 真实 CI

PR 必须执行：

- Rust fmt、Clippy、unit、doc、cargo-deny；
- pnpm frozen install、format、lint、typecheck、unit、build；
- OpenAPI/TS clean diff；
- Protobuf descriptor；
- D-Bus contract；
- SQL migration；
- Mermaid；
- policy scan；
- package smoke。

Nightly/目标环境执行：

- install/reinstall/upgrade/remove/purge/reboot；
- high-risk fault；
- dependency/runtime closure。

### P0.3 Error model

- 第一方 typed SNAFU error；
- stable ErrorCode registry；
- HTTP Problem Details；
- Protobuf、D-Bus 和 CommandStatus 显式映射；
- redaction tests；
- 禁止解析 Display 文本。

### P0.4 Contract skeleton

- Device-only Enrollment；
- Device control envelope/framing；
- Gateway request/result 只在 active `SYNC_STATE`；
- Observed/CommandStatus；
- Local D-Bus；
- SQL migrations；
- machine-generated golden；
- certificate ladder 负向 contract test；
- XDG direct Agent contract；
- Slint Phase 6 boundary scaffold。

### P0.5 Empty Deb

- `natsume-server` 和 `natsume-client` 可构建/安装；
- sysusers/tmpfiles/mode；
- systemd services；
- D-Bus policy；
- package-owned Caddy；
- XDG Autostart；
- endpoint preseed/reconfigure；
- 无 Identity Guard；
- 无 Agent user unit；
- 无 runtime download、CA/private key 生成或 systemd credentials。

### P0.6 Probe A–F

- A：IP-SAN 与 endpoint；
- B：Enrollment → mTLS → Gateway CSR；
- C：Caddy/DOMjudge；
- D：Machine identity；
- E：Session Agent/Desktop/Home；
- F：Package/systemd。

Probe 报告位于 [`../probes/`](../probes/)，必须包含真实环境、步骤、结果、evidence 和 reviewer，不得只保留计划。

## 4. Definition of Done

- clean checkout 的所有 mandatory CI 真实运行；
- toolchain/artifact pin 可审计；
- 生成契约 clean；
- package smoke 真实；
- 所有 forbidden path 由 policy/negative tests 覆盖；
- Probe A–F 结果可定位；
- 目标环境和物理机已冻结；
- 15 项 G0 全部 PASS；
- 独立 G0 decision 由 Architecture、Engineering、QA 和 Gate Chair 签署。

## 5. 非目标

- 领域 CRUD、生产 Auth/RBAC/SSE；
- 生产 CSV；
- fleet-scale command executor；
- 生产 Caddy generator；
- 完整 Session/Home；
- 以 reference scaffold 宣称 Phase 6 完成；
- 在 G0 未通过时发布支持矩阵。

## 6. 主要风险

| 风险 | 控制 |
|---|---|
| 工具链只在开发机工作 | locked CI + clean checkout |
| 协议文档与 code 漂移 | generated descriptor/OpenAPI/D-Bus clean diff |
| 证书阶梯被简化 | `INV-CERT-01/02` + Probe B |
| 桌面环境太晚验证 | G0-IN-004 + Probe E |
| Machine ID 无物理证据 | G0-IN-005 + 6 台 fixture |
| package 拓扑只在文档 | install/upgrade/reboot smoke |
| 文档状态漂移 | link validator |
