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

允许新增顶层目录的条件：

1. 有独立构建/发布或明确组织边界；
2. 不能合理归入现有 owner；
3. 有至少一个 production artifact；
4. 通过 ADR 或 repository-owner 评审。

不得为“看起来整齐”创建 `apps/`、`packages/`、`rust/`、`tools/`、`scripts/`、`assets/` 或 `pipeline/` 等第二套分类层。

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

禁止：

- 让 `just` 复制包管理器依赖解析；
- 在 nFPM 内重新构建产品；
- 为 Rust 再创建 npm package；
- 手工维护生成契约；
- 使用两个 Cargo workspace 或多个生产 `Cargo.lock`；
- 用 postinstall 下载 Caddy、nFPM 或 runtime 组件。

## 3. Rust workspace

当前 workspace 成员：

```text
server/
client/device-daemon/
client/privileged-helper/
client/session-agent/
crates/device-protocol/
crates/error-code/
crates/local-control-api/
crates/machine-identity/
integration-tests/
```

工作区统一：

- Rust edition；
- `rust-version`；
- 常用 dependency version；
- lint policy；
- release profile；
- 禁止 `unsafe_code`、`unwrap`/`expect` 等已冻结规则。

一个 binary crate 可以包含多个内部 module，但不能通过创建大量共享 crate 逃避模块边界设计。

## 4. Shared crate 准入

共享 crate 必须同时满足：

1. 至少两个真实 production consumer；
2. contract 已稳定到值得独立版本/测试；
3. 不包含任一 consumer 的数据库、framework 或业务 orchestration；
4. 有明确 owner；
5. 依赖方向不会形成环；
6. 独立 crate 比内部 module 更能保护边界。

当前允许：

| Crate | 责任 | 典型消费者 |
|---|---|---|
| `device-protocol` | Protobuf/wire contract 与 framing value | Server、Device Daemon、integration tests |
| `error-code` | 公开稳定错误码和值 | Server、Device、Agent/API adapters |
| `local-control-api` | D-Bus/local IPC value types | Device Daemon、Helper、Agent |
| `machine-identity` | 纯候选规范化、质量与 UUID 派生 | Device Daemon、Helper/tests |

禁止创建：

- `common`
- `utils`
- `helpers`
- `shared-models`
- `core`
- `platform`
- `errors`

除非名称被更具体的稳定责任替代。只有一个 consumer 的代码留在该 consumer 内。

## 5. Server 内部模块

`server` binary 是 composition root。推荐内部边界：

```text
server/src/
  main.rs
  composition/
  identity_enrollment/
  device_control/
  contest_domain/
  configuration_target/
  command_dispatch/
  operator_api/
  audit_outbox/
  pki_vault/
  adapters/
```

### 5.1 `identity_enrollment`

拥有：

- Device lifecycle；
- Enrollment use case；
- Machine Hardware ID 冲突策略；
- Device certificate metadata；
- Device PKI port。

不得直接计算业务 Target 或读取密码。

### 5.2 `device_control`

拥有：

- authenticated connection registry；
- protocol adapter；
- Observed ingest；
- Gateway certificate request context validation。

不得直接跨表修改 binding、Seat 或 credential。

### 5.3 `contest_domain`

拥有：

- CSV committed domain state；
- Seat/account/credential metadata；
- binding；
- domain transactions。

只通过 port 访问 Server vault。不得依赖 Axum、Quinn 或 Web types。

### 5.4 `configuration_target`

拥有：

- Target derivation；
- generation/hash；
- Drift comparison。

应尽量是纯函数和值对象。不得发送 Command。

### 5.5 `command_dispatch`

拥有：

- Operation/Command/Attempt；
- outbox/queue；
- retry/expiry；
- connection port。

不得成为所有 CRUD 的通用 transaction wrapper。

### 5.6 `operator_api`

拥有：

- Axum/HTTP；
- auth/RBAC adapter；
- Problem Details；
- SSE/read models；
- OpenAPI export。

不得包含领域决策或 secret formatting。

