# Natsume V2

Natsume 是面向单场竞赛现场的工作站控制与访问编排系统。本分支当前仍处于 **Phase 0 工程基线**：仓库已建立 Rust/Web workspace、锁文件、CI、打包拓扑、稳定错误码和部分契约骨架；领域功能、生产数据面与 Session/Home 全流程尚未完成。

本文档集采用“**一个事实、一个权威位置**”的维护规则；每个主题只有一个权威文档，其余位置只引用不复制。

## 阅读入口

- 文档地图与维护规则：[`docs/README.md`](docs/README.md)
- 系统架构：[`docs/architecture.md`](docs/architecture.md)
- 领域模型：[`docs/domain-model.md`](docs/domain-model.md)
- 边界契约：[`docs/contracts.md`](docs/contracts.md)
- 状态与执行模型：[`docs/state-and-execution.md`](docs/state-and-execution.md)
- 安全与恢复不变量：[`docs/security-recovery.md`](docs/security-recovery.md)
- 平台支持状态：[`docs/supported-platform.md`](docs/supported-platform.md)
- 实施路线图：[`docs/roadmap.md`](docs/roadmap.md)
- Phase 0 当前状态：[`docs/gates/phase-0-status.md`](docs/gates/phase-0-status.md)
- 贡献与评审自检：[`CONTRIBUTING.md`](CONTRIBUTING.md)

## 当前实现边界

当前可视为真实工程基线的内容包括：

- Cargo virtual workspace 与单一 `Cargo.lock`；
- pnpm Web workspace 与单一 `pnpm-lock.yaml`；
- `natsume-error-code`、协议/本地 API/Machine ID 等共享 crate 骨架；
- Rust、Web、契约、策略和 Deb smoke 的 CI 命令；
- Server/Client 包拓扑；
- 系统级 XDG Autostart 的 Session Agent 包边界。

以下仍是后续 Phase 的目标，不应从文档存在推断为已实现：

- 完整 Server 领域模型、admin/viewer 授权、审计与 Preparation Center；
- 生产 Enrollment（provisioning 窗口 + Device Token + Gateway certificate）、WSS 控制面与 Command journal；
- 生产 Caddy 激活、xheaders 自动登录与 DOMjudge 数据面；
- 成熟 Slint Session Agent；
- Session/Home 状态机；
- 完整备份、升级、演练和发布签收。

## 仓库拓扑

```text
server/                  natsume-server
client/
  device-daemon/         natsume-device-daemon
  privileged-helper/     natsume-privileged-helper
  session-agent/         natsume-session-agent
crates/
  device-protocol/
  error-code/
  local-control-api/
  machine-identity/
web/                     operator Web Panel
integration-tests/
packaging/
docs/
```

详细边界见 [`docs/repository-layout.md`](docs/repository-layout.md)。

## 常用命令

仓库根 `justfile` 只分发原生工具命令：

```bash
just toolchain
just install
just fmt
just lint
just unit
just api
just diagrams
just integration
just verify
just package
```

验证文档：

```bash
node docs/verification/validate-links.mjs docs README.md
node docs/verification/validate-markdown.mjs docs README.md
pnpm diagrams
```

## 关键设计边界

1. 一个初始化后的 Server 实例只服务当前一场竞赛，不建模多 Event。
2. 一切签发只发生在 provisioning 窗口内的 Enrollment：Device Token + Gateway certificate；窗口关闭后无签发路径。
3. Device control 为 server-auth TLS 上的 WSS + Device Token；无 token 的连接在解码前拒绝。
4. Target 不含密码且不自动产生远端副作用。
5. `SYNC_STATE` 与 `SYNC_SECRET` 都必须由操作员明确触发；密码只进入秘密专用路径。
6. Observed snapshot 是设备实际状态的唯一业务来源。
7. root Helper 无外网、无 DOMjudge 密码、无任意命令接口。
8. Session Agent 无凭据、PKI 或 Caddy 所有权。
9. Session lock/unlock 不改变 Caddy 配置、epoch 或状态。
10. 无法证明 Machine ID、本地凭据或 Home 安全时必须 fail closed。

完整规则以 [`docs/security-recovery.md`](docs/security-recovery.md) 的 `INV-*` 条目为准。

## 文档基线

- 架构来源：Natsume V2 v2.8 决策集合（[ADR-0022](docs/adr/0022-deployment-facts-and-trust-assumptions.md)–[0029](docs/adr/0029-right-sizing-control-plane-machinery.md)）；
- Phase 0 窗口：2026-07-23 至 2026-08-19；
- G0 当前结论：`OPEN`，不得从文档存在推断为 Gate 通过。

文档演进记录见 git history。
