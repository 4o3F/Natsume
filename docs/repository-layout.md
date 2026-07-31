# Natsume V2 仓库布局与模块所有权

> 状态：`NORMATIVE`  
> 原则：原生工具拥有各自依赖图；部署进程不等于代码级巨型模块

## 1. 顶层目录

```text
.github/                 CI、CODEOWNERS 和仓库策略
server/                  natsume-server
client/
  device-daemon/         natsume-device-daemon
  privileged-helper/     natsume-privileged-helper
  session-agent/         natsume-session-agent
crates/
  device-protocol/       Device control wire contract
  error-code/            稳定公开错误码
  local-control-api/     本地 IPC value types/contract
  machine-identity/      纯 Machine ID 判定
web/                     React/TypeScript operator UI
integration-tests/       跨 crate/进程契约、策略和集成测试
packaging/               nFPM、systemd、D-Bus、XDG、Caddy 包边界
docs/                    设计、决策、状态和 runbook
```

允许新增顶层目录的条件：有独立构建/发布或明确组织边界；不能合理归入现有 owner；有至少一个 production artifact；通过 ADR 或 repository-owner 评审。**不得为“看起来整齐”创建 `apps/`、`packages/`、`rust/`、`tools/`、`scripts/`、`assets/` 或 `pipeline/` 等第二套分类层。**

## 2. 工具所有权

| 工具 | 拥有 |
|---|---|
| Cargo | Rust workspace、crate graph、`Cargo.lock` |
| pnpm | Web workspace、Node graph、`pnpm-lock.yaml` |
| `just` | 命令分发，不重建依赖图 |
| nFPM | 将已构建 artifact 映射到 Deb |
| systemd/D-Bus/XDG files | 运行时拓扑和权限 |
| OpenAPI/Protobuf/D-Bus generator | 机器契约 |
| Node Mermaid validator | 文档 diagram syntax |

禁止：让 `just` 复制包管理器依赖解析；在 nFPM 内重新构建产品；为 Rust 再创建 npm package；手工维护生成契约；使用两个 Cargo workspace 或多个生产 `Cargo.lock`；用 postinstall 下载 Caddy、nFPM 或 runtime 组件。

## 3. Rust workspace

workspace 成员：`server/`、`client/device-daemon/`、`client/privileged-helper/`、`client/session-agent/`、`crates/device-protocol/`、`crates/error-code/`、`crates/local-control-api/`、`crates/machine-identity/`、`integration-tests/`。工作区统一 Rust edition、`rust-version`、常用 dependency version、lint policy、release profile，并禁止 `unsafe_code`、`unwrap`/`expect` 等已冻结规则。一个 binary crate 可以包含多个内部 module，但不能通过创建大量共享 crate 逃避模块边界设计。

## 4. Shared crate 准入

共享 crate 必须同时满足：至少两个真实 production consumer；contract 已稳定到值得独立版本/测试；不包含任一 consumer 的数据库、framework 或业务 orchestration；有明确 owner；依赖方向不形成环；独立 crate 比内部 module 更能保护边界。

| Crate | 责任 | 典型消费者 |
|---|---|---|
| `device-protocol` | Protobuf/wire contract 与 framing value | Server、Device Daemon、integration tests |
| `error-code` | 公开稳定错误码和值 | Server、Device、Agent/API adapters |
| `local-control-api` | D-Bus/local IPC value types | Device Daemon、Helper、Agent |
| `machine-identity` | 纯候选规范化、质量与 UUID 派生 | Device Daemon、Helper/tests |

禁止创建 `common`、`utils`、`helpers`、`shared-models`、`core`、`platform`、`errors`，除非名称被更具体的稳定责任替代。只有一个 consumer 的代码留在该 consumer 内。

## 5. 进程内部模块边界

每个有业务逻辑的进程应分离 transport / application / domain / port / adapter 层（见 [架构 §6](architecture.md)）。各进程的内部模块仅给出目标边界；具体 `src/` 结构在对应 Phase 实现时确定。

- **Server（`natsume-server`，composition root）**：内部模块按职责隔离（identity/enrollment、device control、contest domain、configuration target、command dispatch、operator API、audit/outbox、pki/vault）。各模块只通过明确 port 交互；**不得直接跨表写入或把 framework 类型泄漏到 domain。**
- **Device Daemon（`natsume-device-daemon`）**：分离 identity startup、enrollment、control、command runtime、target apply、gateway、caddy、session、home、observed、vault、journal。**QUIC handler 不直接操作 vault、journal、Caddy 或 D-Bus**；module 间传递 value object，不传递 transport request 或全局 mutable context；`identity_startup` 在其他 identity-bound adapter 初始化前运行。
- **Privileged Helper**：每个 capability 独立可审计（hardware sources、home backend、login session、filesystem policy）。**禁止 `execute(request)` 或 `run_action(name, args)` 一类通用入口**；path/UID 由固定 policy 重新派生，Device Daemon 传入值只作受限 ID。
- **Session Agent（`natsume-session-agent`）**：分离 platform（logind/session/singleton/desktop）、local_api、presentation、ui。**不得引入 Server client、vault、PKI、Caddy 或 privileged D-Bus client**；由系统级 XDG Autostart 直接启动。
- **Web（operator Web Panel）**：feature-oriented（api/generated、auth、preparation、devices、bindings、operations、audit、shared/ui）。**Web 只依赖生成 API 和自己的 view model，不复制 Rust domain enum 后自行演进**；`shared/ui` 只含无业务语义的视觉组件。

## 6. Tests

| 位置 | 责任 |
|---|---|
| crate 内 `tests`/unit | 纯规则、value object、adapter |
| `integration-tests/` | 跨 crate、协议、schema、policy |
| Web unit/component | view model、UI |
| Web e2e | operator journey |
| `docs/probes/` | 目标环境高风险验证报告 |
| packaging smoke | install/upgrade/remove/reboot |
| runbook rehearsal | 恢复和发布可执行性 |

探针报告不是自动化测试替代；自动化测试也不替代目标硬件证据。

## 7. Generated artifacts

生成源和产物必须同仓或在 CI 可重建：

| 源 | 产物 |
|---|---|
| Rust/OpenAPI schema | `web/openapi/...json` |
| OpenAPI | Web TypeScript schema |
| `.proto` | Rust types + descriptor golden |
| D-Bus XML/value types | adapter code/tests |
| Mermaid fences | parse validation |

生成产物不得手工编辑。CI 使用 clean diff 证明同步。

## 8. 包和运行时文件

Server package 只包含 Server artifact 和其明确依赖。Client package 只包含 Client 进程、Caddy 和运行时政策。固定边界：

- Session Agent 通过 `/etc/xdg/autostart/org.natsume.SessionAgent.desktop` 直接启动；
- 不安装 Session Agent systemd user unit；
- Device Daemon 身份检查内置，不安装 Identity Guard service；
- secret 不通过 systemd credentials；
- postinstall 不生成 CA/private key，不下载 runtime；
- nFPM 从仓库固定路径映射已构建产物；
- Caddy binary 和 checksum 由 supply-chain policy 管理。

## 9. 新模块审查清单

- Owner 和变化原因是什么？
- 哪些表/文件/secret 属于它？
- 它对外暴露的 port 是什么？
- 是否只有一个消费者而不应成为 crate？
- 是否泄漏 framework types？
- 是否需要 root 或外网？
- 是否能用现有 typed contract？
- 是否造成循环依赖？
- 是否可以独立测试？
- 删除它时影响哪些组件？

回答不清楚时，不新增“manager/service/common”层。