### 5.7 `audit_outbox`

拥有：

- AuditEvent；
- ChangeEvent/outbox；
- redaction；
- SSE/consumer delivery。

领域 transaction 通过明确 port/transaction participant 原子写入，不能由日志替代。

### 5.8 `pki_vault`

拥有：

- Server vault adapter；
- key/certificate material；
- profile-specific signer；
- format/rotation。

不决定谁有资格签发；资格由 application/domain policy决定。

## 6. Device Daemon 内部模块

推荐：

```text
client/device-daemon/src/
  main.rs
  composition/
  identity_startup/
  enrollment/
  control/
  command_runtime/
  target_apply/
  gateway/
  caddy/
  session/
  home/
  observed/
  vault/
  journal/
  adapters/
```

强制依赖规则：

- QUIC handler 不直接操作 vault、SQLite/journal、Caddy 或 D-Bus；
- `control` 解码后调用 application use case；
- `command_runtime` 拥有幂等和恢复；
- `target_apply` 只创建验证后的 activation plan；
- `gateway` 只处理 key/CSR/certificate 语义；
- `caddy` adapter 只接受已验证 plan；
- `session` 不依赖 Enrollment 内部状态；
- `home` 不直接修改 Command journal；
- `identity_startup` 在其他 identity-bound adapter 初始化前运行；
- `observed` 从各 module 的 typed snapshot 组合，不读取内部数据库 row；
- module 间传递 value object，不传递 transport request 或全局 mutable context。

## 7. Privileged Helper

Helper 每个 capability 可独立审计，例如：

```text
hardware_sources/
home_backend/
login_session/
filesystem_policy/
```

禁止创建 `execute(request)` 或 `run_action(name, args)` 一类通用入口。

Helper 内 path/UID 由固定 policy 重新派生。Device Daemon 传入的值只作为受限 ID，不作为 root 动作直接参数。

## 8. Session Agent

建议：

```text
client/session-agent/src/
  main.rs
  platform/
  local_api/
  presentation/
  ui/
```

- `platform`：logind、session probe、singleton、desktop capability；
- `local_api`：Daemon typed contract；
- `presentation`：snapshot → view model；
- `ui`：Slint adapter；
- `main`：XDG Autostart composition。

不得引入 Server client、vault、Caddy 或 privileged D-Bus client。

## 9. Web

建议 feature-oriented，但 API contract 统一生成：

```text
web/src/
  api/generated/
  auth/
  preparation/
  devices/
  bindings/
  operations/
  audit/
  platform/
  shared/ui/
```

`shared/ui` 只包含无业务语义的视觉组件。业务 hooks、schemas 和状态不得塞入 `shared`。

Web 只能依赖生成 API 和自己的 view model；不能复制 Rust domain enum 后自行演进。

## 10. Tests

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

## 11. Generated artifacts

生成源和产物必须同仓或在 CI 可重建：

| 源 | 产物 |
|---|---|
| Rust/OpenAPI schema | `web/openapi/...json` |
| OpenAPI | Web TypeScript schema |
| `.proto` | Rust types + descriptor golden |
| D-Bus XML/value types | adapter code/tests |
| `verification/registry.json` | Phase 0 Markdown views |
| Mermaid fences | parse validation |

生成产物不得手工编辑。CI 使用 clean diff 证明同步。

## 12. 包和运行时文件

Server package 只包含 Server artifact 和其明确依赖。Client package 只包含 Client 进程、Caddy 和运行时政策。

固定边界：

- Session Agent 通过 `/etc/xdg/autostart/org.natsume.SessionAgent.desktop` 直接启动；
- 不安装 Session Agent systemd user unit；
- Device Daemon 身份检查内置，不安装 Identity Guard service；
- secret 不通过 systemd credentials；
- postinstall 不生成 CA/private key，不下载 runtime；
- nFPM 从仓库固定路径映射已构建产物；
- Caddy binary 和 checksum 由 supply-chain policy 管理。

## 13. 新模块审查清单

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
