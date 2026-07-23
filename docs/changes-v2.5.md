# Natsume V2 v2.5 变更索引

> 状态：架构与工程基线索引，不表示目标环境、Probe 或 G0 已通过。  
> 架构基线：v2.5；Roadmap 基线：v1.2。

## 权威文档

- [`v2-design.md`](v2-design.md)：产品与架构权威设计；
- [`implementation-roadmap.md`](implementation-roadmap.md)：Phase 与 Gate 总览；
- [`implementation/`](implementation/)：分阶段实施计划；
- [`supported-platform.md`](supported-platform.md)：架构和目标环境冻结状态；
- [`requirements/phase-0.md`](requirements/phase-0.md)：Phase 0 REQ/Probe/G0 追踪；
- [`gates/g0-checklist.md`](gates/g0-checklist.md)：当前 G0 检查清单。

## v2.5 核心收敛

- 单赛事实例，不引入 `Event`、运行时 phase 或兼容层；
- Enrollment 只提交 Device Identity CSR，只返回 Device Identity leaf/chain；
- Gateway CSR/certificate 只存在于 mandatory-mTLS QUIC 的 active `SYNC_STATE`；
- 匿名 QUIC 在 TLS handshake 阶段拒绝，0-RTT 禁用；
- Session lock/unlock 不拥有或切换 Caddy 状态；
- 无 TOFU、Identity Guard、systemd credentials 或 postinstall/runtime download。

## Step 0–1 工程基线

- Cargo virtual workspace 独占 Rust graph 与 `Cargo.lock`；
- pnpm workspace 只包含 `web`，Node/pnpm 与 `pnpm-lock.yaml` 由仓库固定；
- Mermaid 图由固定版本的 `mermaid.parse` 校验；
- Caddy 和 nFPM 使用官方 release artifact、版本及 SHA-256 记录；
- 根 `justfile` 只分发原生命令，nFPM 只映射已构建产物；
- GitHub CODEOWNERS 只引用已验证的 repository collaborator。

上述工具链记录仍是 `ENV-PROPOSED`；在 locked CI、目标 OS 和供应链证据完成前，不得升级为 `ENV-FROZEN`，G0 继续保持 `OPEN`。
