# Natsume V2 Monorepo Blueprint

> Blueprint revision: v2.5; architecture baseline: v2.5; roadmap baseline: v1.2.

这是 Natsume V2 v2.5 的目录、workspace、协议、数据库、systemd、Debian 打包、阶段计划和测试骨架。它用于冻结 ownership 与关键契约，不是已完成的业务实现。

- 权威架构设计：[`docs/v2-design.md`](docs/v2-design.md)
- 总体 Roadmap：[`docs/implementation-roadmap.md`](docs/implementation-roadmap.md)
- 分阶段详细实施计划：[`docs/implementation/`](docs/implementation/)
- 本次变更：[`docs/changes-v2.5.md`](docs/changes-v2.5.md)

## v2.5 冻结项

- 一个初始化后的实例只服务当前赛事；无 `Event`、phase 或多赛事兼容层。
- Natsume 只管理 `seat,account,password`；单次导入只接受一个固定 schema 的 UTF-8 CSV；首次提交后 Seat 全集冻结，后续文件必须包含完全相同的 Seat 集合。
- Device 只有一个不可修改的 `MachineHardwareId`；无 installation instance、version、merge/split 或冗余 identity fields；站点 `fleet_namespace_uuid` 跨赛事重置保持不变。
- Machine ID 文件校验、vault 打开和本地 reset 都由 `natsume-device-daemon` 的启动流程负责；无独立 Identity Guard service。
- 首次接入使用仅验证 Server 的 HTTPS Enrollment，经人工批准或受限自动批准后，只签发 Daemon 的 Device Identity `clientAuth` certificate；无 bootstrap token，Enrollment 不接受或返回 Gateway CSR/certificate。
- 正常设备控制使用 Quinn/QUIC + mandatory mTLS；首次 Enrollment 与正常 control 使用不同 TLS 配置。
- Gateway private key/CSR/certificate 延迟到显式 `SYNC_STATE`：Daemon 通过已认证 QUIC 提交绑定 command/generation/configuration/SPKI 的请求，Server 从冻结 target 派生 SAN/profile 后签发。
- `DeviceTargetState` 是不主动下发的非秘密目标记录；应用必须创建 `SYNC_STATE` Command。
- Password 不在 target state 中，只能通过 human-triggered `SYNC_SECRET` Command 分发。
- `ObservedStateSnapshot` 是设备事实与 apply progress 的唯一状态来源；无 `DesiredStateStatus`。
- Session lock/unlock 只控制桌面与 Session Agent gate，不 reload Caddy。
- Server/Client 的持久化秘密是 SQLite 中的 AEAD ciphertext；root key 是受文件权限保护的随机 key，不使用 systemd credentials。
- Client 安装通过 nFPM Debian debconf `templates`/`config` 收集 Server IP/port，并保存到 `/etc/natsume/config.toml`；站点 namespace、Control Root 与 Local Origin Root 由构建期签名输入注入。
- 总体 Roadmap 只定义各 Phase 的责任和 Gate；Phase 0–7 的任务顺序、工作包、测试矩阵与风险分别维护在独立文件中。

## 仓库模型

- Cargo virtual workspace 管全部 Rust packages；pnpm workspace 管 `web/`；根 `justfile` 只委托原生工具。
- nFPM 直接映射 Rust binaries、`web/dist`、固定 Caddy 和 package-owned rootfs，最终只发布 Server/Client 两个 Debian packages。
- `crates/error-code`、`crates/device-protocol`、`crates/local-control-api`、`crates/machine-identity` 是仅有的共享生产契约。
- `crates/error-code` 是第四个、也是 Phase 0 唯一新增的共享生产契约；它独占稳定错误字符串、HTTP/protocol/D-Bus 显式映射与报告脱敏，各领域仍保留自己的 typed SNAFU error。
- Package-owned Caddy 状态页不是第二个 Web application。

## Step 1 工具链基线

- Node `24.1.0`、pnpm `11.1.0` 和 Rust `1.97.1` 由仓库固定；
- pnpm workspace 仍只包含 `web`，依赖图记录在 `pnpm-lock.yaml`；
- `pnpm diagrams` 使用固定版本的 Mermaid 校验 `docs/` 中的全部 Mermaid fences；
- Caddy `2.11.4` 与 nFPM `2.47.0` 使用官方 release artifact 和 SHA-256；
- Caddy、nFPM、站点材料和任何 secret 均不得由 postinstall 或 runtime 下载。

这些记录仍是工程候选，不表示目标环境或 G0 已通过；当前状态以 [`docs/supported-platform.md`](docs/supported-platform.md) 和 [`docs/gates/g0-checklist.md`](docs/gates/g0-checklist.md) 为准。

## Blueprint 限制

仓库提交真实 Cargo/pnpm lockfile 和公开供应链 checksum，但不伪造生产 CA、密码、目标环境或实验室证据。正式实现仍必须构建并验证真实 binary，运行 target-OS/physical-hardware/fault tests，并满足 `docs/v2-design.md` 和每个 Phase Gate 的全部发布门禁。
